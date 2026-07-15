//! Verifier — gates subagent buffer transitions by validating that completed
//! work satisfies the task contract.
//!
//! Receives the full serialized Task (the contract), the subagent's post-summary
//! (its claim), and the git diff (the evidence). An LLM judges whether the claim
//! + evidence satisfy the contract.

use reqwest::Client;
use serde::Deserialize;
use tracing::{info, warn};

use crate::agent::llm_client;
use crate::config::Config;
use crate::model::Task;

/// Result of a verification check.
#[derive(Debug, Clone)]
pub enum VerifyResult {
    /// Work satisfies the task contract. Proceed to buffer.
    Pass,
    /// Work does NOT satisfy the task contract. Contains the reason.
    Fail { reason: String },
}

/// System prompt for the Verifier role. Includes trope-detection guidance
/// (formerly in trope_blocker.rs) as part of contract-violation detection.
const VERIFIER_SYSTEM_PROMPT: &str = r#"You are a strict verification agent. Your job is to determine whether completed work satisfies a task contract.

You will receive:
1. A TASK (the contract) — a structured object with title, description, and properties defining what must be done.
2. A SUMMARY — the executing agent's own account of what it did.
3. A DIFF — the actual code changes (git patch).

Your job: does the SUMMARY + DIFF satisfy ALL requirements expressed in the TASK?

## Verdict Rules

- If the work fulfills the task requirements: respond with PASS.
- If the work does NOT fulfill the task requirements: respond with FAIL and explain specifically what is missing or wrong.

## Evasion Detection

The executing agent may attempt to avoid doing work while claiming completion. Watch for:
- Claims that context is too large, recommending a "fresh session" or "new context"
- Stopping partway through with "let me commit what we have so far"
- Doing something different from what the task specifies without justification
- Claiming the task doesn't need to be done or was "already handled"
- Empty or trivial diffs that don't match the scope of the task
- Summary that describes intent ("I would do X") rather than completed action ("I did X")

Any of these is a FAIL.

## Response Format

Respond with exactly one of:
PASS
FAIL: <one-paragraph explanation of what is missing or wrong>

Nothing else. No preamble, no markdown, no extra commentary."#;

/// Run the verifier against a completed task.
///
/// # Arguments
/// - `config` — application config (to resolve the verifier's LLM backend)
/// - `task` — the full task struct (contract)
/// - `agent_summary` — the subagent's output/summary of what it did
/// - `diff` — the full git diff (patch text)
pub async fn verify(config: &Config, task: &Task, agent_summary: &str, diff: &str) -> VerifyResult {
    let agent_def = config.resolve_agent("verifier");
    let base_url = agent_def
        .api_url
        .as_deref()
        .unwrap_or("http://localhost:1234/v1");
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let model = &agent_def.model;

    // Serialize the full task as the contract — all properties included automatically
    let task_json = match serde_json::to_string_pretty(task) {
        Ok(j) => j,
        Err(e) => {
            warn!(task_id = %task.id, error = %e, "failed to serialize task for verifier");
            return VerifyResult::Fail {
                reason: format!("internal error: failed to serialize task: {e}"),
            };
        }
    };

    // Truncate diff if excessively large (verifier doesn't need 100k lines)
    let diff_truncated = if diff.len() > 50_000 {
        format!(
            "{}\n\n[... truncated at 50KB, {} total bytes ...]",
            &diff[..50_000],
            diff.len()
        )
    } else {
        diff.to_string()
    };

    let user_message = format!(
        "## TASK (contract)\n```json\n{task_json}\n```\n\n\
         ## SUMMARY (agent's claim)\n{agent_summary}\n\n\
         ## DIFF (evidence)\n```diff\n{diff_truncated}\n```"
    );

    let body = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": VERIFIER_SYSTEM_PROMPT },
            { "role": "user", "content": user_message },
        ],
        "temperature": 0.0,
    });

    let client = Client::new();
    let response: Result<VerifierResponse, _> =
        llm_client::post_with_retry(&client, &url, &body).await;

    match response {
        Ok(resp) => parse_verdict(&resp, &task.id),
        Err(e) => {
            warn!(task_id = %task.id, error = %e, "verifier LLM call failed");
            // On LLM failure, fail open — don't block the pipeline indefinitely
            // on infrastructure issues. The human review buffer is the next gate.
            VerifyResult::Pass
        }
    }
}

/// Parse the LLM's verdict from its response text.
fn parse_verdict(resp: &VerifierResponse, task_id: &str) -> VerifyResult {
    let text = resp
        .choices
        .first()
        .and_then(|c| c.message.content.as_deref())
        .unwrap_or("")
        .trim();

    if text.starts_with("PASS") {
        info!(%task_id, "verifier: PASS");
        VerifyResult::Pass
    } else if text.starts_with("FAIL") {
        let reason = text
            .strip_prefix("FAIL:")
            .or_else(|| text.strip_prefix("FAIL"))
            .unwrap_or(text)
            .trim()
            .to_string();
        let reason = if reason.is_empty() {
            "verifier rejected without explanation".to_string()
        } else {
            reason
        };
        info!(%task_id, %reason, "verifier: FAIL");
        VerifyResult::Fail { reason }
    } else {
        // Ambiguous response — treat as fail with the full text as reason
        warn!(%task_id, response = %text, "verifier returned ambiguous verdict");
        VerifyResult::Fail {
            reason: format!("ambiguous verifier response: {text}"),
        }
    }
}

// ─── Response deserialization ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct VerifierResponse {
    choices: Vec<VerifierChoice>,
}

#[derive(Deserialize)]
struct VerifierChoice {
    message: VerifierMessage,
}

#[derive(Deserialize)]
struct VerifierMessage {
    content: Option<String>,
}
