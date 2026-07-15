use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex as TokioMutex, Notify, broadcast};
use tracing::{error, info, warn};

use crate::config::Config;
use crate::dispatcher::Dispatcher;
use crate::setup::{self, SetupState};
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

// ── Bootstrap setup phase (ADR-002) ──────────────────────────────────────────

/// Shared state for the setup phase, held by the accept loop and every client
/// handler. All discovery/activation logic lives in `crate::setup`; this just
/// publishes state and routes the frontend's selections into it.
struct SetupSession {
    /// Current state, queryable on demand via `setup_state`.
    state: TokioMutex<SetupState>,
    /// Broadcasts every state change to `subscribe`d frontends.
    tx: broadcast::Sender<SetupState>,
    /// The config under construction — mutated by activations, persisted on success.
    config: TokioMutex<Config>,
    /// Fired once the gate is satisfied; the accept loop stops on it.
    ready: Notify,
}

impl SetupSession {
    async fn set_state(&self, state: SetupState) {
        *self.state.lock().await = state.clone();
        let _ = self.tx.send(state);
    }
}

/// Run the bootstrap "setup or use" phase on the daemon socket until a valid
/// control-plane provider exists, then return the now-ready `Config` (with
/// `main` bound to the activated provider). The dispatcher is only built from
/// the value this returns, so no pipeline agents come alive until the gate
/// clears (ADR-002).
pub async fn run_setup(initial: Config) -> Result<Config> {
    let path = socket_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&path)?;
    info!("entering setup mode — no LLM provider configured yet");

    let (tx, _) = broadcast::channel(16);
    let session = Arc::new(SetupSession {
        state: TokioMutex::new(SetupState::Loading),
        tx,
        config: TokioMutex::new(initial),
        ready: Notify::new(),
    });

    // Kick off the initial scan in the background so the splash shows promptly.
    spawn_scan(Arc::clone(&session));

    // Accept connections until the gate clears.
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        tokio::spawn(handle_setup_client(stream, Arc::clone(&session)));
                    }
                    Err(e) => error!("setup: failed to accept connection: {e}"),
                }
            }
            _ = session.ready.notified() => break,
        }
    }

    // Tear down the setup listener so the real server can rebind the socket.
    drop(listener);
    let _ = std::fs::remove_file(&path);

    let config = session.config.lock().await.clone();
    info!("setup complete — a control-plane provider is configured");
    Ok(config)
}

/// Run the (blocking) environment scan on a background task and publish the
/// resulting `NeedsProvider`/`Error` state. Discovered providers are merged
/// into the session config and persisted so activation has entries to work on.
///
/// If the only available providers are Bedrock (no local LLMs), auto-activates
/// the first one immediately — driving the AWS SSO device flow without waiting
/// for manual selection.
fn spawn_scan(session: Arc<SetupSession>) {
    tokio::spawn(async move {
        session.set_state(SetupState::Loading).await;
        let cfg = session.config.lock().await.clone();
        let scanned = tokio::task::spawn_blocking(move || {
            let mut cfg = cfg;
            let state = setup::scan_to_state(&mut cfg);
            (state, cfg)
        })
        .await;
        match scanned {
            Ok((state, cfg)) => {
                {
                    let mut guard = session.config.lock().await;
                    *guard = cfg;
                    let _ = guard.write(std::path::Path::new("."));
                }

                // Auto-activate if only Bedrock providers are available — don't
                // make the user manually select what's obvious.
                if let SetupState::NeedsProvider { ref candidates } = state {
                    let all_bedrock =
                        !candidates.is_empty() && candidates.iter().all(|c| c.kind == "bedrock");
                    let single_candidate = candidates.len() == 1;

                    if all_bedrock || single_candidate {
                        let name = candidates[0].name.clone();
                        info!("auto-activating provider '{name}' (only available option)");
                        session.set_state(SetupState::Loading).await;
                        let result = {
                            let mut config = session.config.lock().await;
                            setup::activate(&mut config, &name, None, None).await
                        };
                        match result {
                            Ok(setup::ActivationOutcome::Activated { message }) => {
                                let ready = {
                                    let config = session.config.lock().await;
                                    let _ = config.write(std::path::Path::new("."));
                                    setup::gate_ready(&config)
                                };
                                if ready {
                                    session.set_state(SetupState::Ready).await;
                                    session.ready.notify_one();
                                } else {
                                    session
                                        .set_state(SetupState::NeedsProvider {
                                            candidates: candidates.clone(),
                                        })
                                        .await;
                                }
                                info!("{message}");
                            }
                            Ok(setup::ActivationOutcome::NeedsMoreInput { message }) => {
                                // SSO flow needs account/role selection — fall through
                                // to the picker so the user can choose.
                                info!("{message}");
                                session
                                    .set_state(SetupState::NeedsProvider {
                                        candidates: candidates.clone(),
                                    })
                                    .await;
                            }
                            Err(e) => {
                                warn!("auto-activation failed: {e}");
                                session
                                    .set_state(SetupState::Error {
                                        messages: vec![format!(
                                            "Auto-activation of '{name}' failed: {e}"
                                        )],
                                        can_rescan: true,
                                    })
                                    .await;
                            }
                        }
                        return;
                    }
                }

                session.set_state(state).await;
            }
            Err(e) => {
                session
                    .set_state(SetupState::Error {
                        messages: vec![format!("scan failed: {e}")],
                        can_rescan: true,
                    })
                    .await;
            }
        }
    });
}

#[derive(serde::Deserialize)]
struct SetupSelect {
    name: String,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    role_name: Option<String>,
}

async fn handle_setup_client(stream: UnixStream, session: Arc<SetupSession>) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();

        if trimmed == "subscribe" {
            stream_setup_state(&session, &mut writer).await;
            return;
        }

        let response = match trimmed {
            "ping" => "pong".to_string(),
            "setup_state" => {
                let state = session.state.lock().await.clone();
                serde_json::to_string(&state).unwrap_or_else(|e| format!("error: {e}"))
            }
            "setup_rescan" => {
                spawn_scan(Arc::clone(&session));
                let state = session.state.lock().await.clone();
                serde_json::to_string(&state).unwrap_or_else(|e| format!("error: {e}"))
            }
            cmd if cmd.starts_with("setup_select ") => {
                let payload = cmd.strip_prefix("setup_select ").unwrap().trim();
                handle_setup_select(&session, payload).await
            }
            // Any board-protocol command means a frontend connected expecting
            // the live server. Tell it we're still in setup so it renders the
            // setup screen instead of hanging on a board snapshot.
            _ => "{\"setup\":true}".to_string(),
        };

        if writer
            .write_all(format!("{response}\n").as_bytes())
            .await
            .is_err()
        {
            break;
        }
    }
}

/// Activate the selected provider. On success persist the config; if the gate
/// is now satisfied, publish `Ready` and wake the accept loop. Returns a JSON
/// envelope `{ok, message, done}` for the caller.
async fn handle_setup_select(session: &Arc<SetupSession>, payload: &str) -> String {
    let sel: SetupSelect = match serde_json::from_str(payload) {
        Ok(s) => s,
        Err(e) => {
            return format!(
                "{{\"ok\":false,\"message\":\"invalid selection: {e}\",\"done\":false}}"
            );
        }
    };

    let result = {
        let mut config = session.config.lock().await;
        setup::activate(&mut config, &sel.name, sel.account_id, sel.role_name).await
    };

    match result {
        Ok(setup::ActivationOutcome::Activated { message }) => {
            let ready = {
                let config = session.config.lock().await;
                let _ = config.write(std::path::Path::new("."));
                setup::gate_ready(&config)
            };
            if ready {
                session.set_state(SetupState::Ready).await;
                session.ready.notify_one();
                json_ok(&message, true)
            } else {
                json_ok(&message, false)
            }
        }
        Ok(setup::ActivationOutcome::NeedsMoreInput { message }) => json_ok(&message, false),
        Err(e) => {
            let msg = e.to_string();
            // Surface the failure on the state stream too, keeping rescan available.
            session
                .set_state(SetupState::Error {
                    messages: vec![msg.clone()],
                    can_rescan: true,
                })
                .await;
            format!(
                "{{\"ok\":false,\"message\":{},\"done\":false}}",
                serde_json::to_string(&msg).unwrap_or_else(|_| "\"error\"".to_string())
            )
        }
    }
}

fn json_ok(message: &str, done: bool) -> String {
    format!(
        "{{\"ok\":true,\"message\":{},\"done\":{done}}}",
        serde_json::to_string(message).unwrap_or_else(|_| "\"\"".to_string())
    )
}

/// Stream JSON-encoded `SetupState` lines to a subscribed frontend until the
/// connection closes. Emits the current state immediately so a late subscriber
/// is never left blank.
async fn stream_setup_state(
    session: &Arc<SetupSession>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) {
    use tokio::sync::broadcast::error::RecvError;

    let mut rx = session.tx.subscribe();
    // Send current state first.
    {
        let state = session.state.lock().await.clone();
        if let Ok(json) = serde_json::to_string(&state)
            && writer
                .write_all(format!("{json}\n").as_bytes())
                .await
                .is_err()
        {
            return;
        }
    }
    loop {
        match rx.recv().await {
            Ok(state) => {
                let Ok(json) = serde_json::to_string(&state) else {
                    continue;
                };
                if writer
                    .write_all(format!("{json}\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
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
            // The live server is only reached once the bootstrap gate has
            // cleared, so setup is always Ready here. Frontends query this
            // first to decide between the setup screen and the board (ADR-002).
            "setup_state" => serde_json::to_string(&SetupState::Ready)
                .unwrap_or_else(|_| "{\"state\":\"ready\"}".to_string()),
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
            cmd if cmd.starts_with("chat_ctx ") => {
                let encoded = cmd.strip_prefix("chat_ctx ").unwrap().trim();
                let (message, view_prefix) = parse_chat_ctx(encoded);
                let augmented = if view_prefix.is_empty() {
                    message
                } else {
                    format!("{view_prefix}\n\n{message}")
                };
                let d = Arc::clone(&dispatcher);
                tokio::spawn(async move {
                    handle_chat_with_interrupt(d, augmented).await;
                });
                "queued".to_string()
            }
            cmd if cmd.starts_with("chat ") => {
                let encoded = cmd.strip_prefix("chat ").unwrap().trim();
                let message: String =
                    serde_json::from_str(encoded).unwrap_or_else(|_| encoded.to_string());
                // Spawn chat processing with interrupt classification support.
                let d = Arc::clone(&dispatcher);
                tokio::spawn(async move {
                    handle_chat_with_interrupt(d, message).await;
                });
                "queued".to_string()
            }
            "chat_history" => handle_chat_history(&dispatcher).await,
            "close_fork" => handle_close_fork(&dispatcher).await,
            cmd if cmd.starts_with("update_task ") => {
                handle_update_task(cmd.strip_prefix("update_task ").unwrap().trim())
            }
            cmd if cmd.starts_with("update_epic ") => {
                handle_update_epic(cmd.strip_prefix("update_epic ").unwrap().trim())
            }
            other => format!("error: unknown command '{other}'"),
        };
        if writer
            .write_all(format!("{response}\n").as_bytes())
            .await
            .is_err()
        {
            break;
        }
    }
}

/// Stream JSON-encoded DispatchEvent lines until the connection closes.
async fn handle_subscribe(dispatcher: &Dispatcher, writer: &mut tokio::net::unix::OwnedWriteHalf) {
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
                if writer
                    .write_all(format!("{json}\n").as_bytes())
                    .await
                    .is_err()
                {
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
    use serde_json::{Value, json};

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
                .map(|m| {
                    format!(
                        "{}: {}",
                        match m.role {
                            crate::state::chat::Role::User => "user",
                            crate::state::chat::Role::Agent => "agent",
                            crate::state::chat::Role::System => "system",
                        },
                        &m.content
                    )
                })
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        };

        let base_url = dispatcher
            .main_agent
            .base_url()
            .unwrap_or_else(|| "http://localhost:1234/v1".to_string());
        let model = dispatcher.main_agent.model();

        let action = classifier::classify(
            &base_url,
            &model,
            &context_turns,
            &pending_message,
            &message,
        )
        .await;

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
                start_chat_turn(dispatcher, message, Some((fork_id, fork_name))).await;
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
    use crate::dispatcher::{ChatContext, ForkEntry, event_bus::DispatchEvent};

    let (fork_id, fork_name) =
        fork_info.unwrap_or_else(|| ("main".to_string(), "Main".to_string()));

    // Seed fork_history: clone the last 50 entries from the dispatcher's chat history
    // when this is a named fork; empty for "main" entries.
    let fork_history = if fork_id != "main" {
        let hist = dispatcher.chat_history.lock().await;
        let start = hist.len().saturating_sub(50);
        hist[start..].to_vec()
    } else {
        Vec::new()
    };

    let ctx = if fork_id != "main" {
        Some(ChatContext {
            history: fork_history.clone(),
            persist_path: crate::state::home::fork_chat_file(&fork_id),
            fork_id: fork_id.clone(),
        })
    } else {
        None
    };

    let d = Arc::clone(&dispatcher);
    let msg = message.clone();
    let handle = tokio::spawn(async move { d.chat(&msg, ctx).await });

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

    dispatcher
        .event_bus
        .emit(DispatchEvent::ChatResponse { content: result });
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
            dispatcher
                .event_bus
                .emit(crate::dispatcher::event_bus::DispatchEvent::ForkClosed {
                    id: id.clone(),
                    summary: "Closed by user".to_string(),
                });
            format!("ok: fork {id} closed")
        }
        None => "noop: not in a fork".to_string(),
    }
}

/// Return the full chat history as a JSON array.
async fn handle_chat_history(dispatcher: &Dispatcher) -> String {
    let history = dispatcher.chat_history.lock().await;
    serde_json::to_string(&*history).unwrap_or_else(|_| "[]".to_string())
}

/// Handle `update_task <id> <json>` -- update a task's title/description on disk.
fn handle_update_task(args: &str) -> String {
    let (id, json) = args.split_once(' ').unwrap_or((args, "{}"));
    let updates = match serde_json::from_str::<serde_json::Value>(json) {
        Ok(v) => v,
        Err(_) => return "error: invalid json".to_string(),
    };
    let title = updates
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let desc = updates
        .get("description")
        .and_then(|v| if v.is_null() { None } else { v.as_str() })
        .map(str::to_string);
    match crate::state::reader::load_task(id) {
        Ok(mut task) => {
            if let Some(t) = title {
                task.title = t;
            }
            if let Some(d) = desc {
                task.description = Some(d);
            }
            match crate::state::writer::write_task(&task) {
                Ok(()) => "ok".to_string(),
                Err(e) => format!("error: {e}"),
            }
        }
        Err(e) => format!("error: {e}"),
    }
}

/// Handle `update_epic <id> <json>` -- update an epic's title/description on disk.
fn handle_update_epic(args: &str) -> String {
    let (id, json) = args.split_once(' ').unwrap_or((args, "{}"));
    let updates = match serde_json::from_str::<serde_json::Value>(json) {
        Ok(v) => v,
        Err(_) => return "error: invalid json".to_string(),
    };
    let title = updates
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let desc = updates
        .get("description")
        .and_then(|v| if v.is_null() { None } else { v.as_str() })
        .map(str::to_string);
    match crate::state::reader::load_epic(id) {
        Ok(mut epic) => {
            if let Some(t) = title {
                epic.title = t;
            }
            if let Some(d) = desc {
                epic.description = Some(d);
            }
            match crate::state::writer::write_epic(&epic) {
                Ok(()) => "ok".to_string(),
                Err(e) => format!("error: {e}"),
            }
        }
        Err(e) => format!("error: {e}"),
    }
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
        // Grab the journal logs so the user sees the actual failure reason
        let logs = std::process::Command::new("journalctl")
            .args([
                "--user",
                "-u",
                "gitzi.service",
                "--no-pager",
                "-n",
                "20",
                "-o",
                "short",
            ])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default();
        anyhow::bail!("Failed to start gitzi daemon.\n\n{logs}")
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

    info!(
        "systemd user service installed at {}",
        service_path.display()
    );
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

// ─── View context parsing ─────────────────────────────────────────────────────

/// Parse a `chat_ctx` payload into (message, view_context_prefix).
/// The prefix is a short system-injected line that tells the main agent what the
/// user is currently looking at. If parsing fails, returns the raw string as
/// the message with no prefix.
fn parse_chat_ctx(encoded: &str) -> (String, String) {
    #[derive(serde::Deserialize)]
    struct ChatCtxPayload {
        message: String,
        #[serde(default)]
        view_context: Option<ViewCtx>,
    }

    #[derive(serde::Deserialize)]
    struct ViewCtx {
        panel: Option<String>,
        selected_task_id: Option<String>,
        selected_task_title: Option<String>,
        selected_column: Option<String>,
        #[serde(default)]
        pending_questions: usize,
    }

    let payload: ChatCtxPayload = match serde_json::from_str(encoded) {
        Ok(p) => p,
        Err(_) => {
            // Fallback: treat the whole thing as a plain message
            let message: String =
                serde_json::from_str(encoded).unwrap_or_else(|_| encoded.to_string());
            return (message, String::new());
        }
    };

    let prefix = match payload.view_context {
        Some(ctx) => {
            let mut parts = Vec::new();
            if let Some(ref panel) = ctx.panel {
                parts.push(format!("panel={panel}"));
            }
            if let Some(ref col) = ctx.selected_column {
                parts.push(format!("column={col}"));
            }
            if let Some(ref title) = ctx.selected_task_title
                && let Some(ref id) = ctx.selected_task_id
            {
                parts.push(format!("selected_task=\"{title}\" ({id})"));
            }
            if ctx.pending_questions > 0 {
                parts.push(format!("pending_questions={}", ctx.pending_questions));
            }
            if parts.is_empty() {
                String::new()
            } else {
                format!("[view: {}]", parts.join(", "))
            }
        }
        None => String::new(),
    };

    (payload.message, prefix)
}
