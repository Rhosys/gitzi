use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::AgentRole;
use super::Column;

// ─── DispatchEvent ────────────────────────────────────────────────────────────

/// Typed events emitted on the bus whenever system state changes.
/// All components subscribe and react — no polling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DispatchEvent {
    TaskCreated {
        task_id: String,
    },
    TaskStageChanged {
        task_id: String,
        from: Column,
        to: Column,
    },
    HumanApprovalReceived {
        task_id: String,
        target_column: Column,
    },
    HumanRejectionReceived {
        task_id: String,
        returned_to: Column,
        feedback: String,
    },
    AgentCompleted {
        task_id: String,
        agent_role: AgentRole,
    },
    AgentBlocked {
        task_id: String,
        agent_role: AgentRole,
        question: String,
    },
    BootComplete,
    PanelSwitch {
        view: String,
    },
    /// Chat response from the main agent (delivered async via event bus).
    ChatResponse {
        content: String,
    },
    /// A new fork was created (for TUI fork stack display).
    ForkCreated {
        id: String,
        name: String,
    },
    /// A fork was closed (popped from stack).
    ForkClosed {
        id: String,
        summary: String,
    },
    /// A log entry relayed from tracing (errors only).
    LogEntry {
        level: String,
        message: String,
    },
    /// Streaming: a chunk of tokens arrived from the LLM.
    StreamingTokens {
        count: usize,
    },
    /// Streaming: a tool call was initiated during the response.
    StreamingToolCall {
        name: String,
    },
    /// Streaming: the response is complete (progress bar should reset).
    StreamingDone,
}

// ─── EventBus ─────────────────────────────────────────────────────────────────

/// Single broadcast channel carrying all dispatch events.
/// Multiple subscribers (TUI, logging, agents) without blocking the sender.
pub struct EventBus {
    tx: broadcast::Sender<DispatchEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Emit an event to all subscribers.
    pub fn emit(&self, event: DispatchEvent) {
        // Ignore send errors (no active receivers is fine).
        let _ = self.tx.send(event);
    }

    /// Create a new subscriber that receives future events.
    pub fn subscribe(&self) -> broadcast::Receiver<DispatchEvent> {
        self.tx.subscribe()
    }
}
