# Implementation Plan: Event-Driven Dispatcher

## Overview

Replace the polling `Scheduler` with an event-driven `Dispatcher` that reacts to typed events on a `tokio::broadcast` channel. Build in layers: data models and enums first, then the event bus, board projection, review queue, agent pool, dispatcher core, daemon integration, and finally TUI wiring.

## Tasks

- [x] 1. Define core types and enums
  - [x] 1.1 Create `src/dispatcher/mod.rs` with module declarations and the `Column` enum
    - Define `Column` enum with all 13 variants (Prioritized through Done)
    - Implement `Column::all()`, `next()`, `prev()`, `is_buffer()`, `agent_role()`
    - Define `AgentRole` enum with all 7 variants and `AgentRole::all()`, `AgentRole::column()`
    - Register `pub mod dispatcher;` in `src/lib.rs`
    - _Requirements: 3.1, 5.1_

  - [x] 1.2 Create `src/dispatcher/event_bus.rs` with `DispatchEvent` and `EventBus`
    - Define `DispatchEvent` enum with all 7 variants (TaskCreated, TaskStageChanged, HumanApprovalReceived, HumanRejectionReceived, AgentCompleted, AgentBlocked, BootComplete)
    - Implement `EventBus::new(capacity)`, `emit()`, `subscribe()`
    - Wrap `tokio::sync::broadcast::Sender<DispatchEvent>`
    - _Requirements: 1.1, 1.2, 1.3_

  - [x] 1.3 Extend `Task` model with new fields and history entries
    - Add `agent_feedback: Option<String>` field to `Task` struct in `src/model/task.rs`
    - Extend `Stage` enum to align with new `Column` values (or add mapping from Column to Stage)
    - Add history entry types: `StageChange`, `Approval`, `Rejection` with timestamps and metadata
    - Ensure TOML serialization round-trips for the new `[[history]]` array format
    - _Requirements: 8.1, 8.2, 8.3, 9.3_

  - [x] 1.4 Write property test for Column ordering and mapping
    - **Property 1: Board construction preserves task-to-column mapping**
    - **Property 2: Column priority ordering invariant**
    - **Validates: Requirements 2.1, 2.2, 3.2**

  - [x] 1.5 Write property test for history serialization round-trip
    - **Property 14: History serialization round-trip**
    - **Validates: Requirements 8.3**

- [x] 2. Implement Kanban Board projection
  - [x] 2.1 Create `src/dispatcher/board.rs` with `KanbanBoard` struct
    - Implement `KanbanBoard::from_tasks(Vec<Task>)` that maps tasks to columns by stage
    - Implement priority-sorted insertion (lowest number first) within each column
    - Handle invalid/unrecognized stages by defaulting to Prioritized with a warning log
    - Implement `tasks_in()`, `task()`, `count()`, `advance()`, `set_priority()`
    - _Requirements: 2.1, 2.2, 2.3, 3.1, 3.2, 3.3_

  - [x] 2.2 Create `WipLimits` struct with hardcoded defaults
    - Define `WipLimits` with `HashMap<Column, u32>` and `Default` impl
    - Work columns: limit 1 each (except Prioritized = MAX)
    - Buffer columns: limit 1 each
    - Done: unlimited (MAX)
    - Implement `WipLimits::allows(&self, column, current_count) -> bool`
    - _Requirements: 4.1, 4.4_

  - [x] 2.3 Write property tests for board construction and WIP enforcement
    - **Property 1: Board construction preserves task-to-column mapping**
    - **Property 4: WIP limit enforcement**
    - **Validates: Requirements 2.1, 2.2, 4.1, 4.2**

- [x] 3. Implement Human Review Queue
  - [x] 3.1 Create `src/dispatcher/review_queue.rs` with `HumanReviewQueue`
    - Define `ReviewItemKind` enum (AgentQuestion, BufferApproval)
    - Define `HumanReviewItem` struct with id, task_id, kind, created_at
    - Implement sort invariant: agent questions first (by arrival time), then buffer approvals (rightmost column first, then priority)
    - Implement `enqueue()`, `peek()` (respecting suppression), `dequeue()`, `has_agent_questions()`, `is_empty()`
    - _Requirements: 7.1, 7.2, 7.3, 7.4, 7.5, 7.6_

  - [x] 3.2 Write property tests for queue ordering and visibility
    - **Property 12: Queue ordering invariant**
    - **Property 13: Queue visibility — questions suppress approvals**
    - **Validates: Requirements 7.2, 7.3, 7.4, 6.5**

- [x] 4. Checkpoint - Ensure all tests pass
  - Ensure all tests pass, ask the user if questions arise.

- [x] 5. Implement Agent Pool
  - [x] 5.1 Create `src/dispatcher/agent_pool.rs` with `AgentHandle` and `AgentPool`
    - Define `AgentHandle` with role, `Arc<Notify>`, `Arc<AtomicBool>` for blocked state
    - Implement `AgentPool::spawn()` that creates 7 tokio tasks each sleeping on their `Notify`
    - Each agent loop: wake → pick highest-priority task → run agent backend → handle trope_blocker → advance or block
    - Implement `signal()`, `is_blocked()`, `unblock(role, answer)`
    - Include `agent_feedback` in prompt context when present on a task
    - _Requirements: 5.1, 5.2, 5.3, 5.4, 5.5, 9.4, 10.1, 10.2, 10.3_

  - [x] 5.2 Write property test for agent picks highest-priority task
    - **Property 6: Agent picks highest-priority task**
    - **Validates: Requirements 5.4**

  - [x] 5.3 Write property test for blocked agent rejects new work
    - **Property 16: Blocked agent rejects new work**
    - **Validates: Requirements 10.2**

- [x] 6. Implement Dispatcher core
  - [x] 6.1 Create `Dispatcher` struct in `src/dispatcher/mod.rs`
    - Wire together EventBus, `Arc<RwLock<KanbanBoard>>`, `Arc<Mutex<HumanReviewQueue>>`, AgentPool, Config, WipLimits
    - Implement `Dispatcher::start(config)` that loads tasks, builds board, spawns agents, emits BootComplete, signals agents with work
    - _Requirements: 2.1, 2.4, 11.1, 11.2, 11.3, 13.1_

  - [x] 6.2 Implement `Dispatcher::run()` event loop
    - Subscribe to EventBus, react to each event variant
    - On TaskStageChanged: signal target agent if work column, create review item if buffer column
    - On AgentCompleted: attempt advance to next buffer (with WIP check)
    - On AgentBlocked: create HumanReviewItem for agent question
    - On HumanApprovalReceived: signal the next agent
    - On HumanRejectionReceived: signal the previous agent
    - _Requirements: 1.2, 4.2, 4.3, 5.3, 6.1, 6.2_

  - [x] 6.3 Implement `Dispatcher::approve()`, `reject()`, `answer_question()`
    - `approve`: advance task from buffer to next work column, record history, emit event, signal agent
    - `reject`: move task to previous work column with priority=0, store feedback in agent_feedback, record history, emit event, signal agent
    - `answer_question`: dequeue item, unblock agent with answer
    - _Requirements: 6.3, 6.4, 8.1, 8.2, 9.1, 9.2, 9.3, 10.3_

  - [x] 6.4 Write property tests for dispatcher state mutations
    - **Property 3: State mutation emits corresponding event**
    - **Property 5: WIP release emits signal**
    - **Property 7: Agent completion advances to correct buffer**
    - **Property 8: Buffer columns block without approval**
    - **Property 9: Buffer entry creates review item**
    - **Property 10: Approval advances correctly and records history**
    - **Property 11: Rejection semantics**
    - **Validates: Requirements 1.2, 4.3, 5.5, 6.1, 6.2, 6.3, 6.4, 8.1, 8.2, 9.1, 9.2, 9.3**

- [x] 7. Checkpoint - Ensure all tests pass
  - Ensure all tests pass, ask the user if questions arise.

- [x] 8. Implement boot resume and agent startup
  - [x] 8.1 Implement boot resume logic in agent pool
    - When agent wakes after boot with an InProgress task, inspect existing branch/worktree/commits
    - Include work state summary in prompt context
    - If no recoverable state found, log warning and restart task from beginning of stage
    - _Requirements: 13.1, 13.2, 13.3, 13.4_

  - [x] 8.2 Write property tests for boot signal correctness
    - **Property 17: Boot signals correct agents**
    - **Property 18: Boot resume includes work state in context**
    - **Property 19: Agent wake signal on column entry**
    - **Validates: Requirements 13.1, 13.2, 13.3, 5.3**

- [x] 9. Integrate Dispatcher into daemon and remove Scheduler
  - [x] 9.1 Update `src/daemon/mod.rs` socket protocol with new commands
    - Add handlers for: `subscribe`, `peek_review`, `approve <task_id>`, `reject <task_id> <feedback>`, `answer <item_id> <answer>`, `board`
    - `subscribe` holds connection open and streams JSON-encoded DispatchEvent lines
    - `board` returns JSON snapshot of current board state
    - _Requirements: 12.1, 12.3_

  - [x] 9.2 Update `cmd_daemon` in `src/main.rs` to use Dispatcher instead of Scheduler
    - Replace Orchestrator + Scheduler construction with `Dispatcher::start(config).await`
    - Replace `scheduler.run()` with `dispatcher.run()`
    - Remove `Scheduler` import and usage
    - _Requirements: 11.1, 11.2, 11.3_

  - [x] 9.3 Remove `src/pipeline/scheduler.rs` and trim `src/pipeline/orchestrator.rs`
    - Delete scheduler.rs
    - Remove WIP/advance logic from orchestrator that moved to dispatcher
    - Keep orchestrator as thin compatibility layer or absorb remaining logic into dispatcher
    - _Requirements: 11.1_

- [x] 10. TUI integration
  - [x] 10.1 Update TUI to subscribe to EventBus and render new board layout
    - Connect to daemon socket with `subscribe` command
    - Render 13-column Kanban board from `board` snapshot
    - Update display reactively on each streamed DispatchEvent
    - Display topmost HumanReviewItem with approve/reject controls when queue is non-empty
    - Display idle state with board overview when queue is empty
    - _Requirements: 12.1, 12.2, 12.3, 7.5_

  - [x] 10.2 Write property test for rejection feedback propagation
    - **Property 15: Rejection feedback propagates to agent context**
    - **Validates: Requirements 9.3, 9.4**

- [x] 11. Final checkpoint - Ensure all tests pass
  - Ensure all tests pass, ask the user if questions arise.

## Notes

- Tasks marked with `*` are optional and can be skipped for faster MVP
- Each task references specific requirements for traceability
- Checkpoints ensure incremental validation
- Property tests validate universal correctness properties from the design document
- Unit tests validate specific examples and edge cases
- The `proptest` crate should be added to `[dev-dependencies]` in task 1.4
- Existing `src/state/reader.rs` and `src/state/writer.rs` are used for persistence — no changes needed there
- `src/trope_blocker.rs` is called within agent loops but unchanged

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1", "1.2"] },
    { "id": 1, "tasks": ["1.3", "2.2"] },
    { "id": 2, "tasks": ["1.4", "1.5", "2.1"] },
    { "id": 3, "tasks": ["2.3", "3.1"] },
    { "id": 4, "tasks": ["3.2", "5.1"] },
    { "id": 5, "tasks": ["5.2", "5.3", "6.1"] },
    { "id": 6, "tasks": ["6.2", "6.3"] },
    { "id": 7, "tasks": ["6.4", "8.1"] },
    { "id": 8, "tasks": ["8.2", "9.1"] },
    { "id": 9, "tasks": ["9.2", "9.3"] },
    { "id": 10, "tasks": ["10.1"] },
    { "id": 11, "tasks": ["10.2"] }
  ]
}
```
