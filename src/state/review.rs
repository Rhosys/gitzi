use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::atomic_write;
use crate::dispatcher::Column;
use crate::error::Result;
use crate::state::chat::Role;
use crate::state::home;

/// An action taken on a review item: a terminal decision (approval, rejection,
/// answer) or a non-terminal conversational turn (comment) while the human and
/// main agent discuss what to do.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReviewAction {
    Approval {
        at: DateTime<Utc>,
    },
    Rejection {
        at: DateTime<Utc>,
        feedback: String,
    },
    Answer {
        at: DateTime<Utc>,
        content: String,
    },
    /// A single turn in the rework/explain conversation about this item.
    /// Does not resolve the item — `is_unresolved` ignores these.
    Comment {
        at: DateTime<Utc>,
        role: Role,
        content: String,
    },
}

/// The kind of review item — either an agent question or a buffer approval gate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PersistedReviewKind {
    AgentQuestion {
        question: String,
    },
    BufferApproval {
        buffer_column: Column,
        task_priority: u32,
    },
}

/// A persisted review item stored as TOML in `~/.gitzi/plan/reviews/{id}.toml`.
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
    /// `Comment` turns don't count — discussion can continue indefinitely
    /// without resolving the item.
    pub fn is_unresolved(&self) -> bool {
        !self.actions.iter().any(|a| {
            matches!(
                a,
                ReviewAction::Approval { .. }
                    | ReviewAction::Rejection { .. }
                    | ReviewAction::Answer { .. }
            )
        })
    }
}

// ── Persistence functions ─────────────────────────────────────────────────────

/// Directory for review item files: `~/.gitzi/plan/reviews/`
pub fn reviews_dir() -> PathBuf {
    let dir = home::reviews_dir();
    // Best-effort create; callers handle errors on actual I/O.
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Persist a review item to disk atomically.
pub fn write_review_item(item: &PersistedReviewItem) -> Result<()> {
    let path = reviews_dir().join(format!("{}.toml", item.id));
    atomic_write(&path, &toml::to_string_pretty(item)?)
}

/// Load a single review item by ID.
pub fn load_review_item(id: &str) -> Result<PersistedReviewItem> {
    let path = reviews_dir().join(format!("{id}.toml"));
    let text = std::fs::read_to_string(&path)?;
    Ok(toml::from_str(&text)?)
}

/// Find an unresolved review item for the given task ID.
pub fn find_unresolved_for_task(task_id: &str) -> Result<Option<PersistedReviewItem>> {
    let dir = reviews_dir();
    if !dir.exists() {
        return Ok(None);
    }
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            let text = std::fs::read_to_string(&path)?;
            let item: PersistedReviewItem = toml::from_str(&text)?;
            if item.task_id == task_id && item.is_unresolved() {
                return Ok(Some(item));
            }
        }
    }
    Ok(None)
}

/// Load all answered agent questions for a task, as (question, answer) pairs.
/// Returns an empty Vec if there are no reviews dir or no answered questions.
/// Used to inject prior decisions into an agent's prompt so it never re-asks
/// a question the human has already answered.
pub fn load_answered_for_task(task_id: &str) -> Vec<(String, String)> {
    let dir = reviews_dir();
    if !dir.exists() {
        return Vec::new();
    }
    let mut pairs = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let item: PersistedReviewItem = match toml::from_str(&text) {
            Ok(i) => i,
            Err(_) => continue,
        };
        if item.task_id != task_id {
            continue;
        }
        let question = match &item.kind {
            PersistedReviewKind::AgentQuestion { question } => question.clone(),
            PersistedReviewKind::BufferApproval { .. } => continue,
        };
        // Use the most recent Answer action if multiple exist.
        if let Some(answer) = item.actions.iter().rev().find_map(|a| {
            if let ReviewAction::Answer { content, .. } = a {
                Some(content.clone())
            } else {
                None
            }
        }) {
            pairs.push((question, answer));
        }
    }
    pairs
}

/// Load all review items that have no terminal action recorded.
pub fn load_all_unresolved() -> Result<Vec<PersistedReviewItem>> {
    let dir = reviews_dir();
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::path::Path;

    fn arb_column() -> impl Strategy<Value = Column> {
        prop_oneof![
            Just(Column::Prioritized),
            Just(Column::Designing),
            Just(Column::CodingBuffer),
            Just(Column::Coding),
            Just(Column::ReviewBuffer),
            Just(Column::Reviewing),
            Just(Column::SecurityAuditBuffer),
            Just(Column::Auditing),
            Just(Column::DeploymentBuffer),
            Just(Column::Deploying),
            Just(Column::Done),
        ]
    }

    fn arb_review_kind() -> impl Strategy<Value = PersistedReviewKind> {
        prop_oneof![
            "[a-z]{1,20}".prop_map(|q| PersistedReviewKind::AgentQuestion { question: q }),
            (arb_column(), 0u32..1000).prop_map(|(col, pri)| {
                PersistedReviewKind::BufferApproval {
                    buffer_column: col,
                    task_priority: pri,
                }
            }),
        ]
    }

    fn arb_datetime() -> impl Strategy<Value = DateTime<Utc>> {
        (0i64..2_000_000_000)
            .prop_map(|secs| DateTime::from_timestamp(secs, 0).unwrap_or_else(Utc::now))
    }

    fn arb_role() -> impl Strategy<Value = Role> {
        prop_oneof![Just(Role::User), Just(Role::System), Just(Role::Agent)]
    }

    fn arb_review_action() -> impl Strategy<Value = ReviewAction> {
        prop_oneof![
            arb_datetime().prop_map(|at| ReviewAction::Approval { at }),
            (arb_datetime(), "[a-z ]{1,40}")
                .prop_map(|(at, feedback)| ReviewAction::Rejection { at, feedback }),
            (arb_datetime(), "[a-z ]{1,40}")
                .prop_map(|(at, content)| ReviewAction::Answer { at, content }),
            (arb_datetime(), arb_role(), "[a-z ]{1,40}")
                .prop_map(|(at, role, content)| ReviewAction::Comment { at, role, content }),
        ]
    }

    fn arb_review_item_empty_actions() -> impl Strategy<Value = PersistedReviewItem> {
        (
            "[a-z0-9]{8}",
            "[a-z0-9]{8}",
            arb_review_kind(),
            arb_datetime(),
        )
            .prop_map(|(id, task_id, kind, created_at)| PersistedReviewItem {
                id,
                task_id,
                kind,
                created_at,
                actions: Vec::new(),
            })
    }

    fn arb_persisted_review_item() -> impl Strategy<Value = PersistedReviewItem> {
        (
            "[a-z0-9]{5,20}",
            "[a-z0-9]{5,20}",
            arb_review_kind(),
            arb_datetime(),
            prop::collection::vec(arb_review_action(), 0..5),
        )
            .prop_map(
                |(id, task_id, kind, created_at, actions)| PersistedReviewItem {
                    id,
                    task_id,
                    kind,
                    created_at,
                    actions,
                },
            )
    }

    fn write_item_to(dir: &Path, item: &PersistedReviewItem) {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("{}.toml", item.id));
        let content = toml::to_string_pretty(item).unwrap();
        std::fs::write(&path, &content).unwrap();
    }

    fn read_item_from(dir: &Path, id: &str) -> PersistedReviewItem {
        let path = dir.join(format!("{id}.toml"));
        let text = std::fs::read_to_string(&path).unwrap();
        toml::from_str(&text).unwrap()
    }

    proptest! {
        #[test]
        fn review_item_round_trip(item in arb_persisted_review_item()) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(format!("{}.toml", &item.id));

            let serialized = toml::to_string_pretty(&item).unwrap();
            std::fs::write(&path, &serialized).unwrap();

            let read_back = std::fs::read_to_string(&path).unwrap();
            let deserialized: PersistedReviewItem =
                toml::from_str(&read_back).unwrap();

            prop_assert_eq!(item, deserialized);
        }
    }

    proptest! {
        #[test]
        fn prop_action_append_persistence(
            item in arb_review_item_empty_actions(),
            actions in proptest::collection::vec(arb_review_action(), 1..=5),
        ) {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path();

            write_item_to(dir, &item);

            let mut current = item.clone();
            for action in &actions {
                current.actions.push(action.clone());
                write_item_to(dir, &current);
            }

            let loaded = read_item_from(dir, &current.id);

            prop_assert_eq!(loaded.actions.len(), actions.len());
            for (i, (loaded_action, original_action)) in
                loaded.actions.iter().zip(actions.iter()).enumerate()
            {
                prop_assert_eq!(
                    loaded_action, original_action,
                    "Action mismatch at index {}", i
                );
            }

            prop_assert_eq!(&loaded.id, &item.id);
            prop_assert_eq!(&loaded.task_id, &item.task_id);
            prop_assert_eq!(&loaded.kind, &item.kind);
            prop_assert_eq!(loaded.created_at, item.created_at);
        }
    }
}
