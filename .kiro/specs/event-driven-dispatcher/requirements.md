# Requirements Document

## Introduction

Replace the current polling scheduler (`src/pipeline/scheduler.rs`) with an event-driven dispatcher architecture for the gitzi Kanban agent harness. The new system models a multi-column Kanban board with role-specific agents, a task bag, a human review queue, configurable WIP limits, and buffer columns requiring human approval before work advances. The dispatcher reacts to state-change events rather than polling on a fixed interval.

## Glossary

- **Dispatcher**: The event-driven replacement for the polling Scheduler. Receives events from the Event_Bus and coordinates agent wake-ups, task advancement, and human review surfacing.
- **Event_Bus**: A tokio broadcast channel that carries typed events (task state changes, human approvals, agent completions, boot signals) to all subscribers.
- **Task_Bag**: The persistent collection of all tasks stored as TOML files in `~/.gitzi/<session>/plan/tasks/`. Each task has a status, title, description, priority, and linked metadata.
- **Human_Review_Queue**: An ordered queue of items requiring human action (approvals, answers to agent questions). Only one item is surfaced to the user at a time.
- **Human_Review_Item**: A single entry in the Human_Review_Queue, linked to a specific Task and optionally to an agent question. Contains the work artifact to review and approval/rejection controls.
- **Kanban_Board**: An in-memory projection derived from persistent task state on boot. Maps tasks to columns and enforces WIP limits. Never persisted directly.
- **Column**: A named stage on the Kanban_Board corresponding to an agent role or buffer. Columns are: prioritized, designing, coding-buffer, coding, review-buffer, reviewing, test-buffer, testing, security-audit-buffer, auditing, deployment-buffer, deploying, done.
- **Buffer_Column**: A column that requires human approval before a task advances to the next work column. Buffers sit between every pair of work columns.
- **WIP_Limit**: A configurable maximum number of tasks permitted in a given Column simultaneously. Prevents agents from pulling new work when the next column is full.
- **Agent_Role**: One of the seven LLM agent roles: Prioritizer, Designer, Coder, Reviewer, Tester, Auditor, Infrarian. Each role has exactly one agent instance.
- **Agent_Instance**: A single tokio task running the agent loop for one Agent_Role. Wakes on signal, processes the first item in its matching Column, then sleeps until signalled again.
- **Human_Approval**: A recorded decision (approve or reject with feedback) on a task in a Buffer_Column, stored in the task's TOML history.
- **TUI**: The ratatui-based terminal interface that displays the Kanban_Board, Human_Review_Queue, and agent status.

## Requirements

### Requirement 1: Event Bus

**User Story:** As the harness daemon, I want a typed event bus so that all components react to state changes without polling.

#### Acceptance Criteria

1. THE Event_Bus SHALL use a tokio broadcast channel with typed event variants covering: TaskCreated, TaskStageChanged, HumanApprovalReceived, HumanRejectionReceived, AgentCompleted, AgentBlocked, BootComplete.
2. WHEN a state-mutating operation completes (task write, approval, rejection), THE Dispatcher SHALL emit the corresponding event on the Event_Bus within the same logical operation.
3. THE Event_Bus SHALL support multiple concurrent subscribers without blocking the sender.

### Requirement 2: Task Bag Loading

**User Story:** As the harness daemon, I want to load all tasks from persistent storage on boot so that the in-memory Kanban_Board reflects the current state.

#### Acceptance Criteria

1. WHEN the daemon starts, THE Dispatcher SHALL read all task TOML files from the Task_Bag and construct the Kanban_Board projection in memory.
2. THE Dispatcher SHALL map each task's persisted stage to the corresponding Column on the Kanban_Board.
3. IF a task file contains an invalid or unrecognized stage, THEN THE Dispatcher SHALL log a warning and place the task in the prioritized Column.
4. WHEN boot loading completes, THE Dispatcher SHALL emit a BootComplete event on the Event_Bus.

### Requirement 3: Kanban Board Columns

**User Story:** As a user, I want the board to reflect distinct stages with buffer columns between work stages so that I can control when work advances.

#### Acceptance Criteria

1. THE Kanban_Board SHALL define the following columns in order: prioritized, designing, coding-buffer, coding, review-buffer, reviewing, test-buffer, testing, security-audit-buffer, auditing, deployment-buffer, deploying, done.
2. THE Kanban_Board SHALL maintain an ordered list of task references per Column, sorted by priority (lowest number first).
3. THE Kanban_Board SHALL be derived entirely from persistent task state and SHALL NOT be persisted itself.

### Requirement 4: WIP Limits

**User Story:** As a user, I want configurable WIP limits per column so that agents do not overwhelm downstream stages.

#### Acceptance Criteria

1. THE Kanban_Board SHALL enforce a configurable WIP_Limit for every Column except done.
2. WHEN an agent attempts to advance a task into a Column that has reached its WIP_Limit, THE Dispatcher SHALL block the advancement and leave the task in its current Column.
3. WHEN a task leaves a Column that was at its WIP_Limit, THE Dispatcher SHALL emit a TaskStageChanged event so that upstream agents can re-evaluate blocked work.
4. THE WIP_Limit configuration SHALL be hardcoded in source with per-column defaults (1 for work columns, 1 for buffer columns).

### Requirement 5: Agent Instances

**User Story:** As the harness daemon, I want one agent per role that wakes on signal so that work is processed reactively.

#### Acceptance Criteria

1. WHEN the daemon starts, THE Dispatcher SHALL spawn exactly one Agent_Instance as a tokio task for each of the seven Agent_Roles: Prioritizer, Designer, Coder, Reviewer, Tester, Auditor, Infrarian.
2. WHILE an Agent_Instance has no work in its matching Column, THE Agent_Instance SHALL sleep on a tokio Notify signal.
3. WHEN a task enters an Agent_Instance's matching Column, THE Dispatcher SHALL signal that Agent_Instance to wake.
4. WHEN an Agent_Instance wakes, THE Agent_Instance SHALL pick the highest-priority task in its Column and process it to completion before checking for additional work.
5. WHEN an Agent_Instance completes work on a task, THE Agent_Instance SHALL attempt to advance the task to the next Column (a Buffer_Column) and emit an AgentCompleted event.

### Requirement 6: Buffer Column Human Approval

**User Story:** As a user, I want buffer columns to require my explicit approval before work advances so that I maintain control over the pipeline.

#### Acceptance Criteria

1. THE Dispatcher SHALL treat every Buffer_Column as a gate requiring Human_Approval before the task advances to the next work Column. Buffer approvals are a separate mechanism from agent questions in the Human_Review_Queue.
2. WHEN a task enters a Buffer_Column, THE Dispatcher SHALL create a Human_Review_Item linked to that task and enqueue it in the Human_Review_Queue.
3. WHEN the user approves a Human_Review_Item, THE Dispatcher SHALL advance the task to the next work Column, record the approval in the task history, and emit a HumanApprovalReceived event.
4. WHEN the user rejects a Human_Review_Item, THE Dispatcher SHALL move the task back to the previous work Column with priority set to highest (0), record the rejection feedback in the task, and emit a HumanRejectionReceived event.
5. WHILE the Human_Review_Queue contains agent questions or blockers, THE TUI SHALL suppress buffer approval items and only surface them when no agent questions remain.

### Requirement 7: Human Review Queue

**User Story:** As a user, I want a single ordered queue of items needing my attention so that agent questions take priority over buffer approvals and I process reviews one at a time.

#### Acceptance Criteria

1. THE Human_Review_Queue SHALL contain two categories of items: agent questions/blockers (raised mid-task while agents are actively blocked waiting) and buffer approval items (stage gates where completed work needs human approval before advancing).
2. THE Human_Review_Queue SHALL order items with all agent questions/blockers sorted first (by arrival time), followed by buffer approval items sorted by rightmost Buffer_Column first (deployment-buffer before security-audit-buffer, etc.), then by task priority within the same column.
3. WHILE the Human_Review_Queue contains agent questions or blockers, THE TUI SHALL display only agent question items and suppress buffer approval items from surfacing.
4. WHILE no agent questions remain in the Human_Review_Queue and buffer approval items exist, THE TUI SHALL surface buffer approval items to the user.
5. WHILE there are no active items in the Human_Review_Queue, THE TUI SHALL display an idle state with the Kanban_Board overview.
6. THE Human_Review_Queue SHALL be maintained in memory and derived from persistent task state on boot.

### Requirement 8: Human Approval Tracking

**User Story:** As a user, I want approval and rejection decisions recorded in the task so that there is an audit trail.

#### Acceptance Criteria

1. WHEN the user approves a task, THE Dispatcher SHALL append an approval entry to the task's history with a timestamp and the target stage.
2. WHEN the user rejects a task, THE Dispatcher SHALL append a rejection entry to the task's history with a timestamp, the feedback text, and the stage it was returned to.
3. THE Task_Bag SHALL persist approval and rejection entries as part of the task TOML file's history array.

### Requirement 9: Rejection Handling

**User Story:** As a user, I want rejected tasks to return to the previous agent with clear feedback so that the issue is addressed.

#### Acceptance Criteria

1. WHEN a task is rejected from a Buffer_Column, THE Dispatcher SHALL move the task to the immediately preceding work Column.
2. WHEN a task is rejected, THE Dispatcher SHALL set the task's priority to 0 (highest) so that the responsible Agent_Instance picks it up first.
3. WHEN a task is rejected, THE Dispatcher SHALL store the rejection feedback in the task's agent_feedback field so that the Agent_Instance receives it on wake-up.
4. WHEN a rejected task enters a work Column, THE Agent_Instance SHALL include the rejection feedback in its prompt context.

### Requirement 10: Agent Blocking on Human Review

**User Story:** As an agent, I want to block and wait when I have a question for the human so that I do not proceed with incomplete information.

#### Acceptance Criteria

1. WHEN an Agent_Instance encounters ambiguity requiring human input, THE Agent_Instance SHALL create a Human_Review_Item with the question, link it to the current task, and emit an AgentBlocked event.
2. WHILE an Agent_Instance is blocked on a Human_Review_Item, THE Agent_Instance SHALL not process any other tasks.
3. WHEN a blocked Human_Review_Item is answered, THE Dispatcher SHALL signal the blocked Agent_Instance to resume with the answer injected into context.

### Requirement 11: Removal of Polling Scheduler

**User Story:** As a developer, I want the polling scheduler removed so that there is a single dispatch mechanism.

#### Acceptance Criteria

1. THE Dispatcher SHALL replace the Scheduler struct and its 10-second polling loop in `src/pipeline/scheduler.rs`.
2. THE Dispatcher SHALL handle all responsibilities previously owned by the Scheduler: task dispatch, agent invocation, test execution after review, and stage advancement.
3. WHEN the daemon starts, THE Dispatcher SHALL be the sole orchestration entry point (no concurrent Scheduler running).

### Requirement 12: TUI Integration

**User Story:** As a user, I want the TUI to reflect the new board layout and surface the topmost review item so that I interact through the terminal.

#### Acceptance Criteria

1. WHEN the TUI connects to the daemon, THE TUI SHALL subscribe to the Event_Bus and update its display on every state-change event.
2. WHILE there are items in the Human_Review_Queue, THE TUI SHALL display the topmost item's work artifact (diff, question, or output) with approve/reject controls.
3. WHEN the user submits an approval or rejection through the TUI, THE TUI SHALL send the decision to the Dispatcher via the daemon socket protocol.

### Requirement 13: Agent Startup Resume

**User Story:** As an agent, I want to review existing work state on startup so that I resume in-progress tasks correctly rather than starting fresh.

#### Acceptance Criteria

1. WHEN boot loading completes and the Kanban_Board is constructed, THE Dispatcher SHALL signal each Agent_Instance whose matching Column contains at least one task.
2. WHEN an Agent_Instance wakes after boot and finds a task that was InProgress before shutdown, THE Agent_Instance SHALL review the existing work state (branch, worktree, partial commits) before resuming execution.
3. THE Agent_Instance SHALL include the existing work state summary in its prompt context so that the agent understands what was previously accomplished.
4. IF an Agent_Instance finds no recoverable work state for an InProgress task, THEN THE Agent_Instance SHALL log a warning and restart the task from the beginning of the current stage.
