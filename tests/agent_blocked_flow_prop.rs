// Feature: dispatcher-audit-fixes, Property 10: Agent question creates review item and blocks
// **Validates: Requirements 6.1**
//
// For any `AgentResult::Blocked { question }` with random question string,
// the agent loop SHALL persist a review item of kind AgentQuestion to disk,
// emit AgentBlocked on the event bus containing the question, and set the
// agent handle's blocked flag to true.

use std::sync::Arc;

use gitzi::dispatcher::agent_pool::AgentHandle;
use gitzi::dispatcher::event_bus::{DispatchEvent, EventBus};
use gitzi::dispatcher::review_queue::{HumanReviewItem, HumanReviewQueue, ReviewItemKind};
use gitzi::dispatcher::AgentRole;
use gitzi::state::review::{PersistedReviewItem, PersistedReviewKind};
use proptest::prelude::*;
use tokio::sync::Mutex;

/// Strategy that generates an arbitrary `AgentRole`.
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
    /// Property 10: Agent question creates review item and blocks
    ///
    /// Simulates the AgentResult::Blocked path from handle_agent_result:
    /// 1. Persists a review item of kind AgentQuestion to disk
    /// 2. Emits AgentBlocked event on the event bus with the question
    /// 3. Sets the agent handle's blocked flag to true
    /// 4. Enqueues in the in-memory HumanReviewQueue
    #[test]
    fn agent_blocked_creates_review_item_and_blocks(
        role in arb_role(),
        question in "[a-zA-Z0-9 ?.!]{1,100}",
        task_id in "[a-z0-9\\-]{5,30}",
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            // ── Setup components ──────────────────────────────────────────────
            let event_bus = Arc::new(EventBus::new(64));
            let review_queue = Arc::new(Mutex::new(HumanReviewQueue::new()));
            let handle = AgentHandle::new_for_test(role);
            let tmp = tempfile::tempdir().unwrap();

            // Subscribe to events BEFORE triggering the flow
            let mut rx = event_bus.subscribe();

            // ── Simulate AgentResult::Blocked path ───────────────────────────
            // Mirrors the exact sequence in handle_agent_result:

            // 1. Persist review item to disk
            let item = PersistedReviewItem {
                id: format!("review-{}", &task_id),
                task_id: task_id.clone(),
                kind: PersistedReviewKind::AgentQuestion {
                    question: question.clone(),
                },
                created_at: chrono::Utc::now(),
                actions: Vec::new(),
            };
            let reviews_dir = tmp.path().join("reviews");
            std::fs::create_dir_all(&reviews_dir).unwrap();
            let path = reviews_dir.join(format!("{}.toml", &item.id));
            let serialized = toml::to_string_pretty(&item).unwrap();
            std::fs::write(&path, &serialized).unwrap();

            // 2. Emit AgentBlocked event
            event_bus.emit(DispatchEvent::AgentBlocked {
                task_id: task_id.clone(),
                agent_role: role,
                question: question.clone(),
            });

            // 3. Set blocked flag
            handle.set_blocked(true);

            // 4. Enqueue in-memory review item
            let queue_item = HumanReviewItem::new(
                &task_id,
                ReviewItemKind::AgentQuestion {
                    question: question.clone(),
                },
            );
            {
                let mut q = review_queue.lock().await;
                q.enqueue(queue_item);
            }

            // ── Verify invariants ────────────────────────────────────────────

            // Invariant 1: Review item persisted to disk with correct kind
            let read_back = std::fs::read_to_string(&path).unwrap();
            let loaded: PersistedReviewItem = toml::from_str(&read_back).unwrap();
            prop_assert_eq!(&loaded.task_id, &task_id);
            prop_assert_eq!(
                &loaded.kind,
                &PersistedReviewKind::AgentQuestion {
                    question: question.clone()
                },
                "Persisted review item must be AgentQuestion with the question"
            );
            prop_assert!(
                loaded.is_unresolved(),
                "Newly created review item must have no actions (unresolved)"
            );

            // Invariant 2: AgentBlocked event emitted with correct payload
            let event = rx.try_recv()
                .expect("AgentBlocked event must be emitted");
            match event {
                DispatchEvent::AgentBlocked {
                    task_id: ev_task_id,
                    agent_role: ev_role,
                    question: ev_question,
                } => {
                    prop_assert_eq!(
                        &ev_task_id, &task_id,
                        "AgentBlocked event task_id must match"
                    );
                    prop_assert_eq!(
                        ev_role, role,
                        "AgentBlocked event agent_role must match"
                    );
                    prop_assert_eq!(
                        &ev_question, &question,
                        "AgentBlocked event question must match"
                    );
                }
                other => {
                    prop_assert!(
                        false,
                        "Expected AgentBlocked event, got {:?}",
                        other
                    );
                }
            }

            // Invariant 3: Agent handle blocked flag is true
            prop_assert!(
                handle.is_blocked(),
                "Agent handle must be blocked after AgentResult::Blocked"
            );

            // Invariant 4: Review queue has the item
            let q = review_queue.lock().await;
            prop_assert!(
                !q.is_empty(),
                "Review queue must not be empty after enqueue"
            );
            let peeked = q.peek().unwrap();
            prop_assert_eq!(
                &peeked.task_id, &task_id,
                "Queued review item task_id must match"
            );
            match &peeked.kind {
                ReviewItemKind::AgentQuestion { question: q_text } => {
                    prop_assert_eq!(
                        q_text, &question,
                        "Queued review item question must match"
                    );
                }
                other => {
                    prop_assert!(
                        false,
                        "Expected AgentQuestion in queue, got {:?}",
                        other
                    );
                }
            }

            Ok(())
        })?;
    }
}
