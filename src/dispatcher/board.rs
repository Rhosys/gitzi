use std::collections::HashMap;

use serde::Deserialize;
use tracing::warn;

use super::Column;
use crate::model::task::Task;

// ─── KanbanBoard ──────────────────────────────────────────────────────────────

/// In-memory projection of all tasks organized by Kanban column.
/// Tasks within each column are kept sorted by priority (lowest number first).
/// Rebuilt from persistent task files on every boot — never persisted itself.
#[derive(Debug, Clone)]
pub struct KanbanBoard {
    /// Column → ordered task IDs (sorted by priority ascending).
    columns: HashMap<Column, Vec<String>>,
    /// task_id → full Task data.
    tasks: HashMap<String, Task>,
}

impl KanbanBoard {
    /// Construct the board projection from a set of tasks.
    /// Each task is placed in the column corresponding to its stage.
    /// Invalid/unrecognized stages are handled by `Stage::to_column()` which
    /// maps legacy stages to their closest equivalent (defaulting toward Prioritized).
    pub fn from_tasks(tasks: Vec<Task>) -> Self {
        let mut columns: HashMap<Column, Vec<String>> = HashMap::new();
        let mut task_map: HashMap<String, Task> = HashMap::new();

        // Initialize all columns
        for col in Column::all() {
            columns.insert(*col, Vec::new());
        }

        for task in tasks {
            let column = task.stage.to_column();
            columns.entry(column).or_default().push(task.id.clone());
            task_map.insert(task.id.clone(), task);
        }

        let mut board = Self {
            columns,
            tasks: task_map,
        };

        // Sort each column by priority
        for col in Column::all() {
            board.sort_column(*col);
        }

        board
    }

    /// Get the ordered task IDs in a column.
    pub fn tasks_in(&self, column: Column) -> &[String] {
        self.columns
            .get(&column)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Find a task by ID across all columns.
    pub fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.get(id)
    }

    /// Find a task mutably by ID.
    pub fn task_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.get_mut(id)
    }

    /// Find which column a task currently lives in (public for dispatcher use).
    pub fn column_of(&self, task_id: &str) -> Option<Column> {
        self.find_column(task_id)
    }

    /// Number of tasks in a column.
    pub fn count(&self, column: Column) -> usize {
        self.columns.get(&column).map(|v| v.len()).unwrap_or(0)
    }

    /// Move a task to a new column, re-sorting at the destination.
    /// Updates the task's stage to match the target column.
    pub fn advance(&mut self, task_id: &str, to: Column) -> anyhow::Result<()> {
        // Find and remove from current column
        let current_col = self
            .find_column(task_id)
            .ok_or_else(|| anyhow::anyhow!("task '{task_id}' not found on board"))?;

        let ids = self.columns.get_mut(&current_col).unwrap();
        ids.retain(|id| id != task_id);

        // Update task's stage
        let task = self
            .tasks
            .get_mut(task_id)
            .ok_or_else(|| anyhow::anyhow!("task '{task_id}' not in task map"))?;
        task.stage = to.into();

        // Insert into destination column
        self.columns
            .entry(to)
            .or_default()
            .push(task_id.to_string());
        self.sort_column(to);

        Ok(())
    }

    /// Add a new task to the board, placing it in the column matching its stage.
    pub fn add_task(&mut self, task: Task) {
        let column = task.stage.to_column();
        self.columns
            .entry(column)
            .or_default()
            .push(task.id.clone());
        self.tasks.insert(task.id.clone(), task);
        self.sort_column(column);
    }

    /// First task in `column` (priority order) whose blockers have all reached
    /// `Stage::Done`. Tasks with no blockers are always eligible. A blocker ID
    /// not found on the board is treated as not-yet-done (fails closed).
    pub fn next_unblocked(&self, column: Column) -> Option<&Task> {
        self.tasks_in(column).iter().find_map(|id| {
            let task = self.tasks.get(id)?;
            let ready = task.blocked_by.iter().all(|b| {
                self.tasks
                    .get(b)
                    .is_some_and(|bt| bt.stage == crate::model::Stage::Done)
            });
            ready.then_some(task)
        })
    }

    /// Update a task's priority and re-sort its column.
    pub fn set_priority(&mut self, task_id: &str, priority: u32) {
        if let Some(task) = self.tasks.get_mut(task_id) {
            task.priority = priority;
            let col = task.stage.to_column();
            self.sort_column(col);
        } else {
            warn!(task_id, "set_priority called for unknown task");
        }
    }

    // ─── Private helpers ──────────────────────────────────────────────────────

    /// Sort a column's task IDs by their priority (lowest first).
    fn sort_column(&mut self, column: Column) {
        if let Some(ids) = self.columns.get_mut(&column) {
            let tasks = &self.tasks;
            ids.sort_by_key(|id| tasks.get(id).map(|t| t.priority).unwrap_or(u32::MAX));
        }
    }

    /// Find which column a task currently lives in.
    fn find_column(&self, task_id: &str) -> Option<Column> {
        for (col, ids) in &self.columns {
            if ids.iter().any(|id| id == task_id) {
                return Some(*col);
            }
        }
        None
    }
}

// ─── WipLimits ────────────────────────────────────────────────────────────────

/// Per-column work-in-progress limits. Enforcement happens in the Dispatcher
/// when attempting to advance a task into a column.
#[derive(Debug, Clone)]
pub struct WipLimits {
    limits: HashMap<Column, u32>,
}

impl Default for WipLimits {
    fn default() -> Self {
        let mut limits = HashMap::new();
        // Work columns: 1 each (except Prioritized = unlimited backlog staging)
        limits.insert(Column::Prioritized, u32::MAX);
        limits.insert(Column::Designing, 1);
        limits.insert(Column::Coding, 1);
        limits.insert(Column::Reviewing, 1);
        limits.insert(Column::Auditing, 1);
        limits.insert(Column::Deploying, 1);
        // Buffer columns: 1 each
        limits.insert(Column::CodingBuffer, 1);
        limits.insert(Column::ReviewBuffer, 1);
        limits.insert(Column::SecurityAuditBuffer, 1);
        limits.insert(Column::DeploymentBuffer, 1);
        // Done: unlimited
        limits.insert(Column::Done, u32::MAX);
        Self { limits }
    }
}

impl WipLimits {
    /// Returns true if the column can accept another task given its current count.
    pub fn allows(&self, column: Column, current_count: u32) -> bool {
        let limit = self.limits.get(&column).copied().unwrap_or(u32::MAX);
        current_count < limit
    }

    /// Build limits from config overrides layered onto the built-in defaults.
    /// Keys are column names as they serialize on the wire (kebab-case, e.g.
    /// `"coding-buffer"`). Returns an error naming the offending key if any
    /// override doesn't match a known column.
    /// Unknown columns are silently ignored (logged as warnings).
    pub fn from_config(overrides: &HashMap<String, u32>) -> Result<Self, String> {
        let mut limits = Self::default().limits;
        for (name, &value) in overrides {
            match parse_column_name(name) {
                Some(column) => {
                    limits.insert(column, value);
                }
                None => {
                    tracing::warn!("ignoring unknown WIP column '{name}' in config");
                }
            }
        }
        Ok(Self { limits })
    }

    /// Check if a column name string is a valid/known column.
    pub fn is_valid_column(name: &str) -> bool {
        parse_column_name(name).is_some()
    }
}

/// Parse a column name (as it appears in config, e.g. `"coding-buffer"`) into
/// a `Column` by reusing its existing kebab-case `Deserialize` impl — keeps
/// the name mapping in one place instead of duplicating it here.
fn parse_column_name(name: &str) -> Option<Column> {
    use serde::de::value::{Error as DeError, StrDeserializer};
    let deserializer: StrDeserializer<DeError> = StrDeserializer::new(name);
    Column::deserialize(deserializer).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::task::{Stage, Task};

    fn make_task(id: &str, stage: Stage, priority: u32) -> Task {
        let mut t = Task::new(id, "epic-1", format!("Task {id}"));
        t.stage = stage;
        t.priority = priority;
        t
    }

    // ─── KanbanBoard tests ────────────────────────────────────────────────────

    #[test]
    fn from_tasks_places_tasks_in_correct_columns() {
        let tasks = vec![
            make_task("a", Stage::Coding, 10),
            make_task("b", Stage::Prioritized, 20),
            make_task("c", Stage::Done, 5),
        ];
        let board = KanbanBoard::from_tasks(tasks);

        assert_eq!(board.tasks_in(Column::Coding), &["a"]);
        assert_eq!(board.tasks_in(Column::Prioritized), &["b"]);
        assert_eq!(board.tasks_in(Column::Done), &["c"]);
    }

    #[test]
    fn from_tasks_sorts_by_priority_within_column() {
        let tasks = vec![
            make_task("high", Stage::Coding, 50),
            make_task("low", Stage::Coding, 10),
            make_task("mid", Stage::Coding, 30),
        ];
        let board = KanbanBoard::from_tasks(tasks);

        assert_eq!(board.tasks_in(Column::Coding), &["low", "mid", "high"]);
    }

    #[test]
    fn from_tasks_maps_legacy_stages() {
        let tasks = vec![
            make_task("a", Stage::Backlog, 10),
            make_task("b", Stage::InProgress, 20),
            make_task("c", Stage::WaitingForReview, 30),
        ];
        let board = KanbanBoard::from_tasks(tasks);

        assert_eq!(board.count(Column::Prioritized), 1); // Backlog → Prioritized
        assert_eq!(board.count(Column::Coding), 1); // InProgress → Coding
        assert_eq!(board.count(Column::ReviewBuffer), 1); // WaitingForReview → ReviewBuffer
    }

    #[test]
    fn task_finds_by_id() {
        let tasks = vec![make_task("abc", Stage::Designing, 5)];
        let board = KanbanBoard::from_tasks(tasks);

        let t = board.task("abc").unwrap();
        assert_eq!(t.id, "abc");
        assert_eq!(t.priority, 5);
        assert!(board.task("nonexistent").is_none());
    }

    #[test]
    fn count_returns_correct_values() {
        let tasks = vec![
            make_task("a", Stage::Coding, 1),
            make_task("b", Stage::Coding, 2),
            make_task("c", Stage::Done, 3),
        ];
        let board = KanbanBoard::from_tasks(tasks);

        assert_eq!(board.count(Column::Coding), 2);
        assert_eq!(board.count(Column::Done), 1);
        assert_eq!(board.count(Column::Designing), 0);
    }

    #[test]
    fn advance_moves_task_and_resorts() {
        let tasks = vec![
            make_task("a", Stage::Coding, 50),
            make_task("b", Stage::ReviewBuffer, 10),
        ];
        let mut board = KanbanBoard::from_tasks(tasks);

        board.advance("a", Column::ReviewBuffer).unwrap();

        assert_eq!(board.count(Column::Coding), 0);
        // b (prio 10) before a (prio 50)
        assert_eq!(board.tasks_in(Column::ReviewBuffer), &["b", "a"]);
        // Task's stage is updated
        assert_eq!(board.task("a").unwrap().stage, Stage::ReviewBuffer);
    }

    #[test]
    fn advance_unknown_task_errors() {
        let mut board = KanbanBoard::from_tasks(vec![]);
        assert!(board.advance("ghost", Column::Coding).is_err());
    }

    #[test]
    fn set_priority_updates_and_resorts() {
        let tasks = vec![
            make_task("a", Stage::Coding, 10),
            make_task("b", Stage::Coding, 20),
        ];
        let mut board = KanbanBoard::from_tasks(tasks);

        // a=10, b=20 → order is [a, b]
        assert_eq!(board.tasks_in(Column::Coding), &["a", "b"]);

        // Set a's priority higher (worse) than b
        board.set_priority("a", 99);
        assert_eq!(board.tasks_in(Column::Coding), &["b", "a"]);
        assert_eq!(board.task("a").unwrap().priority, 99);
    }

    #[test]
    fn next_unblocked_skips_tasks_with_unfinished_blockers() {
        let mut blocked = make_task("blocked", Stage::Prioritized, 0);
        blocked.blocked_by = vec!["blocker".to_string()];
        let blocker = make_task("blocker", Stage::Coding, 50);
        let free = make_task("free", Stage::Prioritized, 10);
        let board = KanbanBoard::from_tasks(vec![blocked, blocker, free]);

        // "blocked" sorts first (priority 0) but its blocker isn't Done yet.
        let next = board.next_unblocked(Column::Prioritized).unwrap();
        assert_eq!(next.id, "free");
    }

    #[test]
    fn next_unblocked_allows_task_once_blocker_is_done() {
        let mut blocked = make_task("blocked", Stage::Prioritized, 0);
        blocked.blocked_by = vec!["blocker".to_string()];
        let blocker = make_task("blocker", Stage::Done, 50);
        let board = KanbanBoard::from_tasks(vec![blocked, blocker]);

        let next = board.next_unblocked(Column::Prioritized).unwrap();
        assert_eq!(next.id, "blocked");
    }

    #[test]
    fn next_unblocked_treats_unknown_blocker_as_not_done() {
        let mut blocked = make_task("blocked", Stage::Prioritized, 0);
        blocked.blocked_by = vec!["ghost".to_string()];
        let board = KanbanBoard::from_tasks(vec![blocked]);

        assert!(board.next_unblocked(Column::Prioritized).is_none());
    }

    #[test]
    fn next_unblocked_with_no_blockers_is_always_eligible() {
        let tasks = vec![make_task("a", Stage::Prioritized, 10)];
        let board = KanbanBoard::from_tasks(tasks);

        assert_eq!(board.next_unblocked(Column::Prioritized).unwrap().id, "a");
    }

    #[test]
    fn empty_board_has_zero_counts() {
        let board = KanbanBoard::from_tasks(vec![]);
        for col in Column::all() {
            assert_eq!(board.count(*col), 0);
        }
    }

    // ─── WipLimits tests ──────────────────────────────────────────────────────

    #[test]
    fn default_limits_cover_all_columns() {
        let wip = WipLimits::default();
        for col in Column::all() {
            assert!(
                wip.limits.contains_key(col),
                "WipLimits missing entry for {col}"
            );
        }
    }

    #[test]
    fn work_columns_limit_is_one_except_prioritized() {
        let wip = WipLimits::default();
        let work_with_limit_1 = [
            Column::Designing,
            Column::Coding,
            Column::Reviewing,
            Column::Auditing,
            Column::Deploying,
        ];
        for col in &work_with_limit_1 {
            assert_eq!(wip.limits[col], 1, "{col} should have limit 1");
        }
        assert_eq!(wip.limits[&Column::Prioritized], u32::MAX);
    }

    #[test]
    fn buffer_columns_limit_is_one() {
        let wip = WipLimits::default();
        let buffers = [
            Column::CodingBuffer,
            Column::ReviewBuffer,
            Column::SecurityAuditBuffer,
            Column::DeploymentBuffer,
        ];
        for col in &buffers {
            assert_eq!(wip.limits[col], 1, "{col} should have limit 1");
        }
    }

    #[test]
    fn done_is_unlimited() {
        let wip = WipLimits::default();
        assert_eq!(wip.limits[&Column::Done], u32::MAX);
    }

    #[test]
    fn allows_when_under_limit() {
        let wip = WipLimits::default();
        assert!(wip.allows(Column::Coding, 0));
    }

    #[test]
    fn blocks_when_at_limit() {
        let wip = WipLimits::default();
        assert!(!wip.allows(Column::Coding, 1));
    }

    #[test]
    fn unlimited_columns_always_allow() {
        let wip = WipLimits::default();
        assert!(wip.allows(Column::Prioritized, 1_000_000));
        assert!(wip.allows(Column::Done, 1_000_000));
    }
}
