use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, warn};

use crate::dispatcher::Dispatcher;
use crate::state::home;

// ── Socket paths ─────────────────────────────────────────────────────────────

/// Unix socket the daemon listens on.
pub fn socket_path() -> PathBuf {
    home::daemon_socket_path()
}

/// Marker file indicating the systemd service has been registered.
fn marker_path() -> PathBuf {
    home::gitzi_home().join("service-installed")
}

fn binary_path() -> Result<PathBuf> {
    std::env::current_exe().map_err(|e| anyhow::anyhow!("Cannot determine binary path: {e}"))
}

// ── Client: connect to running daemon ────────────────────────────────────────

/// Try to connect to the daemon socket. Returns Ok if daemon is alive.
pub async fn connect() -> Result<UnixStream> {
    let path = socket_path();
    let stream = UnixStream::connect(&path).await?;
    Ok(stream)
}

/// Check if the daemon is already running by attempting a connection.
pub async fn is_running() -> bool {
    connect().await.is_ok()
}

/// Ping the daemon and return its status response.
pub async fn ping() -> Result<String> {
    let mut stream = connect().await?;
    stream.write_all(b"ping\n").await?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response).await?;
    Ok(response.trim().to_string())
}

// ── Daemon: socket server ────────────────────────────────────────────────────

/// Run the daemon server loop. Listens on the unix socket and accepts
/// commands from TUI clients. This function never returns under normal operation.
pub async fn serve(dispatcher: Arc<Dispatcher>) -> Result<()> {
    let path = socket_path();

    // Clean up stale socket file
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(&path)?;
    info!("Daemon listening on {}", path.display());

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let d = Arc::clone(&dispatcher);
                tokio::spawn(handle_client(stream, d));
            }
            Err(e) => {
                error!("Failed to accept connection: {e}");
            }
        }
    }
}

async fn handle_client(stream: UnixStream, dispatcher: Arc<Dispatcher>) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();

        // subscribe holds the connection open — handled separately
        if trimmed == "subscribe" {
            handle_subscribe(&dispatcher, &mut writer).await;
            return; // connection consumed by subscription stream
        }

        let response = match trimmed {
            "ping" => "pong".to_string(),
            "status" => "running".to_string(),
            "peek_review" => handle_peek_review(&dispatcher).await,
            "board" => handle_board(&dispatcher).await,
            "epics" => handle_epics(&dispatcher).await,
            "queue_len" => handle_queue_len(&dispatcher).await,
            cmd if cmd.starts_with("approve ") => {
                let task_id = cmd.strip_prefix("approve ").unwrap().trim();
                handle_approve(&dispatcher, task_id).await
            }
            cmd if cmd.starts_with("answer ") => {
                handle_answer(&dispatcher, cmd.strip_prefix("answer ").unwrap().trim()).await
            }
            cmd if cmd.starts_with("chat ") => {
                let encoded = cmd.strip_prefix("chat ").unwrap().trim();
                let message: String = serde_json::from_str(encoded)
                    .unwrap_or_else(|_| encoded.to_string());
                // Spawn chat processing with interrupt classification support.
                let d = Arc::clone(&dispatcher);
                tokio::spawn(async move {
                    handle_chat_with_interrupt(d, message).await;
                });
                "queued".to_string()
            }
            "chat_history" => handle_chat_history(&dispatcher).await,
            "close_fork" => handle_close_fork(&dispatcher).await,
            other => format!("error: unknown command '{other}'"),
        };
        if writer.write_all(format!("{response}\n").as_bytes()).await.is_err() {
            break;
        }
    }
}

/// Stream JSON-encoded DispatchEvent lines until the connection closes.
async fn handle_subscribe(
    dispatcher: &Dispatcher,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) {
    use tokio::sync::broadcast::error::RecvError;

    let mut rx = dispatcher.event_bus.subscribe();
    loop {
        match rx.recv().await {
            Ok(event) => {
                let json = match serde_json::to_string(&event) {
                    Ok(j) => j,
                    Err(e) => {
                        warn!("failed to serialize event: {e}");
                        continue;
                    }
                };
                if writer.write_all(format!("{json}\n").as_bytes()).await.is_err() {
                    break; // client disconnected
                }
            }
            Err(RecvError::Lagged(n)) => {
                warn!(skipped = n, "subscribe client lagged");
                continue;
            }
            Err(RecvError::Closed) => break,
        }
    }
}

async fn handle_peek_review(dispatcher: &Dispatcher) -> String {
    // Clone the item so we can release the queue lock before taking the board lock.
    let item = {
        let queue = dispatcher.review_queue.lock().await;
        queue.peek().cloned()
    };

    match item {
        None => "null".to_string(),
        Some(item) => {
            let task_title = {
                let board = dispatcher.board.read().await;
                board.task(&item.task_id).map(|t| t.title.clone())
            };
            // Merge item fields + task_title into a single JSON object.
            let mut value = match serde_json::to_value(&item) {
                Ok(v) => v,
                Err(e) => return format!("error: {e}"),
            };
            if let Some(obj) = value.as_object_mut() {
                obj.insert(
                    "task_title".to_string(),
                    task_title.map_or(serde_json::Value::Null, serde_json::Value::String),
                );
            }
            serde_json::to_string(&value).unwrap_or_else(|e| format!("error: {e}"))
        }
    }
}

async fn handle_board(dispatcher: &Dispatcher) -> String {
    use crate::dispatcher::Column;
    use serde_json::{json, Value};

    let board = dispatcher.board.read().await;
    let columns: Vec<Value> = Column::all()
        .iter()
        .map(|col| {
            let task_ids = board.tasks_in(*col);
            let tasks: Vec<Value> = task_ids
                .iter()
                .filter_map(|id| board.task(id))
                .map(|t| {
                    json!({
                        "id": t.id,
                        "epic": t.epic,
                        "title": t.title,
                        "priority": t.priority,
                    })
                })
                .collect();
            json!({
                "column": col.to_string(),
                "tasks": tasks,
            })
        })
        .collect();
    serde_json::to_string(&columns).unwrap_or_else(|e| format!("error: {e}"))
}

/// Return all epics (id, title, child task IDs) as a JSON array — used by the
/// TUI's Status panel to compute current-epic progress.
async fn handle_epics(dispatcher: &Dispatcher) -> String {
    match dispatcher.gitzi_list_epics().await {
        Ok(epics) => serde_json::to_string(&epics).unwrap_or_else(|e| format!("error: {e}")),
        Err(e) => format!("error: {e}"),
    }
}

/// Return the count of pending agent-question review items (the clarification
/// queue size) as a bare number.
async fn handle_queue_len(dispatcher: &Dispatcher) -> String {
    let queue = dispatcher.review_queue.lock().await;
    queue.question_count().to_string()
}

async fn handle_approve(dispatcher: &Dispatcher, task_id: &str) -> String {
    match dispatcher.approve(task_id).await {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("error: {e}"),
    }
}

/// Parse `<item_id> <answer...>` — first token is item_id, rest is answer.
async fn handle_answer(dispatcher: &Dispatcher, args: &str) -> String {
    let (item_id, answer) = match args.split_once(' ') {
        Some((id, ans)) => (id, ans.to_string()),
        None => return "error: answer requires <item_id> <answer>".to_string(),
    };
    match dispatcher.answer_question(item_id, answer).await {
        Ok(()) => "ok".to_string(),
        Err(e) => format!("error: {e}"),
    }
}

/// Handle a chat message with interrupt classification.
/// If no chat is in-flight, starts one. If one IS in-flight, classifies and routes.
async fn handle_chat_with_interrupt(dispatcher: Arc<Dispatcher>, message: String) {
    use crate::agent::classifier::{self, InterruptAction};
    use crate::dispatcher::event_bus::DispatchEvent;

    // Check if a chat is already in-flight (top of stack)
    let in_flight = {
        let guard = dispatcher.chat_stack.lock().await;
        guard.last().map(|f| f.pending_message.clone())
    };

    if let Some(pending_message) = in_flight {
        // A chat is in-flight — classify the interrupt
        let context_turns: Vec<String> = {
            let hist = dispatcher.chat_history.lock().await;
            hist.iter()
                .rev()
                .take(5)
                .map(|m| format!("{}: {}", match m.role {
                    crate::state::chat::Role::User => "user",
                    crate::state::chat::Role::Agent => "agent",
                    crate::state::chat::Role::System => "system",
                }, &m.content))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        };

        let base_url = dispatcher.main_agent.base_url();
        let model = dispatcher.main_agent.model();

        let action = classifier::classify(
            &base_url,
            &model,
            &context_turns,
            &pending_message,
            &message,
        ).await;

        info!(?action, "interrupt classified");

        match action {
            InterruptAction::Amend => {
                // Cancel the top-of-stack request
                {
                    let mut guard = dispatcher.chat_stack.lock().await;
                    if let Some(entry) = guard.pop() {
                        entry.abort_handle.abort();
                    }
                }
                // Merge messages and retry at the same stack level
                let merged = format!("{}\n\n[amended]: {}", pending_message, message);
                start_chat_turn(dispatcher, merged, None).await;
            }
            InterruptAction::Queue => {
                // Wait for the top-of-stack turn to finish, then process
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    let guard = dispatcher.chat_stack.lock().await;
                    if guard.is_empty() {
                        break;
                    }
                }
                start_chat_turn(dispatcher, message, None).await;
            }
            InterruptAction::Fork => {
                // Name the fork
                let fork_name = classifier::name_fork(&base_url, &model, &message).await;
                let fork_id = uuid::Uuid::new_v4().to_string();
                info!(fork_id = %fork_id, fork_name = %fork_name, "creating fork");

                // Emit fork created event for TUI
                dispatcher.event_bus.emit(DispatchEvent::ForkCreated {
                    id: fork_id.clone(),
                    name: fork_name.clone(),
                });

                // Push a new entry onto the stack and start the turn
                start_chat_turn(
                    dispatcher,
                    message,
                    Some((fork_id, fork_name)),
                ).await;
            }
        }
    } else {
        // Nothing in-flight — start a new turn
        start_chat_turn(dispatcher, message, None).await;
    }
}

/// Start a chat turn, pushing it onto the stack for interrupt tracking.
/// When the turn completes, it pops itself from the stack.
/// If `fork_info` is Some, this is a new fork with the given id and name.
async fn start_chat_turn(
    dispatcher: Arc<Dispatcher>,
    message: String,
    fork_info: Option<(String, String)>,
) {
    use crate::dispatcher::{ForkEntry, event_bus::DispatchEvent};

    let d = Arc::clone(&dispatcher);
    let msg = message.clone();
    // TODO: When fork_info is Some, chat should persist to fork_chat_file(fork_id)
    // instead of current_chat_file(). Full isolation requires chat() to accept a
    // target path or an alternate history vec.
    let handle = tokio::spawn(async move {
        d.chat(&msg).await
    });

    let (fork_id, fork_name) = fork_info.unwrap_or_else(|| {
        ("main".to_string(), "Main".to_string())
    });

    // Seed fork_history: clone the last 50 entries from the dispatcher's chat history
    // when this is a named fork; empty for "main" entries.
    let fork_history = if fork_id != "main" {
        let hist = dispatcher.chat_history.lock().await;
        let start = hist.len().saturating_sub(50);
        hist[start..].to_vec()
    } else {
        Vec::new()
    };

    // Push onto the stack
    {
        let mut guard = dispatcher.chat_stack.lock().await;
        guard.push(ForkEntry {
            id: fork_id,
            name: fork_name,
            pending_message: message,
            abort_handle: handle.abort_handle(),
            turn_active: true,
            fork_history,
        });
    }

    // Await result
    let result = match handle.await {
        Ok(Ok(response)) => response,
        Ok(Err(e)) => format!("Error: {e}"),
        Err(_) => {
            // Aborted (amend case) — the amend handler already popped us
            return;
        }
    };

    // Mark turn as inactive (fork stays on stack until user/agent closes)
    {
        let mut guard = dispatcher.chat_stack.lock().await;
        if let Some(entry) = guard.last_mut() {
            entry.turn_active = false;
        }
        // If this is the "main" entry (not a named fork), pop it immediately
        if guard.last().is_some_and(|e| e.id == "main") {
            guard.pop();
        }
    }

    dispatcher.event_bus.emit(DispatchEvent::ChatResponse { content: result });
}

/// Handle "close_fork" command: pop the top fork (if not main) and emit ForkClosed.
async fn handle_close_fork(dispatcher: &Dispatcher) -> String {
    let popped = {
        let mut guard = dispatcher.chat_stack.lock().await;
        if guard.last().is_some_and(|e| e.id != "main") {
            guard.pop()
        } else {
            None
        }
    };
    match popped {
        Some(entry) => {
            entry.abort_handle.abort();
            let id = entry.id.clone();
            dispatcher.event_bus.emit(
                crate::dispatcher::event_bus::DispatchEvent::ForkClosed {
                    id: id.clone(),
                    summary: "Closed by user".to_string(),
                },
            );
            format!("ok: fork {id} closed")
        }
        None => "noop: not in a fork".to_string(),
    }
}

/// Return the full chat history as a JSON array.
async fn handle_chat_history(dispatcher: &Dispatcher) -> String {
    let history = dispatcher.chat_history.lock().await;
    serde_json::to_string(&*history)
        .unwrap_or_else(|_| "[]".to_string())
}

// ── Service registration (systemd / launchd) ─────────────────────────────────

/// Returns true if the service has never been registered (no marker file).
pub fn should_install() -> bool {
    !marker_path().exists()
}

/// Register the daemon as a system service so it starts on login/boot.
pub fn install() -> Result<()> {
    #[cfg(target_os = "linux")]
    install_systemd()?;

    #[cfg(target_os = "macos")]
    install_launchd()?;

    // Write marker so we don't re-register
    let marker = marker_path();
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&marker, "installed")?;
    Ok(())
}

/// Unregister the system service.
pub fn uninstall() -> Result<()> {
    #[cfg(target_os = "linux")]
    uninstall_systemd()?;

    #[cfg(target_os = "macos")]
    uninstall_launchd()?;

    let _ = std::fs::remove_file(marker_path());
    let _ = std::fs::remove_file(socket_path());
    Ok(())
}

/// Ensure the daemon is running. If not, register (if needed) and start it.
pub async fn ensure_running() -> Result<()> {
    if is_running().await {
        return Ok(());
    }

    // First run: register with systemd
    if should_install() {
        info!("First run — registering gitzi daemon as system service");
        install()?;
        // systemd starts it for us, wait a moment then verify
        tokio::time::sleep(Duration::from_secs(2)).await;
        if is_running().await {
            return Ok(());
        }
    }

    // Service registered but not running — start it
    start_service()?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    if is_running().await {
        Ok(())
    } else {
        anyhow::bail!("Failed to start gitzi daemon — check `systemctl --user status gitzi`")
    }
}

fn start_service() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("systemctl")
            .args(["--user", "start", "gitzi.service"])
            .status()
            .ok();
    }
    #[cfg(target_os = "macos")]
    {
        let plist_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("Library/LaunchAgents/io.gitzi.daemon.plist");
        std::process::Command::new("launchctl")
            .args(["load", &plist_path.to_string_lossy()])
            .status()
            .ok();
    }
    Ok(())
}

// ── Linux / systemd ──────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn install_systemd() -> Result<()> {
    use std::fmt::Write;

    let binary = binary_path()?;
    let home = std::env::var("HOME").unwrap_or_else(|_| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .to_string_lossy()
            .into_owned()
    });

    let mut unit = String::new();
    writeln!(unit, "[Unit]")?;
    writeln!(unit, "Description=gitzi agent orchestrator daemon")?;
    writeln!(unit, "After=network.target")?;
    writeln!(unit)?;
    writeln!(unit, "[Service]")?;
    writeln!(unit, "Type=simple")?;
    writeln!(unit, "ExecStart={} --daemon", binary.display())?;
    writeln!(unit, "Environment=HOME={home}")?;
    writeln!(unit, "Restart=on-failure")?;
    writeln!(unit, "RestartSec=5")?;
    writeln!(unit)?;
    writeln!(unit, "[Install]")?;
    writeln!(unit, "WantedBy=default.target")?;

    let service_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("systemd/user");

    std::fs::create_dir_all(&service_dir)?;
    let service_path = service_dir.join("gitzi.service");
    std::fs::write(&service_path, &unit)?;

    std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .ok();
    std::process::Command::new("systemctl")
        .args(["--user", "enable", "gitzi.service"])
        .status()
        .ok();
    std::process::Command::new("systemctl")
        .args(["--user", "start", "gitzi.service"])
        .status()
        .ok();

    info!("systemd user service installed at {}", service_path.display());
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_systemd() -> Result<()> {
    std::process::Command::new("systemctl")
        .args(["--user", "stop", "gitzi.service"])
        .status()
        .ok();
    std::process::Command::new("systemctl")
        .args(["--user", "disable", "gitzi.service"])
        .status()
        .ok();

    let service_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("systemd/user/gitzi.service");
    let _ = std::fs::remove_file(service_path);

    std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .ok();
    Ok(())
}

// ── macOS / launchd ──────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn install_launchd() -> Result<()> {
    let binary = binary_path()?;
    let label = "io.gitzi.daemon";

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{binary}</string>
    <string>--daemon</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>/tmp/gitzi-daemon.log</string>
  <key>StandardErrorPath</key>
  <string>/tmp/gitzi-daemon.err</string>
</dict>
</plist>"#,
        binary = binary.display()
    );

    let plist_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("Library/LaunchAgents");
    std::fs::create_dir_all(&plist_dir)?;
    let plist_path = plist_dir.join(format!("{label}.plist"));
    std::fs::write(&plist_path, &plist)?;

    std::process::Command::new("launchctl")
        .args(["load", &plist_path.to_string_lossy()])
        .status()
        .ok();

    info!("launchd agent installed at {}", plist_path.display());
    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_launchd() -> Result<()> {
    let label = "io.gitzi.daemon";
    let plist_path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(format!("Library/LaunchAgents/{label}.plist"));

    std::process::Command::new("launchctl")
        .args(["unload", &plist_path.to_string_lossy()])
        .status()
        .ok();
    let _ = std::fs::remove_file(plist_path);
    Ok(())
}
