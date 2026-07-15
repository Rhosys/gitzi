// Feature: event-driven-dispatcher, Property 1: Board construction preserves task-to-column mapping
// Feature: event-driven-dispatcher, Property 4: WIP limit enforcement
// **Validates: Requirements 2.1, 2.2, 4.1, 4.2**

use gitzi::dispatcher::Column;
use gitzi::dispatcher::board::{KanbanBoard, WipLimits};
use gitzi::model::task::{Stage, Task};
use proptest::prelude::*;

/// Strategy that generates an arbitrary `Stage` value (all 14 variants).
fn arb_stage() -> impl Strategy<Value = Stage> {
    prop_oneof![
        Just(Stage::Backlog),
        Just(Stage::InProgress),
        Just(Stage::WaitingForReview),
        Just(Stage::Prioritized),
        Just(Stage::Designing),
        Just(Stage::CodingBuffer),
        Just(Stage::Coding),
        Just(Stage::ReviewBuffer),
        Just(Stage::Reviewing),
        Just(Stage::SecurityAuditBuffer),
        Just(Stage::Auditing),
        Just(Stage::DeploymentBuffer),
        Just(Stage::Deploying),
        Just(Stage::Done),
    ]
}

/// Strategy that generates a task with a specific id, arbitrary stage and priority.
fn arb_task(id: usize) -> impl Strategy<Value = Task> {
    (arb_stage(), 0u32..1000).prop_map(move |(stage, priority)| {
        let mut t = Task::new(format!("task-{id}"), "epic-1", format!("Title {id}"));
        t.stage = stage;
        t.priority = priority;
        t
    })
}

/// Strategy that generates a vector of tasks with unique ids, arbitrary stages and priorities.
fn arb_task_vec(max_len: usize) -> impl Strategy<Value = Vec<Task>> {
    (1usize..=max_len).prop_flat_map(|len| {
        let strategies: Vec<_> = (0..len).map(|i| arb_task(i)).collect();
        strategies
    })
}

proptest! {
    /// Property 1: Board construction preserves task-to-column mapping
    ///
    /// For any set of tasks with arbitrary stages and priorities, `KanbanBoard::from_tasks()`
    /// places every task in the column matching `task.stage.to_column()`, and within each
    /// column tasks are sorted by priority ascending.
    #[test]
    fn board_construction_preserves_task_to_column_mapping(
        tasks in arb_task_vec(30)
    ) {
        let task_expectations: Vec<(String, Column, u32)> = tasks
            .iter()
            .map(|t| (t.id.clone(), t.stage.to_column(), t.priority))
            .collect();

        let board = KanbanBoard::from_tasks(tasks);

        // Every task must appear in exactly the column its stage maps to
        for (id, expected_col, _priority) in &task_expectations {
            let ids_in_col = board.tasks_in(*expected_col);
            prop_assert!(
                ids_in_col.contains(id),
                "Task '{}' should be in column {:?} but wasn't found there. Column contains: {:?}",
                id, expected_col, ids_in_col
            );
        }

        // No task is lost: total tasks across all columns equals input count
        let total: usize = Column::all().iter().map(|c| board.count(*c)).sum();
        prop_assert_eq!(
            total,
            task_expectations.len(),
            "Total tasks on board ({}) != input count ({})",
            total,
            task_expectations.len()
        );

        // Within each column, tasks are sorted by priority ascending
        for col in Column::all() {
            let ids_in_col = board.tasks_in(*col);
            for window in ids_in_col.windows(2) {
                let prio_a = board.task(&window[0]).unwrap().priority;
                let prio_b = board.task(&window[1]).unwrap().priority;
                prop_assert!(
                    prio_a <= prio_b,
                    "In column {:?}, task '{}' (prio {}) should come before '{}' (prio {})",
                    col, window[0], prio_a, window[1], prio_b
                );
            }
        }
    }

    /// Property 4: WIP limit enforcement
    ///
    /// For any column with WIP limit L:
    /// - allows(col, 0) is always true for finite limits
    /// - allows(col, limit) is always false for finite limits
    /// - unlimited columns (u32::MAX) always allow
    /// - allows(col, count) returns true iff count < limit
    #[test]
    fn wip_limit_enforcement(
        col_idx in 0usize..11,
        count in 0u32..100
    ) {
        let wip = WipLimits::default();
        let col = Column::all()[col_idx];
        let result = wip.allows(col, count);

        // Determine expected limit for this column
        let limit = match col {
            Column::Prioritized | Column::Done => u32::MAX,
            _ => 1u32,
        };

        if limit == u32::MAX {
            // Unlimited columns always allow
            prop_assert!(
                result,
                "Unlimited column {:?} should allow count={}, but blocked",
                col, count
            );
        } else {
            // Finite limit: allows iff count < limit
            let expected = count < limit;
            prop_assert_eq!(
                result, expected,
                "Column {:?} (limit={}) with count={}: expected allows={}, got={}",
                col, limit, count, expected, result
            );
        }
    }

    /// Property 4 (boundary): allows(col, 0) is always true for every column
    /// (even finite limits, since 0 < any positive limit).
    #[test]
    fn wip_allows_zero_is_always_true(col_idx in 0usize..11) {
        let wip = WipLimits::default();
        let col = Column::all()[col_idx];
        prop_assert!(
            wip.allows(col, 0),
            "Column {:?} should allow count=0 (under any limit), but blocked",
            col
        );
    }

    /// Property 4 (at-limit): For finite-limit columns, allows(col, limit) is always false.
    #[test]
    fn wip_blocks_at_limit(col_idx in 0usize..11) {
        let wip = WipLimits::default();
        let col = Column::all()[col_idx];

        let limit = match col {
            Column::Prioritized | Column::Done => return Ok(()), // skip unlimited
            _ => 1u32,
        };

        prop_assert!(
            !wip.allows(col, limit),
            "Column {:?} with limit={} should block at count={}, but allowed",
            col, limit, limit
        );
    }
}
