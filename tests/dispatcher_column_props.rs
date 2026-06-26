// Feature: event-driven-dispatcher, Property 1: Board construction preserves task-to-column mapping
// Feature: event-driven-dispatcher, Property 2: Column priority ordering invariant
// **Validates: Requirements 2.1, 2.2, 3.2**

use gitzi::dispatcher::Column;
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

/// Strategy that generates a task with a given stage and arbitrary priority.
fn arb_task_with_stage(stage: Stage) -> Task {
    let mut task = Task::new("test-id", "test-epic", "test-title");
    task.stage = stage;
    task
}

proptest! {
    /// Property 1: Board construction preserves task-to-column mapping
    ///
    /// For any task with a valid stage, the stage's `to_column()` mapping produces
    /// a Column that exists in `Column::all()`. This guarantees that when a KanbanBoard
    /// is constructed, every task maps to a valid column without loss.
    #[test]
    fn task_stage_maps_to_valid_column(stage in arb_stage()) {
        let task = arb_task_with_stage(stage.clone());
        let col = task.stage.to_column();

        // The column must exist in the canonical column list
        prop_assert!(
            Column::all().contains(&col),
            "Stage {:?} mapped to column {:?} which is not in Column::all()",
            stage,
            col
        );
    }

    /// Property 1 (extended): For column-aligned stages, `Stage::from(col)` round-trips
    /// through `to_column()` back to the same column.
    #[test]
    fn column_aligned_stage_roundtrips(col_idx in 0usize..11) {
        let col = Column::all()[col_idx];
        let stage: Stage = col.into();
        let mapped_col = stage.to_column();
        prop_assert_eq!(
            mapped_col, col,
            "Stage::from({:?}).to_column() returned {:?}, expected {:?}",
            col, mapped_col, col
        );
    }

    /// Property 1 (set preservation): For any collection of tasks with random stages,
    /// every task maps to exactly one column and no task is lost.
    #[test]
    fn all_tasks_map_to_columns_without_loss(
        stages in prop::collection::vec(arb_stage(), 1..50)
    ) {
        let tasks: Vec<Task> = stages.iter().enumerate().map(|(i, s)| {
            let mut t = Task::new(format!("task-{i}"), "epic", "title");
            t.stage = s.clone();
            t
        }).collect();

        // Every task must produce a valid column
        let mut mapped_count = 0;
        for task in &tasks {
            let col = task.stage.to_column();
            prop_assert!(Column::all().contains(&col));
            mapped_count += 1;
        }
        prop_assert_eq!(mapped_count, tasks.len(), "Some tasks were lost in mapping");
    }

    /// Property 2: Column priority ordering invariant
    ///
    /// For any two adjacent columns in `Column::all()`:
    /// - `next()` of the earlier equals the later
    /// - `prev()` of the later equals the earlier
    /// This proves the ordering forms a strict total order (a chain).
    #[test]
    fn adjacent_columns_linked_by_next_prev(idx in 0usize..10) {
        let all = Column::all();
        let earlier = all[idx];
        let later = all[idx + 1];

        // next() of earlier must point to later
        prop_assert_eq!(
            earlier.next(),
            Some(later),
            "{:?}.next() should be {:?}",
            earlier,
            later
        );

        // prev() of later must point to earlier
        prop_assert_eq!(
            later.prev(),
            Some(earlier),
            "{:?}.prev() should be {:?}",
            later,
            earlier
        );
    }

    /// Property 2 (boundary): The first column has no prev, the last has no next.
    #[test]
    fn boundary_columns_have_no_neighbors(_dummy in 0u8..1) {
        let all = Column::all();
        let first = all[0];
        let last = all[all.len() - 1];

        prop_assert_eq!(first.prev(), None, "First column should have no prev");
        prop_assert_eq!(last.next(), None, "Last column should have no next");
    }

    /// Property 2 (total order): All columns are reachable by repeated `next()` from Prioritized.
    #[test]
    fn all_columns_reachable_from_first(_dummy in 0u8..1) {
        let all = Column::all();
        let mut current = Some(all[0]);
        let mut visited = Vec::new();

        while let Some(col) = current {
            visited.push(col);
            current = col.next();
        }

        prop_assert_eq!(
            visited.len(),
            all.len(),
            "Traversing next() from first should visit all {} columns, got {}",
            all.len(),
            visited.len()
        );

        for (i, col) in visited.iter().enumerate() {
            prop_assert_eq!(*col, all[i], "Column order mismatch at index {}", i);
        }
    }
}
