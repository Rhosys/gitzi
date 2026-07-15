use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::Result;
use crate::model::{Epic, Task};
use crate::state::review::PersistedReviewItem;

/// Abstraction over task/epic/review persistence.
/// `FileStore` wraps the existing free functions; `InMemoryStore` is for tests.
pub trait StateStore: Send + Sync {
    fn write_task(&self, task: &Task) -> Result<()>;
    fn load_task(&self, id: &str) -> Result<Task>;
    fn load_all_tasks(&self) -> Result<Vec<Task>>;
    fn write_epic(&self, epic: &Epic) -> Result<()>;
    fn load_epic(&self, id: &str) -> Result<Epic>;
    fn load_all_epics(&self) -> Result<Vec<Epic>>;
    fn write_review_item(&self, item: &PersistedReviewItem) -> Result<()>;
    fn load_review_item(&self, id: &str) -> Result<PersistedReviewItem>;
    fn find_unresolved_for_task(&self, task_id: &str) -> Result<Option<PersistedReviewItem>>;
    fn load_all_unresolved(&self) -> Result<Vec<PersistedReviewItem>>;
    fn load_answered_for_task(&self, task_id: &str) -> Vec<(String, String)>;
}

// ── FileStore ─────────────────────────────────────────────────────────────────

/// Delegates to the existing free functions in `state::writer`, `state::reader`,
/// and `state::review`. All I/O goes through `~/.gitzi/`.
pub struct FileStore;

impl StateStore for FileStore {
    fn write_task(&self, task: &Task) -> Result<()> {
        crate::state::writer::write_task(task)
    }

    fn load_task(&self, id: &str) -> Result<Task> {
        crate::state::reader::load_task(id)
    }

    fn load_all_tasks(&self) -> Result<Vec<Task>> {
        crate::state::reader::load_all_tasks()
    }

    fn write_epic(&self, epic: &Epic) -> Result<()> {
        crate::state::writer::write_epic(epic)
    }

    fn load_epic(&self, id: &str) -> Result<Epic> {
        crate::state::reader::load_epic(id)
    }

    fn load_all_epics(&self) -> Result<Vec<Epic>> {
        crate::state::reader::load_all_epics()
    }

    fn write_review_item(&self, item: &PersistedReviewItem) -> Result<()> {
        crate::state::review::write_review_item(item)
    }

    fn load_review_item(&self, id: &str) -> Result<PersistedReviewItem> {
        crate::state::review::load_review_item(id)
    }

    fn find_unresolved_for_task(&self, task_id: &str) -> Result<Option<PersistedReviewItem>> {
        crate::state::review::find_unresolved_for_task(task_id)
    }

    fn load_all_unresolved(&self) -> Result<Vec<PersistedReviewItem>> {
        crate::state::review::load_all_unresolved()
    }

    fn load_answered_for_task(&self, task_id: &str) -> Vec<(String, String)> {
        crate::state::review::load_answered_for_task(task_id)
    }
}

// ── InMemoryStore ─────────────────────────────────────────────────────────────

/// Fully in-memory implementation for tests. No disk I/O.
pub struct InMemoryStore {
    tasks: Mutex<HashMap<String, Task>>,
    epics: Mutex<HashMap<String, Epic>>,
    reviews: Mutex<HashMap<String, PersistedReviewItem>>,
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            tasks: Mutex::new(HashMap::new()),
            epics: Mutex::new(HashMap::new()),
            reviews: Mutex::new(HashMap::new()),
        }
    }
}

impl StateStore for InMemoryStore {
    fn write_task(&self, task: &Task) -> Result<()> {
        self.tasks
            .lock()
            .unwrap()
            .insert(task.id.clone(), task.clone());
        Ok(())
    }

    fn load_task(&self, id: &str) -> Result<Task> {
        self.tasks
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| crate::error::GitziError::TaskNotFound(id.to_string()))
    }

    fn load_all_tasks(&self) -> Result<Vec<Task>> {
        Ok(self.tasks.lock().unwrap().values().cloned().collect())
    }

    fn write_epic(&self, epic: &Epic) -> Result<()> {
        self.epics
            .lock()
            .unwrap()
            .insert(epic.id.clone(), epic.clone());
        Ok(())
    }

    fn load_epic(&self, id: &str) -> Result<Epic> {
        self.epics
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| crate::error::GitziError::EpicNotFound(id.to_string()))
    }

    fn load_all_epics(&self) -> Result<Vec<Epic>> {
        Ok(self.epics.lock().unwrap().values().cloned().collect())
    }

    fn write_review_item(&self, item: &PersistedReviewItem) -> Result<()> {
        self.reviews
            .lock()
            .unwrap()
            .insert(item.id.clone(), item.clone());
        Ok(())
    }

    fn load_review_item(&self, id: &str) -> Result<PersistedReviewItem> {
        self.reviews
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| {
                crate::error::GitziError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("review item '{id}' not found"),
                ))
            })
    }

    fn find_unresolved_for_task(&self, task_id: &str) -> Result<Option<PersistedReviewItem>> {
        let reviews = self.reviews.lock().unwrap();
        Ok(reviews
            .values()
            .find(|item| item.task_id == task_id && item.is_unresolved())
            .cloned())
    }

    fn load_all_unresolved(&self) -> Result<Vec<PersistedReviewItem>> {
        let reviews = self.reviews.lock().unwrap();
        Ok(reviews
            .values()
            .filter(|item| item.is_unresolved())
            .cloned()
            .collect())
    }

    fn load_answered_for_task(&self, task_id: &str) -> Vec<(String, String)> {
        use crate::state::review::{PersistedReviewKind, ReviewAction};
        let reviews = self.reviews.lock().unwrap();
        let mut pairs = Vec::new();
        for item in reviews.values() {
            if item.task_id != task_id {
                continue;
            }
            let question = match &item.kind {
                PersistedReviewKind::AgentQuestion { question } => question.clone(),
                PersistedReviewKind::BufferApproval { .. } => continue,
            };
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
}
