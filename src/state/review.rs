use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::atomic_write;
use crate::dispatcher::Column;
use crate::error::Result;
use crate::state::home::session_dir;

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

// ── Persistence functions ─────────────────────────────────────────────────────

/// Directory for review item files: `~/.gitzi/<session>/reviews/`
pub fn reviews_dir() -> Result<PathBuf> {
    let dir = session_dir()?.join("reviews");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Persist a review item to disk atomically.
pub fn write_review_item(item: &PersistedReviewItem) -> Result<()> {
    let path = reviews_dir()?.join(format!("{}.toml", item.id));
    atomic_write(&path, &toml::to_string_pretty(item)?)
}

/// Load a single review item by ID.
pub fn load_review_item(id: &str) -> Result<PersistedReviewItem> {
    let path = reviews_dir()?.join(format!("{id}.toml"));
    let text = std::fs::read_to_string(&path)?;
    Ok(toml::from_str(&text)?)
}

/// Load all review items that have no terminal action recorded.
pub fn load_all_unresolved() -> Result<Vec<PersistedReviewItem>> {
    let dir = session_dir()?.join("reviews");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            let text = std::fs::read_to_string(&path)?;
            let item: PersistedReviewItem = toml::from_str(&text)?;
            if item.is_unresolved() {
                items.push(item);
            }
        }
    }
    Ok(items)
}
