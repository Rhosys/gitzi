# ADR-001: Verifier Agent Replaces Trope Blocker

**Status:** Accepted  
**Date:** 2026-06-22

## Context

LLM subagents exhibit a failure mode where they agree to do A, then silently decide not to do it — either by doing B instead, stalling with "context is too long", or claiming the work is done when it isn't. The existing `trope_blocker` module attempted to catch this via substring matching on known evasion phrases. This is fragile: it catches only syntactic patterns, not semantic contract violations.

The core problem: **there is no structural validation that the work an agent claims to have completed actually satisfies the task contract.**

## Decision

Replace `trope_blocker.rs` with a new `AgentRole::Verifier` that gates every subagent-requested transition to a buffer column.

### Design

The Verifier receives three inputs:

1. **Full serialized Task struct** — title, description, and all properties. This IS the contract. New properties added to `Task` in the future are automatically included without modifying the verifier.
2. **Subagent's post-summary** — the `output` field from `AgentResult::Success`, which is the subagent's own account of what it did.
3. **Git diff** — the full patch (merge-base to branch tip), which is the objective evidence of what changed.

The Verifier produces a pass/fail verdict with reasoning:

- **Pass** → `try_advance` proceeds (task moves to buffer column for human review)
- **Fail** → feedback is injected as `agent_feedback` on the task, the subagent retries (or escalates to human review after retry exhaustion)

### Trope detection is subsumed

The trope blocker's signal phrases ("context is getting long", "let me stop here", etc.) become examples in the Verifier's system prompt — teaching the LLM what evasion looks like. A contract-violation check naturally catches these: if the task says "implement X" and the summary says "I ran out of context", that's a failed verification regardless of specific phrase matching.

### Insertion point

In `handle_agent_result` (dispatcher/agent_pool.rs), the current flow:

```
AgentResult::Success → trope_blocker::scan → Clean → try_advance
```

Becomes:

```
AgentResult::Success { output } →
  get_full_diff(repo, branch) →
  verifier.verify(serialized_task, output, diff) →
    Pass → try_advance
    Fail { reason } → set agent_feedback, retry or escalate
```

### The Verifier as an AgentRole

`AgentRole::Verifier` does not own a board column. It is invoked inline by the agent pool as a gate function. It uses the same LLM infrastructure as other roles (`config.resolve_agent("verifier")`) so it can be configured with a specific model/provider independently of the coding agents.

## Consequences

- `trope_blocker.rs` is deleted. Its test coverage is replaced by verifier integration tests.
- Every buffer transition incurs one additional LLM call (the verification). This is acceptable because it prevents wasted human review time on non-conforming work.
- The verifier is property-agnostic: adding fields to `Task` automatically extends the contract surface. No verifier code changes needed.
- Retry logic (currently one retry after trope detection) is preserved but simplified — the verifier's failure reason becomes the retry prompt.

## Alternatives Considered

1. **Keep trope blocker alongside verifier** — Rejected. The verifier strictly subsumes the trope blocker. Running both wastes a scan pass and creates two sources of truth for "is this output acceptable?"

2. **Pre-summary (agent produces a plan before executing)** — Rejected. The task description already IS the plan. Asking the LLM to rephrase it adds a roundtrip with no new information. The verifier compares output against the task directly.

3. **Structured checklist contract** — Rejected for now. The freeform task description + LLM judgment is sufficient and doesn't require a planning phase to produce structured assertions. Can be revisited if verification accuracy is insufficient.
