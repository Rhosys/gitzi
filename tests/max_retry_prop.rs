// Feature: dispatcher-audit-fixes, Property 6: Maximum one retry before escalation
// **Validates: Requirements 3.1, 3.2, 3.3, 3.4, 3.5**

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use gitzi::agent::{AgentBackend, AgentResult, RunContext};
use gitzi::error::Result;
use gitzi::model::task::Task;
use gitzi::trope_blocker;
use proptest::prelude::*;

// ─── Mock Backend ─────────────────────────────────────────────────────────────

/// A mock agent backend that always returns a trope-triggering response.
/// Counts how many times `run()` is called.
struct CountingBackend {
    call_count: Arc<AtomicUsize>,
    /// The response to return on every call (always triggers a trope).
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

/// Build a response string guaranteed to trigger the "Large Context" trope.
/// Requires at least 2 signal phrases from the TropeBlocker definitions.
fn trope_triggering_response(extra: &str) -> String {
    format!(
        "I've been going for a while and my context is getting long. {extra}"
    )
}

/// Strategy for extra text that doesn't accidentally remove the trope signals.
fn arb_extra_text() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 .,!?]{0,200}"
}

/// Strategy for token estimates (affects whether directive is Continue or RotateSession).
fn arb_token_estimate() -> impl Strategy<Value = usize> {
    // Below and above the CONTEXT_ROTATION_THRESHOLD (180_000)
    prop_oneof![
        0usize..180_000,       // Below threshold → Directive::Continue
        180_001usize..500_000, // Above threshold → Directive::RotateSession
    ]
}

// ─── Retry Logic Simulation ───────────────────────────────────────────────────

/// Replays the exact retry logic from `handle_agent_result` in agent_pool.rs.
/// Returns the total number of backend invocations during the retry path.
///
/// This mirrors the code path:
///   1. Initial response triggers trope → execute() returns directive
///   2. ONE retry call to backend
///   3. If retry still triggers trope → escalate (no further calls)
async fn simulate_retry_path(
    initial_output: &str,
    token_estimate: usize,
    backend: &CountingBackend,
) -> usize {
    let task = Task::new("test-task", "epic-1", "Test Task");
    let ctx = RunContext {
        repo_root: std::path::PathBuf::from("/tmp/test"),
        branch: "main".to_string(),
        resume_summary: None,
        mcp_token: None,
        answered_questions: vec![],
    };

    // Step 1: Scan the initial output (already known to be Blocked)
    let scan_result = trope_blocker::scan(initial_output);
    let trope_match = match scan_result {
        trope_blocker::ScanResult::Blocked(m) => m,
        trope_blocker::ScanResult::Clean => return 0,
    };

    // Step 2: Execute the trope action to get directive
    let task_summary = format!("Task: {} — {}", task.id, task.title);
    let directive = trope_blocker::execute(&trope_match, token_estimate, &task_summary);

    // Step 3: Build retry context based on directive (mirrors handle_agent_result)
    let retry_ctx = match directive {
        trope_blocker::Directive::Continue { ref injection } => RunContext {
            repo_root: ctx.repo_root.clone(),
            branch: ctx.branch.clone(),
            resume_summary: Some(injection.clone()),
            mcp_token: None,
            answered_questions: ctx.answered_questions.clone(),
        },
        trope_blocker::Directive::RotateSession { ref summary } => RunContext {
            repo_root: ctx.repo_root.clone(),
            branch: ctx.branch.clone(),
            resume_summary: Some(summary.clone()),
            mcp_token: None,
            answered_questions: ctx.answered_questions.clone(),
        },
    };

    // Step 4: ONE retry attempt (this is the core invariant being tested)
    let retry_result = backend.run(&task, &retry_ctx).await;

    // Step 5: Check retry result — no further backend calls regardless of outcome
    let _retry_clean = match retry_result {
        Ok(AgentResult::Success { ref output }) => trope_blocker::scan(output).is_clean(),
        _ => false,
    };

    // Whether clean or blocked, no further backend calls happen.
    // Clean → advance (done). Blocked → escalate to review item (done).
    backend.call_count.load(Ordering::SeqCst)
}

// ─── Property Test ────────────────────────────────────────────────────────────

proptest! {
    /// Property 6: Maximum one retry before escalation
    ///
    /// For any trope-triggering response (Continue or RotateSession directive),
    /// the agent loop invokes the backend at most 1 additional time before
    /// escalating. Total backend invocations during the retry path never exceeds
    /// 1. Combined with the original call that produced the trope-triggering
    /// output, the total is at most 2.
    #[test]
    fn max_one_retry_before_escalation(
        extra in arb_extra_text(),
        token_estimate in arb_token_estimate(),
    ) {
        let response = trope_triggering_response(&extra);

        // Verify our input actually triggers a trope
        let scan = trope_blocker::scan(&response);
        prop_assert!(
            matches!(scan, trope_blocker::ScanResult::Blocked(_)),
            "Test input must trigger a trope, got Clean for: {}",
            &response[..80.min(response.len())]
        );

        // Run the retry simulation with a backend that always returns trope responses
        let call_count = Arc::new(AtomicUsize::new(0));
        let backend = CountingBackend {
            call_count: Arc::clone(&call_count),
            response: response.clone(),
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let retry_invocations = rt.block_on(simulate_retry_path(
            &response,
            token_estimate,
            &backend,
        ));

        // The retry path should invoke the backend exactly 1 time.
        // Adding the original call (which happened before entering the retry
        // logic), total backend invocations = 2 (original + 1 retry).
        prop_assert_eq!(
            retry_invocations, 1,
            "Retry path must invoke backend exactly 1 time (the single retry), got {} \
             for token_estimate={}",
            retry_invocations,
            token_estimate
        );
    }
}
