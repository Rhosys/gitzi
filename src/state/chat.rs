use std::path::Path;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::error::{GitziError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    System,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    pub ts: DateTime<Utc>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: Role::User, content: content.into(), ts: Utc::now() }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self { role: Role::System, content: content.into(), ts: Utc::now() }
    }

    pub fn agent(content: impl Into<String>) -> Self {
        Self { role: Role::Agent, content: content.into(), ts: Utc::now() }
    }
}

/// Append a single message to the chat log (creates file and parent dirs if needed).
pub fn append(path: &Path, msg: &ChatMessage) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(msg)
        .map_err(|e| GitziError::AgentFailed(e.to_string()))?;
    writeln!(f, "{line}")?;
    Ok(())
}

/// Load the full chat history in order.
pub fn load(path: &Path) -> Result<Vec<ChatMessage>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(path)?;
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l).map_err(|e| GitziError::AgentFailed(e.to_string()))
        })
        .collect()
}
