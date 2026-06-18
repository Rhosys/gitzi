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
    home::gitzi_home().join("daemon.sock")
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
            cmd if cmd.starts_with("reject ") => {
                handle_reject(&dispatcher, cmd.strip_prefix("reject ").unwrap().trim()).await
            }
            cmd if cmd.starts_with("answer ") => {
                handle_answer(&dispatcher, cmd.strip_prefix("answer ").unwrap().trim()).await
            }
            cmd if cmd.starts_with("chat ") => {
                let encoded = cmd.strip_prefix("chat ").unwrap().trim();
                let message: String = serde_json::from_str(encoded)
                    .unwrap_or_else(|_| encoded.to_string());
                handle_chat(&dispatcher, &message).await
            }
            "chat_history" => handle_chat_history(&dispatcher).await,
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

/// Parse `<task_id> <feedback...>` — first token is task_id, rest is feedback.
async fn handle_reject(dispatcher: &Dispatcher, args: &str) -> String {
    let (task_id, feedback) = match args.split_once(' ') {
        Some((id, fb)) => (id, fb.to_string()),
        None => return "error: reject requires <task_id> <feedback>".to_string(),
    };
    match dispatcher.reject(task_id, feedback).await {
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

/// Run the user's chat message through the main agent, return JSON-encoded response.
async fn handle_chat(dispatcher: &Dispatcher, message: &str) -> String {
    match dispatcher.chat(message).await {
        Ok(response) => serde_json::to_string(&response)
            .unwrap_or_else(|e| format!("\"error serializing: {e}\"")),
        Err(e) => serde_json::to_string(&format!("Error: {e}"))
            .unwrap_or_else(|_| "\"error\"".to_string()),
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
