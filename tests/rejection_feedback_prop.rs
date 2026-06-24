// Feature: event-driven-dispatcher, Property 15: Rejection feedback propagates to agent context
// **Validates: Requirements 9.3, 9.4**

use std::sync::Arc;

use gitzi::config::Config;
use gitzi::dispatcher::agent_pool::AgentPool;
use gitzi::dispatcher::board::{KanbanBoard, WipLimits};
use gitzi::dispatcher::event_bus::EventBus;
use gitzi::dispatcher::review_queue::HumanReviewQueue;
use gitzi::dispatcher::{Column, Dispatcher};
use gitzi::model::task::{Stage, Task};
use proptest::prelude::*;
use tokio::sync::{Mutex, RwLock};

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn make_task(id: &str, stage: Stage, priority: u32) -> Task {
    let mut t = Task::new(id, "epic-1", format!("Task {id}"));
    t.stage = stage;
    t.priority = priority;
    t
}

/// Strategy that generates an arbitrary buffer column.
fn arb_buffer_column() -> impl Strategy<Value = Column> {
    prop_oneof![
        Just(Column::CodingBuffer),
        Just(Column::ReviewBuffer),
        Just(Column::TestBuffer),
        Just(Column::SecurityAuditBuffer),
        Just(Column::DeploymentBuffer),
    ]
}

/// Strategy that generates arbitrary non-empty feedback strings.
fn arb_feedback() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 .,!?;:'\"-]{1,200}".prop_filter("non-empty feedback", |s| !s.is_empty())
}

/// Build a minimal Dispatcher for testing with given tasks on the board.
async fn build_test_dispatcher(tasks: Vec<Task>) -> Dispatcher {
    let event_bus = Arc::new(EventBus::new(256));
    let board = Arc::new(RwLock::new(KanbanBoard::from_tasks(tasks)));
    let review_queue = Arc::new(Mutex::new(HumanReviewQueue::new()));
    let config = Arc::new(Config::default());
    let wip_limits = Arc::new(RwLock::new(WipLimits::default()));
    let wip_waiting = Arc::new(Mutex::new(std::collections::HashMap::new()));

    let token_store = Arc::new(gitzi::mcp::auth::TokenStore::new());
    let agent_pool = AgentPool::inert();

    let main_agent_def = config.resolve_agent("main");
    let main_agent = gitzi::agent::build_main_agent(&main_agent_def);

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
        main_agent,
        token_store,
        store,
        chat_stack: Mutex::new(Vec::new()),
    }
}

// ─── Property 15: Rejection feedback propagates to agent context ──────────────

proptest! {
    /// Property 15: Rejection feedback propagates to agent context
    ///
    /// For any task in a buffer column and any feedback string, after calling
    /// Dispatcher::reject(task_id, feedback), the task's agent_feedback field
    /// equals the feedback. This ensures rejection feedback propagates to the
    /// task object where the agent loop will pick it up.
    #[test]
    fn rejection_feedback_propagates_to_agent_context(
        buf_col in arb_buffer_column(),
        feedback in arb_feedback(),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let stage: Stage = buf_col.into();
            let task = make_task("feedback-task", stage, 50);
            let dispatcher = build_test_dispatcher(vec![task]).await;

            // Reject with the generated feedback
            dispatcher
                .reject("feedback-task", feedback.clone())
                .await
                .unwrap();

            // Verify agent_feedback is set to the exact feedback string
            let board = dispatcher.board.read().await;
            let task = board.task("feedback-task").unwrap();

            prop_assert_eq!(
                task.agent_feedback.as_deref(),
                Some(feedback.as_str()),
                "After reject(), task.agent_feedback should equal the feedback string"
            );

            Ok(())
        })?;
    }
}
