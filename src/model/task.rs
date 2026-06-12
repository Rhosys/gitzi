use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::dispatcher::Column;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    // Legacy stages (backward compat with old pipeline)
    Backlog,
    InProgress,
    WaitingForReview,
    InTesting,
    // Column-aligned stages (new dispatcher)
    Prioritized,
    Designing,
    CodingBuffer,
    Coding,
    ReviewBuffer,
    Reviewing,
    TestBuffer,
    Testing,
    SecurityAuditBuffer,
    Auditing,
    DeploymentBuffer,
    Deploying,
    Done,
}

impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stage::Backlog => write!(f, "backlog"),
            Stage::InProgress => write!(f, "in-progress"),
            Stage::WaitingForReview => write!(f, "waiting-for-review"),
            Stage::InTesting => write!(f, "in-testing"),
            Stage::Prioritized => write!(f, "prioritized"),
            Stage::Designing => write!(f, "designing"),
            Stage::CodingBuffer => write!(f, "coding-buffer"),
            Stage::Coding => write!(f, "coding"),
            Stage::ReviewBuffer => write!(f, "review-buffer"),
            Stage::Reviewing => write!(f, "reviewing"),
            Stage::TestBuffer => write!(f, "test-buffer"),
            Stage::Testing => write!(f, "testing"),
            Stage::SecurityAuditBuffer => write!(f, "security-audit-buffer"),
            Stage::Auditing => write!(f, "auditing"),
            Stage::DeploymentBuffer => write!(f, "deployment-buffer"),
            Stage::Deploying => write!(f, "deploying"),
            Stage::Done => write!(f, "done"),
        }
    }
}

impl From<Column> for Stage {
    fn from(col: Column) -> Self {
        match col {
            Column::Prioritized => Stage::Prioritized,
            Column::Designing => Stage::Designing,
            Column::CodingBuffer => Stage::CodingBuffer,
            Column::Coding => Stage::Coding,
            Column::ReviewBuffer => Stage::ReviewBuffer,
            Column::Reviewing => Stage::Reviewing,
            Column::TestBuffer => Stage::TestBuffer,
            Column::Testing => Stage::Testing,
            Column::SecurityAuditBuffer => Stage::SecurityAuditBuffer,
            Column::Auditing => Stage::Auditing,
            Column::DeploymentBuffer => Stage::DeploymentBuffer,
            Column::Deploying => Stage::Deploying,
            Column::Done => Stage::Done,
        }
    }
}

impl Stage {
    /// Convert to a Column. Legacy stages map to their closest equivalent.
    pub fn to_column(&self) -> Column {
        match self {
            Stage::Backlog | Stage::Prioritized => Column::Prioritized,
            Stage::InProgress | Stage::Coding => Column::Coding,
            Stage::WaitingForReview | Stage::ReviewBuffer => Column::ReviewBuffer,
            Stage::InTesting | Stage::Testing => Column::Testing,
            Stage::Designing => Column::Designing,
            Stage::CodingBuffer => Column::CodingBuffer,
            Stage::Reviewing => Column::Reviewing,
            Stage::TestBuffer => Column::TestBuffer,
            Stage::SecurityAuditBuffer => Column::SecurityAuditBuffer,
            Stage::Auditing => Column::Auditing,
            Stage::DeploymentBuffer => Column::DeploymentBuffer,
            Stage::Deploying => Column::Deploying,
            Stage::Done => Column::Done,
        }
    }
}

/// A single entry in a task's history log. Serialized as `[[history]]` in TOML
/// with a `kind` discriminator field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistoryEntry {
    /// A stage transition (agent completed, auto-advance, etc.)
    StageChange {
        from: Stage,
        to: Stage,
        at: DateTime<Utc>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Human approved a task in a buffer column.
    Approval {
        at: DateTime<Utc>,
        target_stage: Stage,
    },
    /// Human rejected a task in a buffer column.
    Rejection {
        at: DateTime<Utc>,
        feedback: String,
        returned_to: Stage,
    },
}

/// Legacy struct kept for backward compatibility with code that constructs transitions directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageTransition {
    pub from: Stage,
    pub to: Stage,
    pub at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl From<StageTransition> for HistoryEntry {
    fn from(t: StageTransition) -> Self {
        HistoryEntry::StageChange {
            from: t.from,
            to: t.to,
            at: t.at,
            note: t.note,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub epic: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub stage: Stage,
    #[serde(default = "default_priority")]
    pub priority: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_feedback: Option<String>,
    #[serde(default)]
    pub wip_limit_blocked: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
}

fn default_priority() -> u32 {
    100
}

impl Task {
    pub fn new(id: impl Into<String>, epic: impl Into<String>, title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            epic: epic.into(),
            title: title.into(),
            description: None,
            stage: Stage::Backlog,
            priority: default_priority(),
            agent: None,
            branch: None,
            agent_feedback: None,
            wip_limit_blocked: false,
            created_at: now,
            updated_at: now,
            history: Vec::new(),
        }
    }

    pub fn branch_name(&self) -> String {
        let slug = slug(&self.title);
        format!("gitzi/{}-{}", self.id, slug)
    }

    pub fn transition_to(&mut self, to: Stage, note: Option<String>) {
        let from = self.stage.clone();
        self.history.push(HistoryEntry::StageChange {
            from,
            to: to.clone(),
            at: Utc::now(),
            note,
        });
        self.stage = to;
        self.updated_at = Utc::now();
    }
}

pub fn slug(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(40)
        .collect()
}

pub fn new_id() -> String {
    Uuid::new_v4().to_string()[..8].to_string()
}
