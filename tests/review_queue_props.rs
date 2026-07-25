// Feature: event-driven-dispatcher, Property 12: Queue ordering invariant
// Feature: event-driven-dispatcher, Property 13: All items visible (no suppression)
// **Validates: Requirements 7.2**

use chrono::{DateTime, TimeZone, Utc};
use gitzi::dispatcher::Column;
use gitzi::dispatcher::review_queue::{HumanReviewItem, HumanReviewQueue, ReviewItemKind};
use proptest::prelude::*;

/// The 4 buffer columns in pipeline order (left to right on the board).
const BUFFER_COLUMNS: [Column; 4] = [
    Column::CodingBuffer,
    Column::ReviewBuffer,
    Column::SecurityAuditBuffer,
    Column::DeploymentBuffer,
];

/// Strategy for an arbitrary buffer column.
fn arb_buffer_column() -> impl Strategy<Value = Column> {
    (0usize..4).prop_map(|i| BUFFER_COLUMNS[i])
}

/// Strategy for an arbitrary timestamp (seconds from epoch, spread out to avoid collisions).
fn arb_timestamp() -> impl Strategy<Value = DateTime<Utc>> {
    (1i64..100_000).prop_map(|s| Utc.timestamp_opt(s, 0).unwrap())
}

/// Strategy for an AgentQuestion review item.
fn arb_question() -> impl Strategy<Value = HumanReviewItem> {
    (0usize..1000, arb_timestamp()).prop_map(|(id, ts)| {
        HumanReviewItem::with_timestamp(
            format!("task-q-{id}"),
            ReviewItemKind::AgentQuestion {
                question: format!("Question {id}?"),
            },
            format!("Question {id}?"),
            ts,
        )
    })
}

/// Strategy for a BufferApproval review item.
fn arb_approval() -> impl Strategy<Value = HumanReviewItem> {
    (
        0usize..1000,
        arb_buffer_column(),
        0u32..100,
        arb_timestamp(),
    )
        .prop_map(|(id, col, prio, ts)| {
            HumanReviewItem::with_timestamp(
                format!("task-a-{id}"),
                ReviewItemKind::BufferApproval {
                    buffer_column: col,
                    task_priority: prio,
                },
                format!("Approve task-a-{id} in {col:?}"),
                ts,
            )
        })
}

/// Strategy for a mixed review item (question or approval).
fn arb_review_item() -> impl Strategy<Value = HumanReviewItem> {
    prop_oneof![arb_question(), arb_approval(),]
}

/// Strategy for a non-empty vec of mixed review items.
fn arb_item_vec(max_len: usize) -> impl Strategy<Value = Vec<HumanReviewItem>> {
    proptest::collection::vec(arb_review_item(), 1..=max_len)
}

/// Column index in the pipeline (higher = more rightward).
fn column_index(col: Column) -> usize {
    Column::all().iter().position(|c| *c == col).unwrap_or(0)
}

proptest! {
    /// Property 12: Queue ordering invariant
    ///
    /// For any sequence of enqueue operations mixing questions and approvals, the resulting
    /// queue maintains the invariant:
    /// - All questions precede all approvals
    /// - Questions are FIFO by timestamp
    /// - Approvals are ordered by rightmost column first, then priority ascending
    #[test]
    fn queue_ordering_invariant(items in arb_item_vec(50)) {
        let mut queue = HumanReviewQueue::new();
        for item in &items {
            queue.enqueue(item.clone());
        }

        // Drain the queue by repeatedly peeking and dequeueing the peeked item.
        // This gives us the ordered sequence.
        let mut ordered: Vec<HumanReviewItem> = Vec::new();
        while let Some(peeked) = queue.peek() {
            let id = peeked.id.clone();
            let item = queue.dequeue(&id).unwrap();
            ordered.push(item);
        }

        // All items were preserved
        prop_assert_eq!(ordered.len(), items.len());

        // Split into questions and approvals in order
        let mut saw_approval = false;
        let mut last_question_ts: Option<DateTime<Utc>> = None;
        let mut last_approval_col_idx: Option<usize> = None;
        let mut last_approval_prio: Option<u32> = None;
        let mut last_approval_col_for_prio: Option<Column> = None;

        for item in &ordered {
            match &item.kind {
                ReviewItemKind::AgentQuestion { .. } => {
                    // No question should appear after an approval
                    prop_assert!(
                        !saw_approval,
                        "Question '{}' appeared after an approval in the queue ordering",
                        item.task_id
                    );
                    // Questions must be FIFO by timestamp
                    if let Some(prev_ts) = last_question_ts {
                        prop_assert!(
                            item.created_at >= prev_ts,
                            "Question '{}' (ts={}) appeared after question with later ts={}",
                            item.task_id, item.created_at, prev_ts
                        );
                    }
                    last_question_ts = Some(item.created_at);
                }
                ReviewItemKind::BufferApproval { buffer_column, task_priority } => {
                    saw_approval = true;
                    let col_idx = column_index(*buffer_column);

                    // Approvals: rightmost column first (descending column index)
                    if let Some(prev_col_idx) = last_approval_col_idx {
                        prop_assert!(
                            col_idx <= prev_col_idx,
                            "Approval '{}' in column {:?} (idx={}) appeared after column idx={} — should be rightmost first",
                            item.task_id, buffer_column, col_idx, prev_col_idx
                        );

                        // Within same column: priority ascending
                        if col_idx == prev_col_idx
                            && let (Some(prev_prio), Some(prev_col)) = (last_approval_prio, last_approval_col_for_prio)
                                && prev_col == *buffer_column {
                                    prop_assert!(
                                        *task_priority >= prev_prio,
                                        "Approval '{}' (prio={}) in {:?} should come after prio={} (ascending)",
                                        item.task_id, task_priority, buffer_column, prev_prio
                                    );
                                }
                    }

                    last_approval_col_idx = Some(col_idx);
                    last_approval_prio = Some(*task_priority);
                    last_approval_col_for_prio = Some(*buffer_column);
                }
            }
        }
    }

    /// Property 13: All items visible — no suppression
    ///
    /// When the queue contains both questions and approvals, `items()` returns all of them.
    /// `peek()` returns the first by sort order (a question, since questions sort first),
    /// but approvals are never hidden.
    #[test]
    fn all_items_visible_no_suppression(
        questions in proptest::collection::vec(arb_question(), 1..=10),
        approvals in proptest::collection::vec(arb_approval(), 1..=10),
    ) {
        let mut queue = HumanReviewQueue::new();

        // Enqueue approvals first, then questions (order shouldn't matter)
        for a in &approvals {
            queue.enqueue(a.clone());
        }
        for q in &questions {
            queue.enqueue(q.clone());
        }

        let total = questions.len() + approvals.len();

        // All items are visible
        prop_assert_eq!(queue.items().len(), total);
        prop_assert_eq!(queue.len(), total);

        // peek returns a question (since questions sort first by kind_rank)
        let peeked = queue.peek().unwrap();
        prop_assert!(
            matches!(peeked.kind, ReviewItemKind::AgentQuestion { .. }),
            "peek() should return a question (sort-first) but got approval: task_id='{}'",
            peeked.task_id
        );

        // question_count is informational and correct
        prop_assert_eq!(queue.question_count(), questions.len());

        // Every item has a non-empty description
        for item in queue.items() {
            prop_assert!(!item.description.is_empty(), "item '{}' has empty description", item.task_id);
        }
    }
}
