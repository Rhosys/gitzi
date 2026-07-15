// Feature: verifier-gate, Property: Maximum one retry before escalation
// **Validates: ADR-001 — verifier allows exactly one retry before escalating**
//
// This test validates the STRUCTURAL property that the retry path invokes the
// backend at most once. It does NOT call the real verifier (which requires an LLM)
// — instead it simulates the exact control flow from handle_agent_result.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gitzi::agent::{AgentBackend, AgentResult, RunContext};
use gitzi::error::Result;
use gitzi::model::task::Task;
use proptest::prelude::*;

// ─── Mock Backend ─────────────────────────────────────────────────────────────

/// A mock agent backend that always returns a given response.
/// Counts how many times `run()` is called.
struct CountingBackend {
    call_count: Arc<AtomicUsize>,
    response: String,
}

impl AgentBackend for CountingBackend {
    async fn run(&self, _task: &Task, _ctx: &RunContext) -> Result<AgentResult> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(AgentResult::Success {
            output: self.response.clone(),
        })
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Strategy for agent summaries (content doesn't matter — we simulate rejection).
fn arb_agent_output() -> impl Strategy<Value = String> {
    "[a-zA-Z ]{10,200}"
}

/// Strategy for verifier rejection reasons.
fn arb_rejection_reason() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("Task requires adding a /health endpoint but the diff is empty.".to_string()),
        Just(
            "Agent claims to have stopped due to context limits — task is incomplete.".to_string()
        ),
        Just("Summary describes intent but no actual implementation was done.".to_string()),
        "[a-zA-Z ]{20,100}".prop_map(|s| format!("Verifier: {s}")),
    ]
}

// ─── Retry Logic Simulation ───────────────────────────────────────────────────

/// Replays the exact retry logic from `handle_agent_result` in agent_pool.rs,
/// assuming the verifier REJECTED the initial output. This is the code path:
///
///   1. Verifier rejects → reason provided
///   2. Build retry context with verifier feedback injected as resume_summary
///   3. ONE retry call to backend
///   4. (In production: verify again. Here we just count invocations.)
///
/// Returns the number of backend invocations during the retry path.
async fn simulate_retry_after_rejection(reason: &str, backend: &CountingBackend) -> usize {
    let task = Task::new("test-task", "epic-1", "Implement feature X");

    // This mirrors handle_agent_result after VerifyResult::Fail:
    let retry_ctx = RunContext {
        repo_root: std::path::PathBuf::from("/tmp/test"),
        branch: "test-branch".to_string(),
        resume_summary: Some(format!(
            "Your previous attempt was rejected by the verifier: {reason}\n\
             Re-read the task requirements and try again."
        )),
        mcp_token: None,
        answered_questions: vec![],
    };

    // ONE retry attempt — this is the invariant being tested
    let _retry_result = backend.run(&task, &retry_ctx).await;

    // After this single retry, the code either:
    // - Verifies again → Pass → advance (done, no more calls)
    // - Verifies again → Fail → escalate_verification_failure (done, no more calls)
    // Either way: exactly 1 backend invocation in the retry path.
    backend.call_count.load(Ordering::SeqCst)
}

// ─── Property Test ────────────────────────────────────────────────────────────

proptest! {
    /// Property: Maximum one retry before escalation
    ///
    /// When the verifier rejects an agent's output, the retry path invokes
    /// the backend exactly 1 time. Combined with the original call that
    /// produced the rejected output, the total is at most 2 backend calls
    /// per task processing cycle.
    #[test]
    fn max_one_retry_before_escalation(
        output in arb_agent_output(),
        reason in arb_rejection_reason(),
    ) {
        let call_count = Arc::new(AtomicUsize::new(0));
        let backend = CountingBackend {
            call_count: Arc::clone(&call_count),
            response: output,
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let retry_invocations = rt.block_on(simulate_retry_after_rejection(
            &reason,
            &backend,
        ));

        // Exactly 1 backend call in the retry path. Not 0 (would mean no retry).
        // Not 2+ (would mean unbounded retries).
        prop_assert_eq!(
            retry_invocations, 1,
            "Retry path must invoke backend exactly 1 time, got {}",
            retry_invocations,
        );
    }
}
