use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tracing::warn;

use crate::daemon::socket_path;
use crate::setup::SetupState;
use crate::state::chat::{ChatMessage as StoredMessage, Role};
use super::app::{BoardColumn, ChatEntry, DaemonCommand, DaemonMessage, EditorTarget};

/// Spawn the background task that manages the daemon socket connection.
/// Returns an UnboundedReceiver for incoming messages.
pub fn spawn(
    cmd_rx: mpsc::UnboundedReceiver<DaemonCommand>,
) -> mpsc::UnboundedReceiver<DaemonMessage> {
    let (msg_tx, msg_rx) = mpsc::unbounded_channel();
    tokio::spawn(run_client(cmd_rx, msg_tx));
    msg_rx
}

/// Which phase the daemon is in, probed via the `setup_state` command. The
/// daemon serves a bootstrap *setup* phase on the socket before the dispatcher
/// exists, then the live *board* server once the gate clears (ADR-002).
enum Phase {
    Setup,
    Board,
    Unreachable,
}

/// Outer reconnect loop. Probes the daemon's phase and runs the matching
/// sub-protocol, reconnecting across the setup→board handoff (the setup server
/// drops its listener when the gate clears, briefly making the socket
/// unreachable before the live server rebinds).
async fn run_client(
    mut cmd_rx: mpsc::UnboundedReceiver<DaemonCommand>,
    msg_tx: mpsc::UnboundedSender<DaemonMessage>,
) {
    let path = socket_path();
    loop {
        match probe_phase(&path).await {
            Phase::Unreachable => {
                let _ = msg_tx.send(DaemonMessage::Disconnected("connecting…".to_string()));
                sleep(Duration::from_millis(300)).await;
            }
            Phase::Setup => {
                let _ = msg_tx.send(DaemonMessage::Connected);
                run_setup_phase(&path, &mut cmd_rx, &msg_tx).await;
                // Returns when Ready is observed or the connection drops — either
                // way, loop back to re-probe (the live server may now be up).
            }
            Phase::Board => {
                let _ = msg_tx.send(DaemonMessage::Connected);
                run_board_phase(&path, &mut cmd_rx, &msg_tx).await;
                let _ = msg_tx.send(DaemonMessage::Disconnected("reconnecting…".to_string()));
                // Brief backoff so a fast-failing board protocol can't hot-spin.
                sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

/// Connect and ask `setup_state` to learn which phase the daemon is in.
async fn probe_phase(path: &std::path::Path) -> Phase {
    let stream = match UnixStream::connect(path).await {
        Ok(s) => s,
        Err(_) => return Phase::Unreachable,
    };
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    if writer.write_all(b"setup_state\n").await.is_err() {
        return Phase::Unreachable;
    }
    match lines.next_line().await {
        Ok(Some(line)) => match serde_json::from_str::<SetupState>(&line) {
            Ok(SetupState::Ready) => Phase::Board,
            Ok(_) => Phase::Setup,
            // Unknown/legacy response — treat as the live board server.
            Err(_) => Phase::Board,
        },
        _ => Phase::Unreachable,
    }
}

// ── Setup phase (ADR-002) ─────────────────────────────────────────────────────

/// Render-and-relay loop for the bootstrap setup phase: subscribe to streamed
/// `SetupState`, and relay the user's provider selection / rescan back. Returns
/// once `Ready` is observed (so the outer loop reconnects to the board server)
/// or the connection drops.
async fn run_setup_phase(
    path: &std::path::Path,
    cmd_rx: &mut mpsc::UnboundedReceiver<DaemonCommand>,
    msg_tx: &mpsc::UnboundedSender<DaemonMessage>,
) {
    // Command connection.
    let cmd_stream = match UnixStream::connect(path).await {
        Ok(s) => s,
        Err(_) => return,
    };
    let (cmd_reader, mut cmd_writer) = cmd_stream.into_split();
    let mut cmd_lines = BufReader::new(cmd_reader).lines();

    // Subscription connection — streams SetupState (current state first).
    let sub_stream = match UnixStream::connect(path).await {
        Ok(s) => s,
        Err(_) => return,
    };
    let (sub_reader, mut sub_writer) = sub_stream.into_split();
    if sub_writer.write_all(b"subscribe\n").await.is_err() {
        return;
    }
    let mut sub_lines = BufReader::new(sub_reader).lines();

    loop {
        tokio::select! {
            sub = sub_lines.next_line() => {
                match sub {
                    Ok(Some(line)) => {
                        if let Ok(state) = serde_json::from_str::<SetupState>(&line) {
                            let ready = matches!(state, SetupState::Ready);
                            let _ = msg_tx.send(DaemonMessage::SetupState(state));
                            if ready {
                                return;
                            }
                        }
                    }
                    _ => return,
                }
            }
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return };
                match cmd {
                    DaemonCommand::SetupSelect { name } => {
                        let payload = serde_json::json!({
                            "name": name,
                            "account_id": serde_json::Value::Null,
                            "role_name": serde_json::Value::Null,
                        });
                        let line = format!("setup_select {payload}\n");
                        if cmd_writer.write_all(line.as_bytes()).await.is_err() {
                            return;
                        }
                        if let Ok(Some(resp)) = cmd_lines.next_line().await
                            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&resp)
                            && let Some(msg) = v.get("message").and_then(|m| m.as_str())
                        {
                            let _ = msg_tx.send(DaemonMessage::SetupMessage(msg.to_string()));
                        }
                    }
                    DaemonCommand::SetupRescan => {
                        if cmd_writer.write_all(b"setup_rescan\n").await.is_err() {
                            return;
                        }
                        // Ack is the current state JSON; the subscribe stream
                        // delivers the post-scan state, so just drain it.
                        let _ = cmd_lines.next_line().await;
                    }
                    // Board commands are meaningless during setup — drop them.
                    _ => {}
                }
            }
        }
    }
}

// ── Board phase ───────────────────────────────────────────────────────────────

async fn run_board_phase(
    path: &std::path::Path,
    cmd_rx: &mut mpsc::UnboundedReceiver<DaemonCommand>,
    msg_tx: &mpsc::UnboundedSender<DaemonMessage>,
) {
    // Tell the frontend setup is behind us so it renders the board, not the
    // splash, even if it never saw a Ready over a setup subscription.
    let _ = msg_tx.send(DaemonMessage::SetupState(SetupState::Ready));

    let stream = match UnixStream::connect(path).await {
        Ok(s) => s,
        Err(e) => {
            let _ = msg_tx.send(DaemonMessage::Disconnected(format!("connect failed: {e}")));
            return;
        }
    };

    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    // Send initial board request
    if writer.write_all(b"board\n").await.is_err() {
        let _ = msg_tx.send(DaemonMessage::Disconnected("write failed".to_string()));
        return;
    }

    // Read board snapshot response
    if let Ok(Some(line)) = lines.next_line().await
        && let Ok(columns) = serde_json::from_str::<Vec<BoardColumn>>(&line) {
        let _ = msg_tx.send(DaemonMessage::BoardSnapshot(columns));
    }

    // Now open a second connection for subscription (subscribe holds the connection)
    let sub_stream = match UnixStream::connect(path).await {
        Ok(s) => s,
        Err(e) => {
            warn!("subscribe connect failed: {e}");
            let _ = msg_tx.send(DaemonMessage::Disconnected(format!("subscribe failed: {e}")));
            return;
        }
    };

    let (sub_reader, mut sub_writer) = sub_stream.into_split();
    if sub_writer.write_all(b"subscribe\n").await.is_err() {
        let _ = msg_tx.send(DaemonMessage::Disconnected("subscribe write failed".to_string()));
        return;
    }
    let mut sub_lines = BufReader::new(sub_reader).lines();

    // Also get initial peek_review via the command connection
    if writer.write_all(b"peek_review\n").await.is_err() {
        let _ = msg_tx.send(DaemonMessage::Disconnected("write failed".to_string()));
        return;
    }
    if let Ok(Some(line)) = lines.next_line().await {
        let pending = line.trim() != "null";
        let _ = msg_tx.send(DaemonMessage::ReviewPending(pending));
    }

    // Initial epics list (for the Status panel's "Current epic" section)
    if writer.write_all(b"epics\n").await.is_err() {
        let _ = msg_tx.send(DaemonMessage::Disconnected("write failed".to_string()));
        return;
    }
    if let Ok(Some(line)) = lines.next_line().await
        && let Ok(epics) = serde_json::from_str::<Vec<crate::model::Epic>>(&line) {
        let _ = msg_tx.send(DaemonMessage::Epics(epics));
    }

    // Initial clarification-queue count (for the Status panel)
    if writer.write_all(b"queue_len\n").await.is_err() {
        let _ = msg_tx.send(DaemonMessage::Disconnected("write failed".to_string()));
        return;
    }
    if let Ok(Some(line)) = lines.next_line().await
        && let Ok(count) = line.trim().parse::<usize>() {
        let _ = msg_tx.send(DaemonMessage::QueueLen(count));
    }

    // Load chat history
    if writer.write_all(b"chat_history\n").await.is_err() {
        let _ = msg_tx.send(DaemonMessage::Disconnected("write failed".to_string()));
        return;
    }
    if let Ok(Some(line)) = lines.next_line().await {
        let stored: Vec<StoredMessage> = serde_json::from_str(&line).unwrap_or_default();
        let entries = stored_to_entries(stored);
        let _ = msg_tx.send(DaemonMessage::ChatHistory(entries));
    }

    // Main loop: select between subscription events and outgoing commands
    loop {
        tokio::select! {
            // Incoming events from subscription stream
            sub_result = sub_lines.next_line() => {
                match sub_result {
                    Ok(Some(line)) => {
                        // Check for panel_switch events specifically
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line)
                            && val.get("type").and_then(|t| t.as_str()) == Some("panel_switch")
                            && let Some(view) = val.get("view").and_then(|v| v.as_str())
                        {
                            let _ = msg_tx.send(DaemonMessage::SwitchPanel(view.to_string()));
                            continue;
                        }
                        // Check for chat_response events
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line)
                            && val.get("type").and_then(|t| t.as_str()) == Some("chat_response")
                            && let Some(content) = val.get("content").and_then(|v| v.as_str())
                        {
                            let _ = msg_tx.send(DaemonMessage::ChatResponse(content.to_string()));
                            continue;
                        }
                        // Check for fork_created events
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line)
                            && val.get("type").and_then(|t| t.as_str()) == Some("fork_created")
                            && let Some(id) = val.get("id").and_then(|v| v.as_str())
                            && let Some(name) = val.get("name").and_then(|v| v.as_str())
                        {
                            let _ = msg_tx.send(DaemonMessage::ForkCreated {
                                id: id.to_string(),
                                name: name.to_string(),
                            });
                            continue;
                        }
                        // Check for fork_closed events
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line)
                            && val.get("type").and_then(|t| t.as_str()) == Some("fork_closed")
                            && let Some(id) = val.get("id").and_then(|v| v.as_str())
                        {
                            let _ = msg_tx.send(DaemonMessage::ForkClosed {
                                id: id.to_string(),
                            });
                            continue;
                        }
                        // Check for log_entry events (error relay)
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line)
                            && val.get("type").and_then(|t| t.as_str()) == Some("log_entry")
                            && let Some(message) = val.get("message").and_then(|v| v.as_str())
                        {
                            let level = val.get("level")
                                .and_then(|v| v.as_str())
                                .unwrap_or("ERROR");
                            let _ = msg_tx.send(DaemonMessage::Event(
                                format!("[{level}] {message}")
                            ));
                            continue;
                        }
                        let _ = msg_tx.send(DaemonMessage::Event(line));
                    }
                    Ok(None) | Err(_) => {
                        let _ = msg_tx.send(DaemonMessage::Disconnected("subscription closed".to_string()));
                        return;
                    }
                }
            }

            // Outgoing commands from the TUI
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return };
                match cmd {
                    DaemonCommand::Chat { message, view_context } => {
                        let payload = serde_json::json!({
                            "message": message,
                            "view_context": view_context,
                        });
                        let encoded = serde_json::to_string(&payload).unwrap_or_default();
                        let cmd = format!("chat_ctx {encoded}\n");
                        if writer.write_all(cmd.as_bytes()).await.is_err() {
                            let _ = msg_tx.send(DaemonMessage::Disconnected("write failed".to_string()));
                            return;
                        }
                        // Read the "queued" acknowledgment — discard it.
                        // The actual response arrives via the subscribe stream as a ChatResponse event.
                        let _ = lines.next_line().await;
                    }
                    DaemonCommand::RefreshBoard => {
                        if writer.write_all(b"board\n").await.is_err() {
                            let _ = msg_tx.send(DaemonMessage::Disconnected("write failed".to_string()));
                            return;
                        }
                        if let Ok(Some(line)) = lines.next_line().await
                            && let Ok(columns) = serde_json::from_str::<Vec<BoardColumn>>(&line) {
                            let _ = msg_tx.send(DaemonMessage::BoardSnapshot(columns));
                        }
                    }
                    DaemonCommand::RefreshReview => {
                        if writer.write_all(b"peek_review\n").await.is_err() {
                            let _ = msg_tx.send(DaemonMessage::Disconnected("write failed".to_string()));
                            return;
                        }
                        if let Ok(Some(line)) = lines.next_line().await {
                            let pending = line.trim() != "null";
                            let _ = msg_tx.send(DaemonMessage::ReviewPending(pending));
                        }
                    }
                    DaemonCommand::RefreshEpics => {
                        if writer.write_all(b"epics\n").await.is_err() {
                            let _ = msg_tx.send(DaemonMessage::Disconnected("write failed".to_string()));
                            return;
                        }
                        if let Ok(Some(line)) = lines.next_line().await
                            && let Ok(epics) = serde_json::from_str::<Vec<crate::model::Epic>>(&line) {
                            let _ = msg_tx.send(DaemonMessage::Epics(epics));
                        }
                    }
                    DaemonCommand::RefreshQueueLen => {
                        if writer.write_all(b"queue_len\n").await.is_err() {
                            let _ = msg_tx.send(DaemonMessage::Disconnected("write failed".to_string()));
                            return;
                        }
                        if let Ok(Some(line)) = lines.next_line().await
                            && let Ok(count) = line.trim().parse::<usize>() {
                            let _ = msg_tx.send(DaemonMessage::QueueLen(count));
                        }
                    }
                    DaemonCommand::CloseFork => {
                        let _ = writer.write_all(b"close_fork\n").await;
                    }
                    DaemonCommand::UpdateEntity { id, target, title, description } => {
                        let json = serde_json::json!({
                            "title": title,
                            "description": description,
                        });
                        let cmd = match target {
                            EditorTarget::Epic => {
                                format!("update_epic {} {}\n", id, json)
                            }
                            EditorTarget::Task => {
                                format!("update_task {} {}\n", id, json)
                            }
                        };
                        if writer.write_all(cmd.as_bytes()).await.is_err() {
                            let _ = msg_tx.send(
                                DaemonMessage::Disconnected("write failed".to_string()),
                            );
                            return;
                        }
                        // Read response but don't block the loop on failure
                        if let Ok(Some(_resp)) = lines.next_line().await {}
                    }
                    // Setup commands are meaningless once on the board — drop them.
                    DaemonCommand::SetupSelect { .. } | DaemonCommand::SetupRescan => {}
                }
            }
        }
    }
}

fn stored_to_entries(stored: Vec<StoredMessage>) -> Vec<ChatEntry> {
    stored
        .into_iter()
        .filter_map(|m| match m.role {
            Role::User => Some(ChatEntry { is_user: true, content: m.content }),
            Role::Agent => Some(ChatEntry { is_user: false, content: m.content }),
            Role::System => None,
        })
        .collect()
}
