# Requirements Document

## Introduction

Post-implementation audit of the event-driven dispatcher revealed 7 critical runtime gaps plus minor API/serialization issues. This spec addresses all audit decisions from AUDIT-DECISIONS.md: review item persistence, UUID v7 IDs, TropeBlocker retry logic, TaskStageChanged emission from agent loop, WIP limit enforcement, AgentBlocked flow, config role validation, daemon HOME discovery, and minor fixes (Deserialize on DispatchEvent, double-signal on approval, git2 0.21 commit.summary() API).

## Glossary

- **Dispatcher**: Central orchestration struct that owns the event bus, board, review queue, agent pool, and WIP limits
- **Review_Item**: A persisted resource in `~/.gitzi/session/reviews/{id}.toml` representing a pending human action (approval, rejection, or agent question)
- **Agent_Pool**: The subsystem managing 7 agent tokio tasks, their Notify handles, and blocked state
- **TropeBlocker**: Scanner that detects lazy/evasive LLM response patterns and prescribes corrective actions
- **Event_Bus**: Single tokio::broadcast channel carrying typed DispatchEvent variants
- **Kanban_Board**: In-memory projection of tasks into 13 ordered columns, rebuilt from disk on boot
- **WIP_Limits**: Runtime gate that prevents column overcrowding by blocking advancement when a column is full
- **Agent_Backend**: Trait abstraction over LLM invocation (run task → AgentResult)
- **UUID_v7_ID**: A time-ordered UUID v7 encoded as base64url with a 3-word slug suffix
- **Session_State**: Persisted state in `~/.gitzi/<session-id>/` including tasks, epics, and reviews

## Requirements

### Requirement 1: Review item persistence

**User Story:** As a daemon operator, I want review items persisted to disk, so that unresolved items survive restarts and approval/rejection actions are durable.

#### Acceptance Criteria

1. WHEN a HumanReviewItem is created, THE Dispatcher SHALL write it to `~/.gitzi/session/reviews/{id}.toml` containing task_id, kind, created_at, and an empty actions array
2. WHEN an approval action is performed on a review item, THE Dispatcher SHALL append an approval action with timestamp to the review item's actions array and persist both the review item file and the task file to disk
3. WHEN a rejection action is performed on a review item, THE Dispatcher SHALL append a rejection action with timestamp and feedback to the review item's actions array and persist both the review item file and the task file to disk
4. WHEN the daemon boots, THE Dispatcher SHALL load all unresolved review item files from `~/.gitzi/session/reviews/` and re-populate the HumanReviewQueue
5. THE Dispatcher SHALL persist task file changes (stage, history, priority, agent_feedback) to disk after every approve or reject mutation

### Requirement 2: UUID v7 base64url identifiers

**User Story:** As a developer, I want time-ordered IDs with human-readable slugs, so that resources sort chronologically and are recognizable at a glance.

#### Acceptance Criteria

1. THE ID_Generator SHALL produce identifiers in the format `{uuid7_base64url}-{three-word-slug}` for all new tasks, epics, and review items
2. THE ID_Generator SHALL encode the UUID v7 bytes as base64url without padding
3. THE ID_Generator SHALL derive the three-word slug from the resource title or description using a deterministic word list
4. WHEN an existing resource is loaded from disk, THE System SHALL accept both old-format IDs (8 hex chars) and new-format IDs without error

### Requirement 3: TropeBlocker retry and escalation

**User Story:** As a daemon operator, I want the TropeBlocker to retry once before escalating, so that transient trope detections self-correct and persistent ones surface for human intervention.

#### Acceptance Criteria

1. WHEN TropeBlocker returns Directive::Continue with an injection, THE Agent_Pool SHALL re-invoke the same agent backend once with the injection appended as a user-turn correction
2. IF the second invocation after a Continue injection also triggers a trope, THEN THE Agent_Pool SHALL emit AgentBlocked, create a persisted review item with the trope correction failure as the question, set the agent's blocked flag, and sleep
3. WHEN TropeBlocker returns Directive::RotateSession with a summary, THE Agent_Pool SHALL save the summary to task metadata as resume_summary and re-invoke the agent with that summary in context
4. IF the re-invocation after a RotateSession also triggers a trope, THEN THE Agent_Pool SHALL emit AgentBlocked, create a persisted review item, set the agent's blocked flag, and sleep
5. THE Agent_Pool SHALL perform at most 1 automatic retry per trope detection before escalating to a review item

### Requirement 4: Emit TaskStageChanged from agent loop

**User Story:** As a dispatcher operator, I want the agent loop to emit TaskStageChanged after advancing a task, so that the run loop creates BufferApproval review items and tasks do not get stuck.

#### Acceptance Criteria

1. WHEN board.advance() succeeds in handle_agent_result(), THE Agent_Pool SHALL emit TaskStageChanged with the task_id, source column, and target column before emitting AgentCompleted
2. THE Agent_Pool SHALL emit both TaskStageChanged and AgentCompleted in that order for every successful agent completion that advances a task

### Requirement 5: WIP limit enforcement

**User Story:** As a daemon operator, I want WIP limits enforced at runtime, so that columns do not exceed their configured capacity and work flows smoothly through the pipeline.

#### Acceptance Criteria

1. THE Task struct SHALL NOT contain a wip_limit_blocked field
2. WHEN an agent completes work and requests advancement to a target column, THE Dispatcher SHALL check wip_limits.allows(target, board.count(target)) before advancing
3. IF the target column is at its WIP limit, THEN THE Dispatcher SHALL leave the task in its current column and put the agent back to sleep
4. IF the target column is at its WIP limit, THEN THE Dispatcher SHALL record which agent is waiting to advance into that column
5. WHEN a task leaves a column that was previously at its WIP limit, THE Dispatcher SHALL re-signal the agent whose completed task was waiting to advance into that column

### Requirement 6: AgentBlocked flow

**User Story:** As a daemon operator, I want agents to block and create review items when they need human input, so that questions surface in the TUI and agents resume with answers in context.

#### Acceptance Criteria

1. WHEN the agent backend signals a question (via a structured AgentResult variant or marker), THE Agent_Pool SHALL create a persisted review item of kind AgentQuestion, emit AgentBlocked, set the agent handle's blocked flag, and sleep on Notify
2. WHEN a human answers an agent question via the TUI, THE Dispatcher SHALL record the answer on the review item resource, call unblock(role, answer), and signal the agent to wake
3. WHEN a blocked agent wakes after receiving an answer, THE Agent_Pool SHALL load the review item and answer into the agent's prompt context and retry the task

### Requirement 7: Config role validation and hardcoded defaults

**User Story:** As a daemon operator, I want unknown agent role names in config to fail on startup, so that typos are caught early and each role always resolves to a valid agent definition.

#### Acceptance Criteria

1. THE System SHALL define a hardcoded default AgentDef (model and system prompt) for each AgentRole variant
2. WHEN config is loaded and an agents entry specifies a role name that does not match any AgentRole variant, THE System SHALL return an error and refuse to start
3. WHEN resolve_agent(role) is called, THE Config SHALL check configured agents first, then fall back to the hardcoded default for that role
4. THE Config SHALL NOT fall back to the first agent in the list when a role is not found

### Requirement 8: Daemon HOME discovery

**User Story:** As a daemon operator, I want the systemd unit to know the user's HOME at install time, so that the daemon can locate ~/.gitzi and repo paths regardless of cwd.

#### Acceptance Criteria

1. WHEN the CLI installs the systemd unit, THE Installer SHALL write `Environment=HOME=/home/<user>` into the unit file using the current user's HOME value
2. WHEN the daemon starts, THE System SHALL read $HOME to locate ~/.gitzi
3. THE Agent_Pool SHALL use the repo path from session state for git operations, not PathBuf::from(".")

### Requirement 9: Add Deserialize to DispatchEvent

**User Story:** As a TUI developer, I want to deserialize DispatchEvent from JSON, so that the TUI can parse event streams from the daemon socket.

#### Acceptance Criteria

1. THE DispatchEvent enum SHALL derive both Serialize and Deserialize
2. FOR ALL valid DispatchEvent values, serializing to JSON then deserializing back SHALL produce an equivalent value (round-trip property)

### Requirement 10: Fix double-signal on approval

**User Story:** As a daemon operator, I want approval to signal the target agent exactly once, so that agents do not receive spurious wake-ups.

#### Acceptance Criteria

1. WHEN approve() is called, THE Dispatcher SHALL signal the target agent exactly once
2. THE run loop SHALL NOT signal the agent again when processing the HumanApprovalReceived event that approve() already handled

### Requirement 11: Fix commit.summary() API for git2 0.21

**User Story:** As a developer, I want the resume_context function to compile correctly against git2 0.21, so that boot resume works without type errors.

#### Acceptance Criteria

1. THE resume_context function SHALL call commit.summary() and handle its return type as `Option<&str>` (git2 0.21 API)
2. WHEN commit.summary() returns None, THE resume_context function SHALL use a fallback string "(no message)"
