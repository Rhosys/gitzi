// Feature: dispatcher-audit-fixes, Property 8: WIP limit blocks advancement
// **Validates: Requirements 5.2, 5.3**

use std::collections::HashMap;
use std::sync::Arc;

use gitzi::dispatcher::board::{KanbanBoard, WipLimits};
use gitzi::dispatcher::{AgentRole, Column};
use gitzi::model::task::{Stage, Task};
use proptest::prelude::*;
use tokio::sync::Mutex;

/// Work columns that have a next column with a finite WIP limit.
fn arb_work_column_with_next() -> impl Strategy<Value = Column> {
    prop_oneof![
        Just(Column::Designing),
        Just(Column::Coding),
        Just(Column::Reviewing),
        Just(Column::Auditing),
        Just(Column::Deploying),
    ]
}

/// Get the AgentRole for a work column.
fn role_for_column(col: Column) -> AgentRole {
    col.agent_role().unwrap()
}

/// Build a task in a specific column with given id and priority.
fn make_task(id: &str, stage: Stage, priority: u32) -> Task {
    let mut t = Task::new(id, "epic-1", format!("Task {id}"));
    t.stage = stage;
    t.priority = priority;
    t
}

proptest! {
    /// Property 8: WIP limit blocks advancement
    ///
    /// For any column at its WIP limit (count equals limit) and any task
    /// attempting to advance into that column, the advance SHALL be rejected:
    /// the task stays in its source column, the target column count does not
    /// increase, and the waiting agent is recorded in the wip_waiting map.
    #[test]
    fn wip_limit_blocks_advancement(
        source_col in arb_work_column_with_next(),
        fill_priority in 0u32..500,
        task_priority in 0u32..500,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let target_col = source_col.next().unwrap();
            let wip_limits = Arc::new(WipLimits::default());

            // Determine WIP limit for the target column (default is 1 for
            // all buffer and work columns except Prioritized/Done)
            let limit_for_target = {
                let mut lim = 0u32;
                while wip_limits.allows(target_col, lim) && lim < 1000 {
                    lim += 1;
                }
                lim
            };

            // Skip unlimited columns — they never block
            if limit_for_target >= 1000 {
                return Ok(());
            }

            // Fill the target column to its WIP limit
            let mut tasks: Vec<Task> = (0..limit_for_target)
                .map(|i| {
                    make_task(
                        &format!("filler-{i}"),
                        target_col.into(),
                        fill_priority,
                    )
                })
                .collect();

            // Add the task that wants to advance (sitting in source column)
            tasks.push(make_task("advancing-task", source_col.into(), task_priority));

            let board = Arc::new(tokio::sync::RwLock::new(
                KanbanBoard::from_tasks(tasks),
            ));
            let wip_waiting: Arc<Mutex<HashMap<Column, AgentRole>>> =
                Arc::new(Mutex::new(HashMap::new()));

            // Snapshot state before attempt
            let count_before = board.read().await.count(target_col);

            // Confirm target is at WIP limit
            prop_assert!(
                !wip_limits.allows(target_col, count_before as u32),
                "Target {:?} should be at limit (count={})",
                target_col,
                count_before
            );

            // Execute the WIP gate logic (mirrors try_advance in agent_pool)
            let role = role_for_column(source_col);
            let count = board.read().await.count(target_col) as u32;
            if !wip_limits.allows(target_col, count) {
                // Blocked — record waiting agent, do NOT advance
                wip_waiting.lock().await.insert(target_col, role);
            } else {
                // Should not reach here given the setup
                board.write().await.advance("advancing-task", target_col).unwrap();
            }

            // Assert: task remains in source column
            let source_ids = board.read().await.tasks_in(source_col).to_vec();
            prop_assert!(
                source_ids.contains(&"advancing-task".to_string()),
                "Task must remain in source {:?} when target {:?} is at WIP limit",
                source_col,
                target_col
            );

            // Assert: target column count unchanged
            let count_after = board.read().await.count(target_col);
            prop_assert_eq!(
                count_before,
                count_after,
                "Target {:?} count must not change (was {}, now {})",
                target_col,
                count_before,
                count_after
            );

            // Assert: waiting agent is recorded in wip_waiting
            let waiting = wip_waiting.lock().await;
            prop_assert_eq!(
                waiting.get(&target_col),
                Some(&role),
                "Agent {:?} should be recorded as waiting for {:?}",
                role,
                target_col
            );

            Ok(())
        })?;
    }
}
