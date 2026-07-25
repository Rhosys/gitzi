use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Column;

// ─── ReviewItemKind ───────────────────────────────────────────────────────────

/// The type of human review action needed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReviewItemKind {
    /// An agent is blocked and needs a human answer to continue.
    AgentQuestion { question: String },
    /// A task has entered a buffer column and needs approval to advance.
    BufferApproval {
        buffer_column: Column,
        task_priority: u32,
    },
}

// ─── HumanReviewItem ──────────────────────────────────────────────────────────

/// A single item in the human review queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanReviewItem {
    pub id: String,
    pub task_id: String,
    pub kind: ReviewItemKind,
    /// Short human-readable summary so the user knows what this is about
    /// without drilling into the full item.
    pub description: String,
    pub created_at: DateTime<Utc>,
}

impl HumanReviewItem {
    /// Create a new review item with a generated UUID.
    pub fn new(task_id: impl Into<String>, kind: ReviewItemKind, description: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            task_id: task_id.into(),
            kind,
            description: description.into(),
            created_at: Utc::now(),
        }
    }

    /// Create with an explicit timestamp (useful for testing).
    pub fn with_timestamp(
        task_id: impl Into<String>,
        kind: ReviewItemKind,
        description: impl Into<String>,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            task_id: task_id.into(),
            kind,
            description: description.into(),
            created_at,
        }
    }

}

// ─── Sort key ─────────────────────────────────────────────────────────────────

/// Column position index (higher = further right on board).
fn column_index(col: Column) -> usize {
    Column::all().iter().position(|c| *c == col).unwrap_or(0)
}

/// Sort key for ordering items within the queue.
/// Returns (kind_rank, column_rank_inverted, priority, timestamp).
///
/// - kind_rank: 0 for questions (sort first), 1 for approvals
/// - column_rank_inverted: for approvals, rightmost column gets smallest value (sorts first)
/// - priority: lower number = higher priority (sorts first)
/// - timestamp: earlier = first (FIFO within same rank)
fn sort_key(item: &HumanReviewItem) -> (u8, usize, u32, DateTime<Utc>) {
    match &item.kind {
        ReviewItemKind::AgentQuestion { .. } => {
            // Questions: sorted by arrival time only. Use 0 for column/priority placeholders.
            (0, 0, 0, item.created_at)
        }
        ReviewItemKind::BufferApproval {
            buffer_column,
            task_priority,
        } => {
            // Approvals: rightmost column first (invert index), then priority ascending.
            let max_idx = Column::all().len();
            let col_idx = column_index(*buffer_column);
            let inverted = max_idx - col_idx; // rightmost column → smallest inverted value
            (1, inverted, *task_priority, item.created_at)
        }
    }
}

// ─── HumanReviewQueue ─────────────────────────────────────────────────────────

/// Priority queue of items awaiting human action.
///
/// Ordering:
/// 1. Agent questions are sorted by arrival time (FIFO)
/// 2. Buffer approvals are sorted by rightmost column first, then task priority ascending
/// 3. Questions sort before approvals at equal rank (kind_rank 0 vs 1)
///
/// All items are visible at all times — the human decides what to handle first.
#[derive(Debug, Clone)]
pub struct HumanReviewQueue {
    items: Vec<HumanReviewItem>,
}

impl HumanReviewQueue {
    pub fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// Insert an item maintaining the sort invariant.
    pub fn enqueue(&mut self, item: HumanReviewItem) {
        let key = sort_key(&item);
        let pos = self
            .items
            .iter()
            .position(|existing| sort_key(existing) > key)
            .unwrap_or(self.items.len());
        self.items.insert(pos, item);
    }

    /// The topmost item by sort order.
    pub fn peek(&self) -> Option<&HumanReviewItem> {
        self.items.first()
    }

    /// All items in queue order (for rendering a full list to the user).
    pub fn items(&self) -> &[HumanReviewItem] {
        &self.items
    }

    /// Remove and return the item with the given ID.
    pub fn dequeue(&mut self, item_id: &str) -> Option<HumanReviewItem> {
        let pos = self.items.iter().position(|i| i.id == item_id)?;
        Some(self.items.remove(pos))
    }

    /// Remove and return the buffer-approval item for the given task, if any.
    /// Buffer approvals are looked up by task ID rather than item ID since
    /// callers (approve/reject) only know the task, not the in-memory item ID.
    pub fn dequeue_by_task_id(&mut self, task_id: &str) -> Option<HumanReviewItem> {
        let pos = self.items.iter().position(|i| i.task_id == task_id)?;
        Some(self.items.remove(pos))
    }

    /// Count of agent-question items in the queue (informational).
    pub fn question_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| matches!(i.kind, ReviewItemKind::AgentQuestion { .. }))
            .count()
    }

    /// True if the queue has no items at all.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Number of items in the queue (all kinds).
    pub fn len(&self) -> usize {
        self.items.len()
    }
}

impl Default for HumanReviewQueue {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    fn question(task_id: &str, q: &str, time: DateTime<Utc>) -> HumanReviewItem {
        HumanReviewItem::with_timestamp(
            task_id,
            ReviewItemKind::AgentQuestion {
                question: q.to_string(),
            },
            q,
            time,
        )
    }

    fn approval(task_id: &str, col: Column, priority: u32, time: DateTime<Utc>) -> HumanReviewItem {
        HumanReviewItem::with_timestamp(
            task_id,
            ReviewItemKind::BufferApproval {
                buffer_column: col,
                task_priority: priority,
            },
            format!("Approve {task_id} in {col:?}"),
            time,
        )
    }

    #[test]
    fn empty_queue() {
        let q = HumanReviewQueue::new();
        assert!(q.is_empty());
        assert_eq!(q.question_count(), 0);
        assert!(q.peek().is_none());
    }

    #[test]
    fn enqueue_single_question() {
        let mut q = HumanReviewQueue::new();
        q.enqueue(question("t1", "What color?", ts(100)));

        assert!(!q.is_empty());
        assert_eq!(q.question_count(), 1);
        assert_eq!(q.peek().unwrap().task_id, "t1");
    }

    #[test]
    fn questions_sorted_by_arrival_time() {
        let mut q = HumanReviewQueue::new();
        q.enqueue(question("t2", "Second?", ts(200)));
        q.enqueue(question("t1", "First?", ts(100)));
        q.enqueue(question("t3", "Third?", ts(300)));

        // Peek should return oldest question
        assert_eq!(q.peek().unwrap().task_id, "t1");
    }

    #[test]
    fn questions_sort_before_approvals() {
        let mut q = HumanReviewQueue::new();
        // Add approval first
        q.enqueue(approval("t1", Column::DeploymentBuffer, 1, ts(50)));
        // Then question
        q.enqueue(question("t2", "Help?", ts(200)));

        // Question sorts first due to kind_rank
        assert_eq!(q.peek().unwrap().task_id, "t2");
        // But both are visible
        assert_eq!(q.items().len(), 2);
    }

    #[test]
    fn approvals_rightmost_column_first() {
        let mut q = HumanReviewQueue::new();
        q.enqueue(approval("coding", Column::CodingBuffer, 1, ts(100)));
        q.enqueue(approval("deploy", Column::DeploymentBuffer, 1, ts(100)));
        q.enqueue(approval("review", Column::ReviewBuffer, 1, ts(100)));

        // No questions, so peek returns approvals. Rightmost first = DeploymentBuffer.
        assert_eq!(q.peek().unwrap().task_id, "deploy");
    }

    #[test]
    fn approvals_same_column_sorted_by_priority() {
        let mut q = HumanReviewQueue::new();
        q.enqueue(approval("high-prio", Column::CodingBuffer, 50, ts(100)));
        q.enqueue(approval("low-prio", Column::CodingBuffer, 10, ts(100)));
        q.enqueue(approval("mid-prio", Column::CodingBuffer, 30, ts(100)));

        // Within same column, lowest priority number first
        assert_eq!(q.peek().unwrap().task_id, "low-prio");
    }

    #[test]
    fn dequeue_removes_by_id() {
        let mut q = HumanReviewQueue::new();
        let item = question("t1", "Q?", ts(100));
        let id = item.id.clone();
        q.enqueue(item);
        q.enqueue(question("t2", "Q2?", ts(200)));

        let removed = q.dequeue(&id).unwrap();
        assert_eq!(removed.task_id, "t1");
        assert_eq!(q.len(), 1);
        assert_eq!(q.peek().unwrap().task_id, "t2");
    }

    #[test]
    fn dequeue_nonexistent_returns_none() {
        let mut q = HumanReviewQueue::new();
        q.enqueue(question("t1", "Q?", ts(100)));
        assert!(q.dequeue("nonexistent").is_none());
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn dequeue_by_task_id_removes_buffer_approval() {
        let mut q = HumanReviewQueue::new();
        q.enqueue(approval("t1", Column::CodingBuffer, 50, ts(100)));
        q.enqueue(approval("t2", Column::CodingBuffer, 50, ts(200)));

        let removed = q.dequeue_by_task_id("t1").unwrap();
        assert_eq!(removed.task_id, "t1");
        assert_eq!(q.len(), 1);
        assert_eq!(q.peek().unwrap().task_id, "t2");
    }

    #[test]
    fn all_items_visible_regardless_of_kind() {
        let mut q = HumanReviewQueue::new();
        q.enqueue(question("t1", "Help?", ts(100)));
        q.enqueue(approval("t2", Column::DeploymentBuffer, 1, ts(50)));

        // Both items accessible via items()
        assert_eq!(q.items().len(), 2);
        // Question sorts first
        assert_eq!(q.items()[0].task_id, "t1");
        assert_eq!(q.items()[1].task_id, "t2");
    }

    #[test]
    fn description_populated_on_items() {
        let mut q = HumanReviewQueue::new();
        q.enqueue(question("t1", "What color?", ts(100)));
        q.enqueue(approval("t2", Column::CodingBuffer, 1, ts(200)));

        assert_eq!(q.items()[0].description, "What color?");
        assert_eq!(q.items()[1].description, "Approve t2 in CodingBuffer");
    }

    #[test]
    fn full_ordering_scenario() {
        let mut q = HumanReviewQueue::new();

        // Mix of questions and approvals
        q.enqueue(approval("a1", Column::CodingBuffer, 10, ts(100)));
        q.enqueue(approval("a2", Column::DeploymentBuffer, 5, ts(100)));
        q.enqueue(question("q1", "First question", ts(300)));
        q.enqueue(approval("a3", Column::DeploymentBuffer, 20, ts(100)));
        q.enqueue(question("q2", "Earlier question", ts(100)));

        // Questions first (by time): q2 (t=100) before q1 (t=300)
        let peeked = q.peek().unwrap();
        assert_eq!(peeked.task_id, "q2");

        // Remove both questions
        let q2_id = q
            .items
            .iter()
            .find(|i| i.task_id == "q2")
            .unwrap()
            .id
            .clone();
        let q1_id = q
            .items
            .iter()
            .find(|i| i.task_id == "q1")
            .unwrap()
            .id
            .clone();
        q.dequeue(&q2_id);
        q.dequeue(&q1_id);

        // Now approvals are at top. Rightmost column first = DeploymentBuffer
        // Within DeploymentBuffer: a2 (prio 5) before a3 (prio 20)
        assert_eq!(q.question_count(), 0);
        let peeked = q.peek().unwrap();
        assert_eq!(peeked.task_id, "a2");

        // Remove a2, next should be a3 (same column, higher prio number)
        let a2_id = q
            .items
            .iter()
            .find(|i| i.task_id == "a2")
            .unwrap()
            .id
            .clone();
        q.dequeue(&a2_id);
        assert_eq!(q.peek().unwrap().task_id, "a3");

        // Remove a3, next should be a1 (CodingBuffer, less rightmost)
        let a3_id = q
            .items
            .iter()
            .find(|i| i.task_id == "a3")
            .unwrap()
            .id
            .clone();
        q.dequeue(&a3_id);
        assert_eq!(q.peek().unwrap().task_id, "a1");
    }
}
