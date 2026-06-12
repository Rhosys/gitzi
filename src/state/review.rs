use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::dispatcher::Column;

/// An action taken on a review item (approval, rejection, or answer to a question).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReviewAction {
    Approval { at: DateTime<Utc> },
    Rejection { at: DateTime<Utc>, feedback: String },
    Answer { at: DateTime<Utc>, content: String },
}

/// The kind of review item — either an agent question or a buffer approval gate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PersistedReviewKind {
    AgentQuestion { question: String },
    BufferApproval { buffer_column: Column, task_priority: u32 },
}

/// A persisted review item stored as TOML in `~/.gitzi/<session>/reviews/{id}.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedReviewItem {
    pub id: String,
    pub task_id: String,
    pub kind: PersistedReviewKind,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub actions: Vec<ReviewAction>,
}

impl PersistedReviewItem {
    /// True if no terminal action (approval/rejection/answer) has been recorded.
    pub fn is_unresolved(&self) -> bool {
        self.actions.is_empty()
    }
}
