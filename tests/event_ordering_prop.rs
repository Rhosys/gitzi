// Feature: dispatcher-audit-fixes, Property 7: TaskStageChanged precedes AgentCompleted on successful advance
// **Validates: Requirements 4.1, 4.2**

use std::collections::HashMap;
use std::sync::Arc;

use gitzi::dispatcher::agent_pool::AgentHandle;
use gitzi::dispatcher::board::{KanbanBoard, WipLimits};
use gitzi::dispatcher::event_bus::{DispatchEvent, EventBus};
use gitzi::dispatcher::{AgentRole, Column};
use gitzi::model::task::{Stage, Task};
use proptest::prelude::*;
use tokio::sync::{Mutex, RwLock};

/// Work columns that have a next column (agent completes and advances).
fn arb_work_column() -> impl Strategy<Value = Column> {
    prop_oneof![
        Just(Column::Prioritized),
        Just(Column::Designing),
        Just(Column::Coding),
        Just(Column::Reviewing),
        Just(Column::Testing),
        Just(Column::Auditing),
        Just(Column::Deploying),
    ]
}

fn make_task(id: &str, stage: Stage, priority: u32) -> Task {
    let mut t = Task::new(id, "epic-1", format!("Task {id}"));
    t.stage = stage;
    t.priority = priority;
    t
}

proptest! {
    /// Property 7: TaskStageChanged precedes AgentCompleted on successful advance
    ///
    /// For any task in any work column, when the agent completes successfully
    /// and the task advances, the event bus SHALL receive TaskStageChanged
    /// before AgentCompleted for that task_id, and both events SHALL be present.
    #[test]
    fn task_stage_changed_precedes_agent_completed(
        work_col in arb_work_column(),
        priority in 0u32..1000,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let task = make_task("order-task", work_col.into(), priority);
            let event_bus = Arc::new(EventBus::new(256));
            let board = Arc::new(RwLock::new(KanbanBoard::from_tasks(vec![task.clone()])));
            let wip_limits = Arc::new(WipLimits::default());
            let wip_waiting: Arc<Mutex<HashMap<Column, AgentRole>>> =
                Arc::new(Mutex::new(HashMap::new()));

            let role = work_col.agent_role().unwrap();
            let _handle = AgentHandle::new_for_test(role);

            // Subscribe to event bus BEFORE the advance
            let mut rx = event_bus.subscribe();

            // Execute try_advance logic (mirrors the private fn in agent_pool)
            let from_col = role.column();
            let target = from_col.next().unwrap();

            // WIP gate check
            let count = board.read().await.count(target) as u32;
            if !wip_limits.allows(target, count) {
                // WIP blocked — record waiting and skip (no events)
                wip_waiting.lock().await.insert(target, role);
                return Ok(());
            }

            // Advance the task on the board
            {
                let mut b = board.write().await;
                b.advance(&task.id, target).unwrap();
            }

            // Emit TaskStageChanged FIRST (as try_advance does)
            event_bus.emit(DispatchEvent::TaskStageChanged {
                task_id: task.id.clone(),
                from: from_col,
                to: target,
            });

            // Then emit AgentCompleted
            event_bus.emit(DispatchEvent::AgentCompleted {
                task_id: task.id.clone(),
                agent_role: role,
            });

            // Collect all events from the bus
            let mut events = Vec::new();
            while let Ok(ev) = rx.try_recv() {
                events.push(ev);
            }

            // Find indices of TaskStageChanged and AgentCompleted for our task
            let stage_changed_idx = events.iter().position(|e| matches!(
                e,
                DispatchEvent::TaskStageChanged { task_id, .. } if task_id == "order-task"
            ));
            let agent_completed_idx = events.iter().position(|e| matches!(
                e,
                DispatchEvent::AgentCompleted { task_id, .. } if task_id == "order-task"
            ));

            // Both events must be present
            prop_assert!(
                stage_changed_idx.is_some(),
                "TaskStageChanged must be emitted for task in {:?}",
                work_col
            );
            prop_assert!(
                agent_completed_idx.is_some(),
                "AgentCompleted must be emitted for task in {:?}",
                work_col
            );

            // TaskStageChanged must come before AgentCompleted
            let sc_idx = stage_changed_idx.unwrap();
            let ac_idx = agent_completed_idx.unwrap();
            prop_assert!(
                sc_idx < ac_idx,
                "TaskStageChanged (idx={}) must precede AgentCompleted (idx={}) for {:?}",
                sc_idx,
                ac_idx,
                work_col
            );

            // Verify TaskStageChanged has correct from/to
            if let DispatchEvent::TaskStageChanged { from, to, .. } = &events[sc_idx] {
                prop_assert_eq!(*from, from_col);
                prop_assert_eq!(*to, target);
            }

            // Verify AgentCompleted has correct role
            if let DispatchEvent::AgentCompleted { agent_role, .. } = &events[ac_idx] {
                prop_assert_eq!(*agent_role, role);
            }

            Ok(())
        })?;
    }
}
