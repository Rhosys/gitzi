use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Backlog,
    Prioritized,
    InProgress,
    WaitingForReview,
    InTesting,
    Done,
}

impl std::fmt::Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stage::Backlog => write!(f, "backlog"),
            Stage::Prioritized => write!(f, "prioritized"),
            Stage::InProgress => write!(f, "in-progress"),
            Stage::WaitingForReview => write!(f, "waiting-for-review"),
            Stage::InTesting => write!(f, "in-testing"),
            Stage::Done => write!(f, "done"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageTransition {
    pub from: Stage,
    pub to: Stage,
    pub at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
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
    pub history: Vec<StageTransition>,
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
        self.history.push(StageTransition {
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
