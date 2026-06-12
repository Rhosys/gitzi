# Dispatcher Audit — Decisions

Decisions captured during post-implementation review. These will become tasks in a follow-up spec.

---

## Issue 1: Persistence model for review items

**Problem:** `approve()` / `reject()` mutate in-memory state only. No disk writes.

**Decision:**
- Task files remain source of truth for task state (stage, history, priority). Already persisted via `writer::write_task()` in CLI paths — need to also call it after dispatcher mutations.
- Review items become their own persisted resource type in the gitzi directory (`reviews/{id}.toml`).
- Each review item stores: task_id, kind, created_at, and an `actions` array (approval/rejection/comment with timestamp and content).
- On boot, unresolved review items are loaded from disk to re-populate the queue.
- Approval/rejection is recorded on the review item itself, then the task file is also updated.

## Issue 1b: ID format change

**Problem:** Current `new_id()` uses UUID v4 truncated to 8 hex chars.

**Decision:**
- All resource IDs (tasks, epics, review items) use UUID v7 encoded as base64url, appended with a 3-word slug derived from the title/description.
- Format: `{uuid7_base64url}-{three-word-slug}`
- Example: `AZD3kF8RdE2x_Qw-coding-auth-middleware`
- Applies to all resource types uniformly.

---

## Issue 2: TropeBlocker retry is a TODO

**Problem:** When trope_blocker fires, the agent drops the response and sleeps. Task is stuck forever.

**Decision:**
- On `Directive::Continue { injection }`: re-invoke the same agent backend once with the injection appended as a user-turn correction. If the second attempt also triggers a trope, escalate to review queue as an `AgentQuestion` ("agent stuck after trope correction — needs human guidance").
- On `Directive::RotateSession { summary }`: save summary to task metadata, re-invoke with summary as `resume_summary`. Same one-retry-then-escalate rule.
- Max 1 automatic retry, then `AgentBlocked` event emitted and review item created.

---

## Issue 3: Agent loop doesn't emit TaskStageChanged

**Problem:** After `board.advance()` in agent loop, emits `AgentCompleted` but never `TaskStageChanged`. The `run()` loop uses `TaskStageChanged` to create BufferApproval review items → tasks get stuck.

**Decision:**
- Emit `TaskStageChanged { task_id, from: current_col, to: target }` immediately after `board.advance()` succeeds in the agent loop.
- `AgentCompleted` stays as a separate downstream signal (TUI, logging).
- Both events are emitted: `TaskStageChanged` first (triggers review item creation), then `AgentCompleted` (informational).

---

## Issue 4: WIP limits never checked

**Problem:** `wip_limits` field exists but is never consulted. Agents advance unconditionally.

**Decision:**
- WIP is a property of columns, not tasks. Remove `wip_limit_blocked: bool` from the Task struct entirely.
- Agent loop: after completing work, asks the dispatcher to advance. Dispatcher checks `wip_limits.allows(target, board.count(target))`. If full, task stays in current column, agent goes back to sleep.
- When a task leaves a column (freeing a slot), dispatcher re-signals the agent whose completed task was waiting to advance into that column.
- No field on Task, no persistence of WIP state — it's purely a runtime gate.

---

## Issue 5: AgentBlocked event never emitted

**Problem:** No code path actually emits `AgentBlocked`. Review queue `AgentQuestion` items can never be created by the running system.

**Decision:**
- The agent backend signals "I have a question" via its response (need to define how — likely a structured marker in output, or a new `AgentResult` variant like `Blocked { question }`)
- When the agent detects a question: create a review item (persisted to `reviews/` as its own resource), emit `AgentBlocked`, set the agent handle's blocked flag, and sleep on the Notify.
- When the human answers via the TUI (answer command), the answer is recorded on the review item resource, the dispatcher calls `unblock(role, answer)`.
- On wake, the agent checks its blocked state, loads the review item + answer into context, and retries the task with that context.
- Same flow for TropeBlocker escalation (issue #2): create a review item with the trope correction failure as the question, block, wait for human.

---

## Issue 6: Config agent roles don't match dispatcher roles

**Problem:** Default config has role "developer" but dispatcher calls `resolve_agent("coder")` etc. Falls back silently.

**Decision:**
- Each `AgentRole` variant has a hardcoded default `AgentDef` built into the app (model, system prompt). This is the fallback when the config doesn't specify that role.
- Config can override any role by defining an `[[agents]]` entry with a matching role name.
- On config load: validate that every `role` in the config matches a known `AgentRole` variant. Error (fail startup) if an unknown role is specified — no silent ignoring of typos.
- `resolve_agent(role)`: look up in config first, fall back to the hardcoded default for that role. Never fall back to "first agent in list".

---

## Issue 7: repo_root hardcoded to "."

**Problem:** Daemon runs as systemd service, cwd is likely `/`. All git ops fail.

**Decision:**
- When the CLI installs the systemd unit (first run), it writes the user's `$HOME` into the unit file as an environment variable (e.g. `Environment=GITZI_HOME=/home/warren`).
- The daemon reads `$HOME` (or `$GITZI_HOME`) at startup to locate `~/.gitzi` and any repos referenced in session state.
- The repo path for each task/session comes from the session state (already stored), not from cwd.
- `build_run_context()` uses the repo path from session state, not `PathBuf::from(".")`.
