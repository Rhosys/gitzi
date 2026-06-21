use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tracing::warn;

use crate::daemon::socket_path;
use crate::state::chat::{ChatMessage as StoredMessage, Role};
use super::app::{BoardColumn, ChatEntry, DaemonCommand, DaemonMessage};

/// Spawn the background task that manages the daemon socket connection.
/// Returns an UnboundedReceiver for incoming messages.
pub fn spawn(
    cmd_rx: mpsc::UnboundedReceiver<DaemonCommand>,
) -> mpsc::UnboundedReceiver<DaemonMessage> {
    let (msg_tx, msg_rx) = mpsc::unbounded_channel();
    tokio::spawn(run_client(cmd_rx, msg_tx));
    msg_rx
}

async fn run_client(
    mut cmd_rx: mpsc::UnboundedReceiver<DaemonCommand>,
    msg_tx: mpsc::UnboundedSender<DaemonMessage>,
) {
    // Connect to daemon
    let path = socket_path();
    let stream = match UnixStream::connect(&path).await {
        Ok(s) => s,
        Err(e) => {
            let _ = msg_tx.send(DaemonMessage::Disconnected(format!("connect failed: {e}")));
            return;
        }
    };

    let _ = msg_tx.send(DaemonMessage::Connected);

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
    let sub_stream = match UnixStream::connect(&path).await {
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
                    DaemonCommand::Chat(message) => {
                        let encoded = serde_json::to_string(&message).unwrap_or_default();
                        let cmd = format!("chat {encoded}\n");
                        if writer.write_all(cmd.as_bytes()).await.is_err() {
                            let _ = msg_tx.send(DaemonMessage::Disconnected("write failed".to_string()));
                            return;
                        }
                        if let Ok(Some(line)) = lines.next_line().await {
                            let response: String = serde_json::from_str(&line)
                                .unwrap_or_else(|_| line.clone());
                            let _ = msg_tx.send(DaemonMessage::ChatResponse(response));
                        }
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
