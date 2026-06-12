# Implementation Plan: Dispatcher Audit Fixes

## Overview

Surgical fixes to the event-driven dispatcher addressing 11 audit findings. Changes span new modules (`id.rs`, `state/review.rs`), modifications to the agent loop, dispatcher run loop, config resolution, daemon systemd unit, and minor serialization/API fixes. Each task builds incrementally — foundational types first, then logic changes, then wiring and tests.

## Tasks

- [x] 1. Add dependencies and create foundational types
  - [x] 1.1 Update Cargo.toml: add `base64 = "0.22"`, enable `uuid/v7` feature
    - Change `uuid = { version = "1", features = ["v4", "serde"] }` to `uuid = { version = "1", features = ["v4", "v7", "serde"] }`
    - Add `base64 = "0.22"` to `[dependencies]`
    - _Requirements: 2.1, 2.2_

  - [x] 1.2 Create `src/id.rs`: UUID v7 + base64url + slug ID generation
    - Implement `new_id(title: &str) -> String` using `Uuid::now_v7()`, `URL_SAFE_NO_PAD` encoding
    - Embed a 256-word list as `const WORDS`
    - Implement `derive_slug(title: &str) -> String` using a hash of the title to select 3 words
    - Register the module in `src/lib.rs` or `src/main.rs`
    - _Requirements: 2.1, 2.2, 2.3_

  - [x] 1.3 Convert `AgentResult` from struct to enum in `src/agent/backend.rs`
    - Replace `pub struct AgentResult { pub success: bool, pub output: String }` with enum: `Success { output }`, `Failure { output }`, `Blocked { question }`
    - Update all call sites that destructure or construct `AgentResult` (agent_pool.rs pattern matches)
    - _Requirements: 6.1_

  - [x] 1.4 Add `Deserialize` to `DispatchEvent`, `AgentRole`, `ReviewItemKind`, `HumanReviewItem`
    - Add `Deserialize` derive to `DispatchEvent` in `src/dispatcher/event_bus.rs`
    - Add `Deserialize` derive to `AgentRole` in `src/dispatcher/mod.rs`
    - Add `Deserialize` derive to `ReviewItemKind` and `HumanReviewItem` in `src/dispatcher/review_queue.rs`
    - _Requirements: 9.1, 9.2_

- [ ] 2. Implement review item persistence layer
  - [x] 2.1 Create `src/state/review.rs` with `PersistedReviewItem`, `ReviewAction`, `PersistedReviewKind` types
    - Define `ReviewAction` enum (Approval, Rejection, Answer) with serde tag dispatch
    - Define `PersistedReviewKind` enum (AgentQuestion, BufferApproval)
    - Define `PersistedReviewItem` struct with id, task_id, kind, created_at, actions vec
    - Implement `is_unresolved()` method
    - _Requirements: 1.1, 1.4_

  - [x] 2.2 Implement persistence functions in `src/state/review.rs`
    - `reviews_dir() -> Result<PathBuf>` using `session_dir().join("reviews")`
    - `write_review_item(item: &PersistedReviewItem) -> Result<()>` with atomic_write
    - `load_review_item(id: &str) -> Result<PersistedReviewItem>`
    - `load_all_unresolved() -> Result<Vec<PersistedReviewItem>>`
    - Register module in `src/state/mod.rs`
    - _Requirements: 1.1, 1.2, 1.3, 1.4_

  - [-] 2.3 Write property test for review item round-trip (Property 1)
    - **Property 1: Review item persistence round-trip**
    - Generate random `PersistedReviewItem` instances, serialize to TOML, write to temp dir, read back, assert equality
    - **Validates: Requirements 1.1, 1.4**

  - [-] 2.4 Write property test for action append persistence (Property 2)
    - **Property 2: Approval/rejection action persistence**
    - Generate review item + random actions, append and persist, verify on-disk file contains all actions in order
    - **Validates: Requirements 1.2, 1.3, 1.5**

- [ ] 3. Implement ID module and config changes
  - [x] 3.1 Implement config role validation and hardcoded defaults in `src/config.rs`
    - Add `default_agent_def()` and `default_system_prompt()` methods to `AgentRole` (requires importing AgentRole or moving logic)
    - Add `validate()` method to `Config` that rejects unknown role names
    - Replace `resolve_agent()` logic: check config entries first, fall back to hardcoded default for role, never fall back to first-in-list
    - Change return type from `&AgentDef` to owned `AgentDef` to support generating defaults
    - _Requirements: 7.1, 7.2, 7.3, 7.4_

  - [-] 3.2 Write property test for ID format invariant (Property 3)
    - **Property 3: ID format invariant**
    - Generate arbitrary non-empty title strings, validate `new_id()` output: 22-char base64url prefix decoding to 16 bytes, hyphen, 3 hyphen-separated lowercase words from word list
    - **Validates: Requirements 2.1, 2.2, 2.3**

  - [-] 3.3 Write property test for slug determinism (Property 4)
    - **Property 4: ID slug determinism**
    - Generate title strings, call `new_id()` twice with same title, verify slug portions are identical (UUID prefix will differ)
    - **Validates: Requirements 2.3**

  - [-] 3.4 Write property test for old/new ID acceptance (Property 5)
    - **Property 5: Old and new ID format acceptance**
    - Generate both old-format (8 hex chars) and new-format IDs, write a task with each, load from disk, verify ID matches
    - **Validates: Requirements 2.4**

  - [-] 3.5 Write property test for config role validation (Property 11)
    - **Property 11: Config role validation rejects unknown roles**
    - Generate random strings that don't match any AgentRole display name, create config with that role, call validate(), assert error containing the invalid role name
    - **Validates: Requirements 7.2**

  - [x] 3.6 Write property test for resolve_agent (Property 12)
    - **Property 12: resolve_agent returns config override or hardcoded default**
    - Generate configs with/without entries for each role, call resolve_agent(), verify it returns config entry if present or hardcoded default if absent, never a mismatched role
    - **Validates: Requirements 7.1, 7.3, 7.4**

- [ ] 4. Checkpoint - Verify foundational changes compile
  - Ensure all tests pass, ask the user if questions arise.

- [ ] 5. Modify task model and dispatcher WIP logic
  - [x] 5.1 Update `src/model/task.rs`: remove `wip_limit_blocked`, add `resume_summary`
    - Remove `wip_limit_blocked: bool` field from `Task` struct and `Task::new()`
    - Add `resume_summary: Option<String>` field (serde skip_serializing_if = "Option::is_none")
    - _Requirements: 5.1, 3.3_

  - [ ] 5.2 Add WIP waiting map to Dispatcher in `src/dispatcher/mod.rs`
    - Add `wip_waiting: Arc<Mutex<HashMap<Column, AgentRole>>>` field to `Dispatcher` struct
    - Initialize in `Dispatcher::start()`
    - Implement WIP slot release in `TaskStageChanged` handler: when task leaves a column, check `wip_waiting` for that column's `from`, signal waiting agent, remove entry
    - _Requirements: 5.4, 5.5_

  - [ ] 5.3 Fix double-signal on approval/rejection in `src/dispatcher/mod.rs`
    - Change `HumanApprovalReceived` match arm to no-op (log only, remove `agent_pool.signal()`)
    - Change `HumanRejectionReceived` match arm to no-op (log only, remove `agent_pool.signal()`)
    - `approve()` and `reject()` methods already signal — the run loop must not duplicate
    - _Requirements: 10.1, 10.2_

  - [ ] 5.4 Persist review items on buffer entry in `src/dispatcher/mod.rs`
    - In `TaskStageChanged` handler when `to.is_buffer()`: write a `PersistedReviewItem` to disk via `write_review_item()` in addition to enqueuing in memory
    - _Requirements: 1.1_

  - [ ] 5.5 Persist task files on approve/reject in `src/dispatcher/mod.rs`
    - After `board.advance()` in `approve()`: call `write_task()` to persist the updated task to disk
    - After `board.advance()` in `reject()`: call `write_task()` to persist the updated task to disk
    - Also persist the review item with action appended
    - _Requirements: 1.2, 1.3, 1.5_

  - [ ] 5.6 Write property test for WIP limit blocking (Property 8)
    - **Property 8: WIP limit blocks advancement**
    - Generate board state with target column at WIP limit, attempt advance, verify task stays in source column, target count unchanged
    - **Validates: Requirements 5.2, 5.3**

  - [ ] 5.7 Write property test for WIP release re-signal (Property 9)
    - **Property 9: WIP release re-signals waiting agent**
    - Generate WIP-blocked state with recorded waiting agent, simulate task leaving that column, verify signal fired once and waiting record removed
    - **Validates: Requirements 5.4, 5.5**

- [ ] 6. Implement agent loop changes
  - [ ] 6.1 Implement TropeBlocker retry logic in `src/dispatcher/agent_pool.rs`
    - Replace TODO comments in `handle_agent_result()` with actual retry: on `Directive::Continue`, re-invoke backend with injection in context
    - On `Directive::RotateSession`, save summary to `task.resume_summary`, re-invoke with summary
    - After retry: if clean, proceed to advance; if still blocked, escalate (create review item, emit AgentBlocked, set blocked)
    - _Requirements: 3.1, 3.2, 3.3, 3.4, 3.5_

  - [ ] 6.2 Implement WIP gate in `try_advance()` within `src/dispatcher/agent_pool.rs`
    - Before `board.advance()`: check `wip_limits.allows(target_col, board.count(target_col))`
    - If blocked: record in `wip_waiting` map, return without advancing (agent sleeps)
    - If allowed: advance, emit `TaskStageChanged` THEN `AgentCompleted`
    - _Requirements: 4.1, 4.2, 5.2, 5.3_

  - [ ] 6.3 Implement `AgentResult::Blocked` handling in agent loop
    - Add match arm for `AgentResult::Blocked { question }`: persist review item, emit `AgentBlocked`, set blocked flag, enqueue in HumanReviewQueue
    - Update `AgentResult::Failure` handling (was `!success` check)
    - _Requirements: 6.1_

  - [ ] 6.4 Fix `build_run_context()` to use session state repo path
    - Replace `PathBuf::from(".")` with actual repo path from session state
    - Load repo path from session state (or config) instead of hardcoded "."
    - _Requirements: 8.3_

  - [ ] 6.5 Fix `resume_context()` git2 0.21 `commit.summary()` API
    - Change `commit.summary().unwrap_or(None).unwrap_or("(no message)")` to `commit.summary().unwrap_or("(no message)")`
    - git2 0.21 returns `Option<&str>` directly, not `Option<Option<&str>>`
    - _Requirements: 11.1, 11.2_

  - [ ] 6.6 Write property test for max one retry (Property 6)
    - **Property 6: Maximum one retry before escalation**
    - Mock backend that returns trope-triggering responses, count invocations, verify at most 2 total (original + 1 retry)
    - **Validates: Requirements 3.1, 3.2, 3.3, 3.4, 3.5**

  - [ ] 6.7 Write property test for event ordering (Property 7)
    - **Property 7: TaskStageChanged precedes AgentCompleted on successful advance**
    - Generate task in random work column, simulate successful completion, capture event bus emissions, verify TaskStageChanged appears before AgentCompleted
    - **Validates: Requirements 4.1, 4.2**

  - [ ] 6.8 Write property test for AgentBlocked flow (Property 10)
    - **Property 10: Agent question creates review item and blocks**
    - Generate random question strings, trigger `AgentResult::Blocked`, verify review item persisted, AgentBlocked emitted, blocked flag set
    - **Validates: Requirements 6.1**

  - [ ] 6.9 Write property test for RunContext repo_root (Property 14)
    - **Property 14: RunContext repo_root from session state**
    - Generate session states with random paths, call `build_run_context()`, verify repo_root equals session path, never `PathBuf::from(".")`
    - **Validates: Requirements 8.3**

- [ ] 7. Checkpoint - Verify agent loop changes compile and pass
  - Ensure all tests pass, ask the user if questions arise.

- [ ] 8. Daemon and boot changes
  - [ ] 8.1 Add `Environment=HOME=...` to systemd unit in `src/daemon/mod.rs`
    - In `install_systemd()`, read `std::env::var("HOME")` and write `Environment=HOME={home}` line into the unit file between `ExecStart` and `Restart`
    - _Requirements: 8.1, 8.2_

  - [ ] 8.2 Load unresolved review items on boot in `Dispatcher::start()`
    - After creating empty `HumanReviewQueue`, call `load_all_unresolved()`
    - Convert each `PersistedReviewItem` into a `HumanReviewItem` and enqueue
    - _Requirements: 1.4_

  - [ ] 8.3 Write property test for DispatchEvent JSON round-trip (Property 13)
    - **Property 13: DispatchEvent JSON round-trip**
    - Generate all 7 DispatchEvent variants with random payloads, serialize to JSON, deserialize back, assert equality
    - **Validates: Requirements 9.1, 9.2**

- [ ] 9. Final checkpoint - Ensure all tests pass
  - Ensure all tests pass, ask the user if questions arise.

## Notes

- Tasks marked with `*` are optional and can be skipped for faster MVP
- Each task references specific requirements for traceability
- Checkpoints ensure incremental validation
- Property tests validate universal correctness properties from the design document
- Unit tests validate specific examples and edge cases
- The `wip_waiting` map is runtime-only (not persisted) — rebuilt implicitly when agents re-signal on boot
- Old-format IDs coexist with new-format indefinitely — no migration needed

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1", "1.3", "1.4"] },
    { "id": 1, "tasks": ["1.2", "2.1", "5.1"] },
    { "id": 2, "tasks": ["2.2", "3.1"] },
    { "id": 3, "tasks": ["2.3", "2.4", "3.2", "3.3", "3.4", "3.5", "3.6"] },
    { "id": 4, "tasks": ["5.2", "5.3", "5.4", "5.5"] },
    { "id": 5, "tasks": ["6.1", "6.2", "6.3", "6.4", "6.5"] },
    { "id": 6, "tasks": ["5.6", "5.7", "6.6", "6.7", "6.8", "6.9"] },
    { "id": 7, "tasks": ["8.1", "8.2"] },
    { "id": 8, "tasks": ["8.3"] }
  ]
}
```
