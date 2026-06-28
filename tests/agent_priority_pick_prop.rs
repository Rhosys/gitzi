// Feature: event-driven-dispatcher, Property 6: Agent picks highest-priority task
// **Validates: Requirements 5.4**
//
// For any non-empty set of tasks in a single column with arbitrary priorities,
// the agent's pick (first element of board.tasks_in(column)) is always the task
// with the lowest priority number. This tests the board's sorting invariant that
// the agent loop relies on.

use gitzi::dispatcher::board::KanbanBoard;
use gitzi::dispatcher::Column;
use gitzi::model::task::{Stage, Task};
use proptest::prelude::*;

/// Generate a work column (one that has an agent role — these are the columns
/// agents pick from).
fn arb_work_column() -> impl Strategy<Value = Column> {
    prop_oneof![
        Just(Column::Prioritized),
        Just(Column::Designing),
        Just(Column::Coding),
        Just(Column::Reviewing),
        Just(Column::Auditing),
        Just(Column::Deploying),
    ]
}

/// Map a Column to the corresponding Stage for task construction.
fn column_to_stage(col: Column) -> Stage {
    col.into()
}

/// Generate a vec of (id_suffix, priority) pairs representing tasks in the same column.
/// Priorities are u32 values; IDs are unique per test case.
fn arb_task_set() -> impl Strategy<Value = Vec<(u16, u32)>> {
    // 1..20 tasks, each with a unique index and arbitrary priority
    prop::collection::vec((any::<u16>(), any::<u32>()), 1..20)
}

proptest! {
    #[test]
    fn agent_pick_is_lowest_priority(
        column in arb_work_column(),
        task_entries in arb_task_set(),
    ) {
        let stage = column_to_stage(column);

        // Build tasks with unique IDs and the given priorities, all in the same column
        let tasks: Vec<Task> = task_entries
            .iter()
            .enumerate()
            .map(|(i, &(suffix, priority))| {
                let id = format!("task-{i}-{suffix}");
                let mut t = Task::new(&id, "epic-1", format!("Task {id}"));
                t.stage = stage.clone();
                t.priority = priority;
                t
            })
            .collect();

        // Determine the expected minimum priority
        let min_priority = tasks.iter().map(|t| t.priority).min().unwrap();

        // Build the board (sorts by priority within each column)
        let board = KanbanBoard::from_tasks(tasks);

        // Agent pick: first element of tasks_in(column)
        let task_ids = board.tasks_in(column);
        prop_assert!(!task_ids.is_empty(), "column should not be empty");

        let picked_id = &task_ids[0];
        let picked_task = board.task(picked_id).unwrap();

        // The picked task must have the lowest priority number
        prop_assert_eq!(
            picked_task.priority,
            min_priority,
            "agent should pick task with lowest priority number ({}), but picked task '{}' with priority {}",
            min_priority,
            picked_id,
            picked_task.priority,
        );
    }
}
