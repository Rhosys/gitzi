// Feature: dispatcher-audit-fixes, Property 9: WIP release re-signals waiting agent
// **Validates: Requirements 5.4, 5.5**

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gitzi::config::Config;
use gitzi::dispatcher::agent_pool::AgentPool;
use gitzi::dispatcher::board::{KanbanBoard, WipLimits};
use gitzi::dispatcher::event_bus::{DispatchEvent, EventBus};
use gitzi::dispatcher::review_queue::HumanReviewQueue;
use gitzi::dispatcher::{AgentRole, Column, Dispatcher};
use proptest::prelude::*;
use tokio::sync::{Mutex, RwLock};

/// Strategy: work columns where WIP waiting can be recorded.
fn arb_work_column() -> impl Strategy<Value = Column> {
    prop_oneof![
        Just(Column::Designing),
        Just(Column::Coding),
        Just(Column::Reviewing),
        Just(Column::Testing),
        Just(Column::Auditing),
        Just(Column::Deploying),
    ]
}

/// Strategy: arbitrary agent role.
fn arb_role() -> impl Strategy<Value = AgentRole> {
    prop_oneof![
        Just(AgentRole::Prioritizer),
        Just(AgentRole::Designer),
        Just(AgentRole::Coder),
        Just(AgentRole::Reviewer),
        Just(AgentRole::Tester),
        Just(AgentRole::Auditor),
        Just(AgentRole::Infrarian),
    ]
}

proptest! {
    /// Property 9: WIP release re-signals waiting agent
    ///
    /// For any column that was at its WIP limit with a recorded waiting agent,
    /// when a task leaves that column (TaskStageChanged event with `from` = column),
    /// the dispatcher SHALL signal the waiting agent's role exactly once and remove
    /// the waiting record for that column.
    ///
    /// We test this by constructing the Dispatcher, spawning its run() loop,
    /// emitting a TaskStageChanged event, and verifying the wip_waiting map
    /// is cleared after event processing.
    #[test]
    fn wip_release_removes_waiting_record_and_signals(
        target_col in arb_work_column(),
        waiting_role in arb_role(),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            // Pre-condition: agent waiting to advance into target_col
            let mut waiting = HashMap::new();
            waiting.insert(target_col, waiting_role);

            let event_bus = Arc::new(EventBus::new(256));
            let board = Arc::new(RwLock::new(KanbanBoard::from_tasks(vec![])));
            let review_queue = Arc::new(Mutex::new(HumanReviewQueue::new()));
            let config = Arc::new(Config::default());
            let wip_limits = Arc::new(RwLock::new(WipLimits::default()));
            let wip_waiting = Arc::new(Mutex::new(waiting));

            let token_store = Arc::new(gitzi::mcp::auth::TokenStore::new());
            let agent_pool = AgentPool::inert();

            let main_agent_def = config.resolve_agent("main");
            let main_agent = gitzi::agent::build_main_agent(&main_agent_def);

            let store: std::sync::Arc<dyn gitzi::state::store::StateStore> =
                std::sync::Arc::new(gitzi::state::store::InMemoryStore::new());

            let dispatcher = Dispatcher {
                event_bus: Arc::clone(&event_bus),
                board,
                review_queue,
                agent_pool,
                config,
                wip_limits,
                wip_waiting: Arc::clone(&wip_waiting),
                chat_history: Arc::new(Mutex::new(vec![])),
                main_agent,
                token_store,
                store,
                chat_stack: Mutex::new(Vec::new()),
            };

            // Verify pre-condition: entry exists
            {
                let ww = wip_waiting.lock().await;
                assert_eq!(ww.get(&target_col), Some(&waiting_role));
            }

            // Spawn the run loop
            let run_handle = tokio::spawn(async move {
                let _ = dispatcher.run().await;
            });

            // Give the run loop time to subscribe to the event bus
            tokio::time::sleep(Duration::from_millis(10)).await;

            // Emit TaskStageChanged: a task leaves target_col
            let next_col = target_col.next().unwrap_or(Column::Done);
            event_bus.emit(DispatchEvent::TaskStageChanged {
                task_id: "leaving-task".to_string(),
                from: target_col,
                to: next_col,
            });

            // Allow the event to be processed
            tokio::time::sleep(Duration::from_millis(50)).await;

            // Verify: wip_waiting entry for target_col is removed
            {
                let ww = wip_waiting.lock().await;
                assert!(
                    !ww.contains_key(&target_col),
                    "wip_waiting should not contain {:?} after TaskStageChanged(from={:?}), but found {:?}",
                    target_col, target_col, ww.get(&target_col)
                );
            }

            // Clean up: drop the event bus to close the channel and stop run()
            drop(event_bus);
            let _ = tokio::time::timeout(Duration::from_millis(100), run_handle).await;
        });
    }

    /// Property 9 (inverse): When TaskStageChanged fires with a `from` column
    /// that has NO waiting agent, wip_waiting entries for other columns are preserved.
    #[test]
    fn wip_release_no_op_when_no_waiting_agent_for_column(
        from_col in arb_work_column(),
        other_col in arb_work_column(),
        other_role in arb_role(),
    ) {
        prop_assume!(from_col != other_col);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            // A waiting entry exists for other_col, NOT from_col
            let mut waiting = HashMap::new();
            waiting.insert(other_col, other_role);

            let event_bus = Arc::new(EventBus::new(256));
            let board = Arc::new(RwLock::new(KanbanBoard::from_tasks(vec![])));
            let review_queue = Arc::new(Mutex::new(HumanReviewQueue::new()));
            let config = Arc::new(Config::default());
            let wip_limits = Arc::new(RwLock::new(WipLimits::default()));
            let wip_waiting = Arc::new(Mutex::new(waiting));

            let token_store = Arc::new(gitzi::mcp::auth::TokenStore::new());
            let agent_pool = AgentPool::inert();

            let main_agent_def = config.resolve_agent("main");
            let main_agent = gitzi::agent::build_main_agent(&main_agent_def);

            let store: std::sync::Arc<dyn gitzi::state::store::StateStore> =
                std::sync::Arc::new(gitzi::state::store::InMemoryStore::new());

            let dispatcher = Dispatcher {
                event_bus: Arc::clone(&event_bus),
                board,
                review_queue,
                agent_pool,
                config,
                wip_limits,
                wip_waiting: Arc::clone(&wip_waiting),
                chat_history: Arc::new(Mutex::new(vec![])),
                main_agent,
                token_store,
                store,
                chat_stack: Mutex::new(Vec::new()),
            };

            // Spawn run loop
            let run_handle = tokio::spawn(async move {
                let _ = dispatcher.run().await;
            });

            tokio::time::sleep(Duration::from_millis(10)).await;

            // Emit event for from_col (which has no waiting agent)
            let next = from_col.next().unwrap_or(Column::Done);
            event_bus.emit(DispatchEvent::TaskStageChanged {
                task_id: "some-task".to_string(),
                from: from_col,
                to: next,
            });

            tokio::time::sleep(Duration::from_millis(50)).await;

            // Verify: other_col entry is preserved
            {
                let ww = wip_waiting.lock().await;
                assert_eq!(
                    ww.get(&other_col),
                    Some(&other_role),
                    "wip_waiting entry for {:?} should be preserved when event from={:?}",
                    other_col, from_col
                );
            }

            drop(event_bus);
            let _ = tokio::time::timeout(Duration::from_millis(100), run_handle).await;
        });
    }
}
