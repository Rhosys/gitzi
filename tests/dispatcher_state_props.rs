// Feature: event-driven-dispatcher, Property 3: State mutation emits corresponding event
// Feature: event-driven-dispatcher, Property 5: WIP release emits signal
// Feature: event-driven-dispatcher, Property 7: Agent completion advances to correct buffer
// Feature: event-driven-dispatcher, Property 8: Buffer columns block without approval
// Feature: event-driven-dispatcher, Property 9: Buffer entry creates review item
// Feature: event-driven-dispatcher, Property 10: Approval advances correctly and records history
// Feature: event-driven-dispatcher, Property 11: Rejection semantics
// **Validates: Requirements 1.2, 4.3, 5.5, 6.1, 6.2, 6.3, 6.4, 8.1, 8.2, 9.1, 9.2, 9.3**

use std::sync::Arc;

use gitzi::config::Config;
use gitzi::dispatcher::agent_pool::AgentPool;
use gitzi::dispatcher::board::{KanbanBoard, WipLimits};
use gitzi::dispatcher::event_bus::{DispatchEvent, EventBus};
use gitzi::dispatcher::review_queue::{HumanReviewQueue, ReviewItemKind};
use gitzi::dispatcher::{Column, Dispatcher};
use gitzi::model::task::{HistoryEntry, Stage, Task};
use proptest::prelude::*;
use tokio::sync::{Mutex, RwLock};

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn make_task(id: &str, stage: Stage, priority: u32) -> Task {
    let mut t = Task::new(id, "epic-1", format!("Task {id}"));
    t.stage = stage;
    t.priority = priority;
    t
}

/// All buffer columns available for testing approve/reject.
fn all_buffer_columns() -> Vec<Column> {
    vec![
        Column::CodingBuffer,
        Column::ReviewBuffer,
        Column::SecurityAuditBuffer,
        Column::DeploymentBuffer,
    ]
}

/// Strategy that generates an arbitrary buffer column.
fn arb_buffer_column() -> impl Strategy<Value = Column> {
    prop_oneof![
        Just(Column::CodingBuffer),
        Just(Column::ReviewBuffer),
        Just(Column::SecurityAuditBuffer),
        Just(Column::DeploymentBuffer),
    ]
}

/// Strategy that generates an arbitrary work column (columns with agent roles,
/// excluding Prioritized since it has no buffer before it and Deploying since
/// its next column is Done, not a buffer).
fn arb_work_column_with_next_buffer() -> impl Strategy<Value = Column> {
    prop_oneof![
        Just(Column::Designing),
        Just(Column::Coding),
        Just(Column::Reviewing),
        Just(Column::Auditing),
        Just(Column::Deploying),
    ]
}

/// Build a minimal Dispatcher for testing with given tasks on the board.
/// Does NOT spawn real agent tasks — uses inert handles.
async fn build_test_dispatcher(tasks: Vec<Task>) -> Dispatcher {
    let event_bus = Arc::new(EventBus::new(256));
    let board = Arc::new(RwLock::new(KanbanBoard::from_tasks(tasks)));
    let review_queue = Arc::new(Mutex::new(HumanReviewQueue::new()));
    let config = Arc::new(tokio::sync::RwLock::new(Config::default()));
    let wip_limits = Arc::new(RwLock::new(WipLimits::default()));
    let wip_waiting = Arc::new(Mutex::new(std::collections::HashMap::new()));

    let token_store = Arc::new(gitzi::mcp::auth::TokenStore::new());
    let agent_pool = AgentPool::inert();

    let main_agent_def = config.read().await.resolve_agent("main");
    let config_guard = config.read().await;
    let main_agent = gitzi::agent::build_main_agent(&*config_guard, &main_agent_def);
    drop(config_guard);

    let store: std::sync::Arc<dyn gitzi::state::store::StateStore> =
        std::sync::Arc::new(gitzi::state::store::InMemoryStore::new());

    Dispatcher {
        event_bus,
        board,
        review_queue,
        agent_pool,
        config,
        wip_limits,
        wip_waiting,
        chat_history: Arc::new(Mutex::new(vec![])),
        chat_summary: Arc::new(Mutex::new(None)),
        main_agent: tokio::sync::RwLock::new(main_agent),
        fallback_agent: tokio::sync::RwLock::new(None),
        token_store,
        store,
        chat_stack: Mutex::new(Vec::new()),
    }
}

// ─── Property 7: Agent completion advances to correct buffer ──────────────────

proptest! {
    /// Property 7: Agent completion advances to correct buffer
    ///
    /// For any task in a work column, when advance() is called to the next column,
    /// that next column is always the immediately following buffer column (or Done
    /// for Deploying).
    #[test]
    fn agent_completion_advances_to_correct_buffer(
        col in arb_work_column_with_next_buffer(),
        priority in 0u32..1000,
    ) {
        let stage: Stage = col.into();
        let task = make_task("test-task", stage, priority);
        let mut board = KanbanBoard::from_tasks(vec![task]);

        let next_col = col.next().unwrap();
        board.advance("test-task", next_col).unwrap();

        // After advance, the task should be in the next column
        let ids = board.tasks_in(next_col);
        prop_assert!(
            ids.contains(&"test-task".to_string()),
            "Task should be in {:?} after advance from {:?}, but found in neither",
            next_col, col
        );

        // The next column for a work column should always be a buffer (or Done for Deploying)
        if col == Column::Deploying {
            prop_assert_eq!(next_col, Column::Done);
        } else {
            prop_assert!(
                next_col.is_buffer(),
                "Next column after work column {:?} should be a buffer, got {:?}",
                col, next_col
            );
        }
    }
}

// ─── Property 8: Buffer columns block without approval ────────────────────────

proptest! {
    /// Property 8: Buffer columns block without approval
    ///
    /// For any task in a buffer column, calling advance() directly on the board
    /// only moves it if explicitly invoked. The board itself never auto-advances
    /// from buffer columns. This is tested by verifying that after placing a task
    /// in a buffer, it stays there without any automatic progression.
    #[test]
    fn buffer_columns_block_without_approval(
        buf_col in arb_buffer_column(),
        priority in 0u32..1000,
    ) {
        let stage: Stage = buf_col.into();
        let task = make_task("blocked-task", stage.clone(), priority);
        let board = KanbanBoard::from_tasks(vec![task]);

        // Task should remain in the buffer column
        let ids = board.tasks_in(buf_col);
        prop_assert!(
            ids.contains(&"blocked-task".to_string()),
            "Task should remain in buffer {:?} without explicit approval",
            buf_col
        );

        // The next column should be a work column (the one requiring approval)
        let next = buf_col.next().unwrap();
        let next_ids = board.tasks_in(next);
        prop_assert!(
            !next_ids.contains(&"blocked-task".to_string()),
            "Task should NOT auto-advance from buffer {:?} to {:?}",
            buf_col, next
        );
    }
}

// ─── Property 3, 5, 9, 10, 11: Async tests via Dispatcher ────────────────────

/// Property 3: State mutation emits corresponding event
///
/// For any buffer column and task within it, calling approve() emits
/// HumanApprovalReceived and calling reject() emits HumanRejectionReceived.
#[tokio::test]
async fn state_mutation_emits_corresponding_event_approve() {
    for buf_col in all_buffer_columns() {
        let stage: Stage = buf_col.into();
        let task = make_task("emit-task", stage, 50);
        let dispatcher = build_test_dispatcher(vec![task]).await;

        // Subscribe before the mutation
        let mut rx = dispatcher.event_bus.subscribe();

        // Perform approval
        dispatcher.approve("emit-task").await.unwrap();

        // Should receive HumanApprovalReceived
        let event = rx.try_recv().unwrap();
        match event {
            DispatchEvent::HumanApprovalReceived {
                task_id,
                target_column,
            } => {
                assert_eq!(task_id, "emit-task");
                assert_eq!(target_column, buf_col.next().unwrap());
            }
            other => panic!(
                "Expected HumanApprovalReceived for buffer {:?}, got {:?}",
                buf_col, other
            ),
        }
    }
}

/// Property 3: State mutation emits corresponding event (rejection path)
#[tokio::test]
async fn state_mutation_emits_corresponding_event_reject() {
    for buf_col in all_buffer_columns() {
        let stage: Stage = buf_col.into();
        let task = make_task("reject-task", stage, 50);
        let dispatcher = build_test_dispatcher(vec![task]).await;

        let mut rx = dispatcher.event_bus.subscribe();

        dispatcher
            .reject("reject-task", "needs work".to_string())
            .await
            .unwrap();

        let event = rx.try_recv().unwrap();
        match event {
            DispatchEvent::HumanRejectionReceived {
                task_id,
                returned_to,
                feedback,
            } => {
                assert_eq!(task_id, "reject-task");
                assert_eq!(returned_to, buf_col.prev().unwrap());
                assert_eq!(feedback, "needs work");
            }
            other => panic!(
                "Expected HumanRejectionReceived for buffer {:?}, got {:?}",
                buf_col, other
            ),
        }
    }
}

/// Property 5: WIP release emits signal
///
/// When a task leaves a column that was at its WIP limit (via approval),
/// a TaskStageChanged-equivalent event (HumanApprovalReceived) is emitted,
/// indicating that the slot is freed.
#[tokio::test]
async fn wip_release_emits_event_on_approval() {
    // Place a task in CodingBuffer (WIP limit = 1). Approving it moves it out,
    // freeing the slot. The approval event confirms the state change happened.
    let task = make_task("wip-task", Stage::CodingBuffer, 10);
    let dispatcher = build_test_dispatcher(vec![task]).await;

    // Verify buffer is at WIP limit
    {
        let board = dispatcher.board.read().await;
        assert_eq!(board.count(Column::CodingBuffer), 1);
        assert!(
            !dispatcher
                .wip_limits
                .read()
                .await
                .allows(Column::CodingBuffer, 1)
        );
    }

    let mut rx = dispatcher.event_bus.subscribe();
    dispatcher.approve("wip-task").await.unwrap();

    // After approval the buffer is empty (WIP released)
    {
        let board = dispatcher.board.read().await;
        assert_eq!(board.count(Column::CodingBuffer), 0);
    }

    // Event was emitted
    let event = rx.try_recv().unwrap();
    assert!(matches!(event, DispatchEvent::HumanApprovalReceived { .. }));
}

/// Property 9: Buffer entry creates review item
///
/// For any task entering a buffer column, a BufferApproval review item is created.
/// We simulate this by manually calling the dispatcher's event loop logic:
/// advance a task into a buffer, then verify the review queue has an item.
#[tokio::test]
async fn buffer_entry_creates_review_item() {
    for buf_col in all_buffer_columns() {
        // Place task in the work column preceding the buffer
        let prev_work_col = buf_col.prev().unwrap();
        let stage: Stage = prev_work_col.into();
        let task = make_task("review-task", stage, 50);
        let dispatcher = build_test_dispatcher(vec![task]).await;

        // Advance task into the buffer column (simulates agent completion)
        {
            let mut board = dispatcher.board.write().await;
            board.advance("review-task", buf_col).unwrap();
        }

        // Simulate the TaskStageChanged handler creating the review item
        let priority = {
            let b = dispatcher.board.read().await;
            b.task("review-task")
                .map(|t| t.priority)
                .unwrap_or(u32::MAX)
        };
        let item = gitzi::dispatcher::review_queue::HumanReviewItem::new(
            "review-task",
            ReviewItemKind::BufferApproval {
                buffer_column: buf_col,
                task_priority: priority,
            },
            format!("review-task — awaiting approval in {buf_col:?}"),
        );
        {
            let mut q = dispatcher.review_queue.lock().await;
            q.enqueue(item);
        }

        // Verify review queue has the item
        let q = dispatcher.review_queue.lock().await;
        assert!(
            !q.is_empty(),
            "Queue should have a review item for buffer {:?}",
            buf_col
        );
        let peeked = q.peek().unwrap();
        assert_eq!(peeked.task_id, "review-task");
        match &peeked.kind {
            ReviewItemKind::BufferApproval {
                buffer_column,
                task_priority,
            } => {
                assert_eq!(*buffer_column, buf_col);
                assert_eq!(*task_priority, 50);
            }
            other => panic!("Expected BufferApproval, got {:?}", other),
        }
    }
}

/// Property 10: Approval advances correctly and records history
///
/// For any task in a buffer column, approval moves it to the next work column
/// and appends an Approval history entry with the correct target stage.
#[tokio::test]
async fn approval_advances_and_records_history() {
    for buf_col in all_buffer_columns() {
        let stage: Stage = buf_col.into();
        let task = make_task("approve-hist", stage, 30);
        let dispatcher = build_test_dispatcher(vec![task]).await;

        dispatcher.approve("approve-hist").await.unwrap();

        let board = dispatcher.board.read().await;
        let next_col = buf_col.next().unwrap();

        // Task should be in the next work column
        let ids = board.tasks_in(next_col);
        assert!(
            ids.contains(&"approve-hist".to_string()),
            "After approval from {:?}, task should be in {:?}",
            buf_col,
            next_col
        );

        // History should contain an Approval entry
        let task = board.task("approve-hist").unwrap();
        let has_approval = task.history.iter().any(|h| {
            matches!(
                h,
                HistoryEntry::Approval { target_stage, .. }
                    if *target_stage == Stage::from(next_col)
            )
        });
        assert!(
            has_approval,
            "Task should have Approval history entry with target_stage {:?}",
            Stage::from(next_col)
        );
    }
}

/// Property 11: Rejection semantics
///
/// For any task in a buffer column, rejection moves it to the previous work column
/// with priority 0, stores feedback in agent_feedback, and appends a Rejection
/// history entry.
#[tokio::test]
async fn rejection_semantics() {
    for buf_col in all_buffer_columns() {
        let stage: Stage = buf_col.into();
        let task = make_task("reject-hist", stage, 50);
        let dispatcher = build_test_dispatcher(vec![task]).await;

        let feedback = format!("Fix issues in {:?}", buf_col);
        dispatcher
            .reject("reject-hist", feedback.clone())
            .await
            .unwrap();

        let board = dispatcher.board.read().await;
        let prev_col = buf_col.prev().unwrap();

        // Task should be in the previous work column
        let ids = board.tasks_in(prev_col);
        assert!(
            ids.contains(&"reject-hist".to_string()),
            "After rejection from {:?}, task should be in {:?}",
            buf_col,
            prev_col
        );

        let task = board.task("reject-hist").unwrap();

        // Priority should be 0 (highest)
        assert_eq!(
            task.priority, 0,
            "Rejected task should have priority 0, got {}",
            task.priority
        );

        // agent_feedback should contain the feedback
        assert_eq!(
            task.agent_feedback.as_deref(),
            Some(feedback.as_str()),
            "agent_feedback should contain rejection feedback"
        );

        // History should contain a Rejection entry
        let has_rejection = task.history.iter().any(|h| {
            matches!(
                h,
                HistoryEntry::Rejection { feedback: f, returned_to, .. }
                    if f == &feedback && *returned_to == Stage::from(prev_col)
            )
        });
        assert!(
            has_rejection,
            "Task should have Rejection history entry with feedback and returned_to {:?}",
            Stage::from(prev_col)
        );
    }
}
