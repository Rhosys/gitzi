# gitzi — Agent Harness Plan

## Overview

**gitzi** is a Rust-based Kanban pipeline harness that orchestrates AI coding agents
across a software development lifecycle. It is **LLM-driven with human in the loop**:
the LLM proposes, the human approves. The harness tracks epics and tasks, assigns work
to agents, enforces WIP limits, and optimizes every interaction with the human reviewer
by surfacing small, reviewable diffs and driving test-first development.

---

## Core principle: one thing at a time

The user is never presented with more than one thing requiring their attention at once.
This applies everywhere without exception:

- Clarification queue → one ADR surfaced at a time
- Attention queue → one background agent request surfaced at a time
- Diff review → one task reviewed at a time
- Opening status → surfaces the single most important thing first

Queues may be long. The user works through them sequentially. The harness never
front-loads, never batches, never summarises-and-picks. One thing. Then the next.

---

## Goals

1. **LLM-driven, human-in-the-loop** — the LLM proposes everything (epic breakdown,
   task splits, prioritization, implementation); the human approves at each gate.
   Nothing moves forward without human sign-off.
2. **Pipeline orchestration** — move work through Kanban stages automatically by
   dispatching agents and reacting to outcomes.
3. **Epic & task management** — LLM breaks epics into tasks, suggests sizing and order;
   human refines and approves before work begins.
4. **WIP tracking** — enforce limits at each stage; surface bottlenecks; prevent
   runaway parallel work.
5. **User-optimized review loop** — every agent output is a small, testable diff that
   the human can approve or reject before the pipeline continues.
6. **Pluggable agent backends** — Claude Code CLI, AWS Kiro CLI, Ollama, ChatLM UI,
   and others can be swapped in per task without changing the harness.
7. **Optional external sync** — GitHub Issues, Jira, and Linear are sync targets, not
   the source of truth.

---

## Kanban Pipeline Stages

```
Backlog → Prioritized → In Progress → Waiting for Review → In Testing → Done
```

| Stage               | Meaning                                                              | Who acts           |
|---------------------|----------------------------------------------------------------------|--------------------|
| **Backlog**         | Unrefined ideas, auto-generated tasks, or imported work             | Harness / human    |
| **Prioritized**     | Refined, sized, and ordered; ready to be picked up                  | Human approval     |
| **In Progress**     | Assigned to an agent actively working on it                         | Agent              |
| **Waiting for Review** | Agent finished; diff ready for human diff-review approval loop   | Human              |
| **In Testing**      | Human approved diff; automated tests running                        | Harness / CI       |
| **Done**            | Tests passed; work published/merged                                 | Harness            |

WIP limits are configurable per stage (default: 1 In Progress, 3 Waiting for Review).
The system is designed to run **parallel but heavily single** — multiple tasks can be
In Progress simultaneously, but the default WIP limit of 1 keeps the review loop tight.
Raise the limit deliberately when confidence is high.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        gitzi harness                         │
│                                                             │
│  ┌──────────┐   ┌───────────────┐   ┌───────────────────┐  │
│  │ Scheduler │   │  Pipeline     │   │   WIP Tracker     │  │
│  │ (cron /  │──▶│  Orchestrator │──▶│  (git-file state) │  │
│  │  events) │   │               │   └───────────────────┘  │
│  └──────────┘   └──────┬────────┘                          │
│                         │                                   │
│            ┌────────────┴────────────┐                     │
│            ▼                         ▼                     │
│   ┌────────────────┐      ┌─────────────────────┐         │
│   │  Agent Runner  │      │  Review Gate        │         │
│   │  (pluggable)   │      │  (diff + approval)  │         │
│   └────────────────┘      └─────────────────────┘         │
│         │                           │                       │
│   ┌─────┴───────┐            ┌──────┴──────────┐           │
│   │ Backends    │            │ Notifications   │           │
│   │ - Claude Code CLI        │ - GitHub comment│           │
│   │ - AWS Kiro  │            │ - Slack webhook │           │
│   │ - Ollama    │            └─────────────────┘           │
│   │ - ChatLM UI │                                          │
│   └─────────────┘                                          │
│                                                             │
│  ┌──────────────────────────────────────────────────────┐  │
│  │              Web Dashboard (Leptos + Axum)            │  │
│  └──────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

---

## State: Files in Git

All pipeline state lives as TOML files. The harness uses **three distinct branch types**,
none of which block or stomp on each other:

| Branch | Purpose | Who merges |
|--------|---------|------------|
| `main` (or default) | Source of truth; production code | Human, via PR |
| `gitzi/state` | All `.gitzi/` TOML state changes (tasks, epics, wip) | Human, via PR into main |
| `gitzi/<task-id>-<slug>` | One per task; agent code changes in a worktree | Human, via PR into main |

The harness **never commits to main directly**. Every write goes to a branch the human
reviews first. `config.state_branch` (default: `"gitzi/state"`) is configurable.

**Git mechanics:**
- State commits use direct git object writes (blob → tree → commit onto the branch ref)
  — the main working tree's index is never touched.
- Task branches use linked git worktrees in `.gitzi/worktrees/<name>/` — the agent
  subprocess runs there; the main workspace stays clean.

```
.gitzi/
  config.toml          # harness config (WIP limits, state_branch, agent defaults)
  epics/
    <epic-id>.toml     # epic metadata + child task list
  tasks/
    <task-id>.toml     # task metadata, stage, assigned agent, history
  wip.toml             # current stage snapshot (auto-generated)
  worktrees/
    gitzi-<task-id>-<slug>/   # linked worktree per active task (temp)
```

Each task gets its own branch: `gitzi/<task-id>-<slug>` (e.g. `gitzi/task-001-add-login`).
Branches are created by the harness when the task enters In Progress and merged/deleted on Done.

**Task file shape (sketch):**
```toml
id = "task-001"
epic = "epic-001"
title = "Add login endpoint"
stage = "in-progress"
agent = "claude-code"
branch = "gitzi/task-001-add-login"
wip_limit_blocked = false
created_at = "2026-06-01T00:00:00Z"
updated_at = "2026-06-01T00:00:00Z"

[history]
# append-only log of stage transitions
```

**`config.toml` test command (no adapter needed):**
```toml
test_command = "cargo test"   # or "npm test", "pytest", etc.
```

---

## Agent Runner (Pluggable)

**LLM / tool-call layer: [Rig](https://github.com/0xPlaygrounds/rig) (`rig-core` crate)**

Rig provides the Rust-native LLM abstraction: provider clients, tool calling, streaming
responses, and agent loops. gitzi owns the orchestration state machine (kanban, WIP,
review gate); Rig handles the LLM I/O underneath each backend.

```
gitzi orchestrator
    └── AgentBackend trait
            ├── RigAgent  ←─ rig-core (Claude, Ollama, Bedrock, OpenAI, …)
            ├── ClaudeCodeCli  ←─ subprocess, stream-json output
            ├── KiroCli        ←─ subprocess
            └── ChatLmUi       ←─ HTTP API
```

Each backend implements a common trait:

```rust
trait AgentBackend {
    async fn run(&self, task: &Task, context: &RunContext) -> AgentResult;
}
```

| Backend        | Invocation                                      | Notes                            |
|----------------|-------------------------------------------------|----------------------------------|
| `rig-agent`    | `rig-core` — direct API calls via Rig           | Primary; 20+ providers, tool use |
| `claude-code`  | `claude --print --output-format stream-json`    | Rich agentic sessions, file edits|
| `kiro`         | `kiro run <task-prompt>`                        | AWS IDE agent CLI                |
| `ollama`       | Rig's Ollama provider (HTTP)                    | Local/offline tasks              |
| `chatlm-ui`    | HTTP API or CLI wrapper                         | ChatLM agent sessions            |

The backend for a task is set in the epic or task TOML, with a fallback to the
`config.toml` default. Rig's provider abstraction means switching between Claude,
Ollama, and Bedrock requires only a config change, not code changes.

---

## Review Gate (Diff-Review Approval Loop)

When a task reaches **Waiting for Review**, the harness:

1. Renders the git diff for the agent's branch.
2. Posts it as a GitHub PR comment (or Slack message) for the human.
3. Exposes an approval endpoint in the web dashboard.
4. On **approve** → moves task to In Testing, runs TDD suite.
5. On **reject with feedback** → sends feedback back to agent, returns task to In Progress.

The batch size is kept small by enforcing that each task must fit in a single reviewable
diff (configurable line limit, default 200 LOC changed).

---

## Dashboard (Leptos + Axum)

A web app served by the harness process itself.

- **Kanban board view** — drag tasks (or click-to-advance); WIP counts per column.
- **Task detail** — diff viewer, agent log, approval buttons, reject-with-comment.
- **Epic view** — progress bar, child task list, create/split task.
- **Live updates** — SSE (Server-Sent Events) from the Axum backend.

---

## Integrations

Integrations are **sync targets**, not sources of truth. The harness pushes state outward.

| Integration    | What syncs                                      | Direction         |
|----------------|-------------------------------------------------|-------------------|
| GitHub Issues  | Tasks ↔ issues; PRs created per task branch    | Bidirectional     |
| Jira           | Tasks → Jira tickets (status updates)           | Push only (v1)    |
| Linear         | Tasks → Linear issues (status updates)          | Push only (v1)    |

### Mobile / Notifications (Future)

Slack is out of scope for now. Instead, a **mobile app** will connect directly to the
git repo for review notifications and approvals. The protocol for this is TBD — likely
a federated, git-native approach (e.g. reading `.gitzi/` state directly, or a lightweight
push channel over a protocol to be defined). The mobile experience is a first-class goal
(approve/reject diffs, view kanban) but deferred past MVP.

---

## Orchestration Layer Decision

**Decision: custom Rust orchestration using Rig (`rig-core`) as the LLM layer.**

Evaluated options:

| Option              | Verdict                                                                 |
|---------------------|-------------------------------------------------------------------------|
| **Rig** (`rig-core`)| ✅ **Chosen** — pure Rust, 20+ providers, tool calling, streaming, GA  |
| AWS Strands SDK     | ❌ Python-only; requires subprocess/FFI boundary from Rust              |
| Kong Agent Gateway  | Optional future layer for A2A routing/policy if multi-tenant needed    |
| AutoAgents          | Alternative if actor-model multi-agent coordination is needed later     |

Rig provides the LLM/tool abstraction; gitzi owns the kanban state machine, WIP limits,
and review gate on top. Provider switching (Claude ↔ Ollama ↔ Bedrock) is config-only.

---

## Tech Stack

| Layer              | Choice                          |
|--------------------|---------------------------------|
| Layer              | Choice                          |
|--------------------|---------------------------------|
| Language           | Rust (2024 edition)             |
| LLM / agent SDK    | `rig-core` (Rig)                |
| Web framework      | Axum                            |
| Frontend           | Leptos (SSR + CSR)              |
| State storage      | TOML files in git               |
| Scheduling         | Tokio + cron-like timer         |
| Git operations     | `git2` crate (libgit2 bindings) |
| HTTP client        | `reqwest`                       |
| Serialization      | `serde` + `toml`                |
| CLI                | `clap`                          |
| TDD runner         | `cargo test` + custom adapter   |

---

## MVP Scope

The MVP proves the core loop end-to-end:

1. Human creates an epic + tasks in `.gitzi/` TOML files.
2. Harness picks the top prioritized task, dispatches it to the Claude Code CLI backend.
3. Agent produces a branch + diff.
4. Harness posts diff to dashboard and sends Slack notification.
5. Human approves via dashboard.
6. Harness runs `cargo test`; on pass, marks task Done and commits state.

Everything else (Jira/Linear sync, multi-agent parallelism, Kiro/Ollama backends,
epic auto-splitting) is post-MVP.

---

## User Interaction Model

### Chat is the primary interface

The chat is an intelligent AI-driven interface — not a command language. Natural language
is the norm. The system understands the project context and reasons about it.

### Session opening — what the chat surfaces first

When a session starts, the chat proactively surfaces what needs the user's attention:

- **Current epic** — what is in flight right now
- **Open work** — tasks being implemented, their status
- **Waiting for human input** — anything blocked on the user (approvals, questions, decisions)
- **Open questions** — questions raised by agents or the harness that the user hasn't answered
- **Followups from past conversations** — unresolved threads from previous sessions

The chat IS the status view. It is not just an input box — it is the primary information surface.

### What the chat does

A single persistent chat thread influences the creation and unblocking of:
- Epics
- Tasks
- Work items within tasks
- Tests (unit, component, integration, e2e)
- Open questions / clarifications for subagents

The user never leaves the chat to "go create a task" — the chat creates it, proposes it,
and the user confirms or redirects inline.

The `role = "main"` agent powers the chat harness. It is defined in `[[agents]]` like
any other agent and gets the full gitzi management tool set. Its system prompt is focused
on project coordination — understanding user intent, creating and refining epics/tasks,
walking through the clarification queue, and surfacing status — not implementation.

```toml
[[agents]]
role = "main"
model = "claude-opus-4-8"
system_prompt = """
You are a project coordination agent. You help the user manage their software project
by creating epics, tasks, and ADRs from natural conversation. You never write code
directly. You surface what needs the user's attention and keep work moving.
"""
```

### Chat is an agentic tool-use loop

Every user message — without exception — goes to the `role = "main"` agent. There is no
command parser and no shortcut path. Even inputs that look like navigation ("show the
board") go through the model, because the user rarely intends a bare command and when
they do there is usually a conversation to be had about it.

When the user sends a message, it goes to the configured LLM along with the full chat
history and current project state. The LLM has a set of **gitzi management tools** it
can call directly in the same turn:

| Tool | What it does |
|------|-------------|
| `gitzi_create_epic` | Create a new epic |
| `gitzi_create_task` | Create a task under an epic |
| `gitzi_create_adr` | Raise a clarification item / pending ADR |
| `gitzi_resolve_adr` | Record a decision on a pending ADR |
| `gitzi_update_task` | Edit task title, description, work items |
| `gitzi_prioritize_task` | Set task priority / order |
| `gitzi_park_task` | Park a task and record its state |
| `gitzi_list_epics` / `list_tasks` | Read project state |
| `gitzi_get_adr` | Fetch ADR details |

The LLM reasons about the user's message, calls whatever tools are needed, and responds
with what it did. "Let's build user authentication" → LLM creates the epic, breaks it
into tasks, responds with a summary and asks for confirmation. All in one turn.

Responses are **never streamed** — the full response is displayed at once when complete.

**Tool call visibility:**

| Agent | Tool calls visible in chat? | Panel updates? |
|-------|-----------------------------|----------------|
| `role = "main"` | Yes — the user sees what was created/changed | Yes — panel switches to reflect the action |
| Coding agents (background) | No — run silently | No — status panel updates only when a task stage changes |

Background agents are workers. The user sees their outcomes (task moves to Waiting for
Review, a notification appears in the status panel) but not their intermediate actions.

**Two contexts, overlapping tool sets:**

| Tool | Chat LLM | Coding agent |
|------|----------|--------------|
| `gitzi_create_epic` | ✓ | — |
| `gitzi_create_task` | ✓ | ✓ (dependency discovery) |
| `gitzi_create_adr` | ✓ | ✓ (raise clarification items) |
| `gitzi_resolve_adr` | ✓ | — |
| `gitzi_update_task` | ✓ | ✓ (own task only) |
| `gitzi_park_task` | ✓ | ✓ (own task only) |
| `gitzi_prioritize_task` | ✓ | — |
| `gitzi_list_epics` / `list_tasks` | ✓ | ✓ (read project context) |
| `gitzi_get_adr` | ✓ | ✓ (read decisions) |
| `gitzi_switch_panel` | ✓ | — |
| `Bash`, `Edit`, `Write`, `Read`, `Glob`, `Grep` | — | ✓ |

Coding agents have enough gitzi access to manage their own work autonomously — creating
dependencies, surfacing uncertainty as ADRs, parking and resuming — without going back
through the chat harness for every action.



From the user's perspective the chat is **one infinite thread** — there are no visible
session boundaries.

Under the hood the harness manages sessions transparently:

1. Each session maintains a **rolling summary** — updated continuously as the conversation
   progresses (see Conversation Summarization in todo.md).
2. When `shouldStartNewSession()` returns true, the harness:
   - Finalises the current session's summary
   - Opens a new session
   - Seeds the new session with the previous summary as its first context message
3. The user sees the conversation continue without interruption.

**`shouldStartNewSession()` fires when a conversation feels complete**, for example:
- The clarification queue is empty and no agents are active
- An epic has just reached Done
- A natural pause in work (all open tasks are either Done or waiting on the user)
- The AI detects the conversation has reached a resolution point

The session boundary is an implementation detail the user never needs to know about.

### Terminology

"Issue" and "task" are the same thing. The canonical term throughout gitzi is **task**.

### Artifact hierarchy

```
Epic
  └── Task
        ├── Work items (sub-steps within the task)
        ├── TDD spec / tests (written first, before implementation)
        ├── Clarifications for subagents (inline context, implementation notes)
        └── Open questions (raised by agent; require human answer before proceeding)
```

Tests are **first-class artifacts**, not afterthoughts:
- Unit, component, integration, and end-to-end tests are all tracked
- TDD: the agent writes failing tests first; the tests are part of the task definition
- Passing tests are the gate to Done — they are the artifact that proves the task is complete

Code is primary documentation. Human-readable markdown docs are generated rarely and only
when they add something code cannot convey.

### Agent dependency discovery (auto-park and resume)

When an agent discovers mid-task that prerequisite work is missing, it follows this flow
**without blocking on the user first**:

1. **Park** — commit the current partial work to the task's branch and record its state.
2. **Create dependency** — generate a new task (with TDD, work items, and a proposed
   implementation) for the missing prerequisite work.
3. **Notify** — explain to the user in chat: what it was working on, what it discovered,
   what new task it created, and what it proposes to do. Ask the user to confirm the
   proposed implementation is correct.
4. **Resume** — once the dependency task is Done (either via the agent's proposed
   implementation or a version the user edited), automatically pull in the new work,
   rebase / rework the parked branch, and continue the original task.

**When to auto-create a dependency vs raise a clarification item:**

| Situation | Action |
|-----------|--------|
| A prerequisite simply doesn't exist yet and what it needs to be is unambiguous | Auto-create dependency task, park, notify, resume |
| Anything about intent, approach, scope, or implementation is uncertain — no matter how small | Raise clarification item; stop until user decides |

The second row has **no size threshold**. A tiny uncertainty is still a clarification item.
The queue may grow large; that is expected and correct.

### System prompt composition

Agent prompts are assembled from layers at runtime:

1. **Base mandate** (hardcoded, not configurable) — the implementer-not-designer
   principle applied to every agent, always:
   > You are an implementer. You do not make design decisions. You do not guess about
   > intent, approach, naming, structure, or scope — no matter how small. If anything
   > is unclear, raise a clarification item and stop. The user's answer is always correct.
2. **Role prompt** — defined per agent in `[[agents]]` in config.toml via `system_prompt`
3. **Dynamic context** (injected at dispatch time):
   - Session summary
   - All ADRs linked to the current task and its parent epic (both `pending` and `resolved`)
   - Current epic context
   - Open clarification items for this task

### Core agent principle: the user is the expert

The agent's role is **implementation only**. The user is the designer, architect, and
domain expert. The agent has no opinions about what to build or how.

This is encoded in every agent's system prompt:

> You are an implementer. You do not make design decisions. You do not guess about intent,
> approach, naming, structure, or scope — no matter how small the question seems. If
> anything is unclear, raise a clarification item and stop. The user's answer is always
> the correct answer. Your job is to execute what has been explicitly decided, nothing more.

There is no threshold for "small enough to guess." Every uncertainty surfaces.

### Clarification queue

When an agent hits ambiguity it cannot resolve on its own, it stops and raises a
**clarification item** — it does not guess and proceed.

Before surfacing the question the agent:
1. Researches the problem (web search, codebase analysis)
2. Identifies multiple solution paths
3. Compiles pros and cons for each path

The clarification item is added to a **clarification queue** with a UUID. If multiple
tasks raise blockers simultaneously, all items accumulate in the queue. The chat then
walks the user through them **one at a time** in order until every item has a resolution.

The harness walks through pending ADRs **one at a time** in chat. Each message includes
the ADR UUID so it is visible to the user. The backend tracks which ADR is currently
awaiting a response; the user's next reply is automatically mapped to it — no explicit
reference required from the user.

Each clarification item records:
- UUID
- Which task/work item raised it
- The question / decision needed
- Research context the agent gathered
- The candidate solution paths with pros/cons
- The user's decision (filled in on resolution)
- Timestamp raised / timestamp resolved

Once resolved, the answer is fed back to the agent so it can continue. The resolution
is also stored permanently as part of the task record — it is a decision artifact, not
just a transient message.



### TUI layout

Two-pane split: **35% chat left / 65% right panel.**

The right panel switches between views based on context:

| View | When shown |
|------|-----------|
| **Status** (default on open) | Structured harness-rendered opening card |
| **Board** | Kanban board across all stages |
| **Task detail** | Selected task — description, work items, diff, approve/reject |
| **ADR detail** | Selected ADR — question, options, decision |
| **Clarification queue** | Pending ADRs awaiting user answers |

### Opening status panel

When gitzi opens, the right panel renders a structured **status card** assembled directly
from harness state — not an AI-generated message:

- **Current epic** — title, progress (tasks done / total)
- **In progress** — tasks currently being worked on by agents
- **Waiting for you** — tasks in Waiting for Review (need approval/rejection)
- **Clarification queue** — count of pending ADRs awaiting your answer
- **Followups** — unresolved items carried forward from the previous session summary

The chat pane starts empty and ready for input. The panel is what the user reads first;
the chat is where they act on it.

### Diff review

Diff review happens in the **Task detail** view of the right panel.
The right panel shows the diff; approve/reject controls are there.

### Architecture Decision Records (ADRs)

ADRs are a first-class artifact type, stored separately from tasks and epics.

Every resolved clarification item produces an ADR. ADRs can also be created directly
from chat when a significant design decision is made outside of a task context.

**Storage:** `~/.gitzi/<session>/adrs/<uuid>.toml`

**Contents:**
- UUID
- Title / decision summary
- Context (what problem was being solved)
- Options considered with pros/cons
- Decision made and rationale
- Consequences / follow-on implications
- Linked task(s) and epic(s) that triggered or reference this decision
- Resolution type: `human` | `agent-self-resolved`
- Author (human or agent) + timestamp raised / timestamp resolved

**Linking:** Tasks and epics carry an `adrs = ["<uuid>", ...]` field.
The chat surfaces relevant ADRs when working on related tasks.

**ADRs are created immediately when a clarification item is raised — before the user
answers.** The ADR starts in `pending` status with the question, context, and candidate
options filled in. When the user answers, the ADR is updated to `resolved` with the
decision and rationale. The clarification item and the ADR are the same thing at
different points in their lifecycle.

**ADR lifecycle — two resolution paths:**

```
Human-resolved (agent needs user input):
  background agent hits uncertainty
    → creates pending ADR
    → enqueues in attention queue
    → agent parks and waits
  main agent becomes idle
    → harness pulls next item from attention queue
    → surfaces ADR to user through chat
  user answers
    → ADR resolved
    → background agent resumes

Agent self-resolved (agent decides autonomously):
  background agent makes a structural decision (dependency task, park, etc.)
    → creates ADR with its own reasoning as the answer
    → marked agent-resolved
    → proceeds immediately
    → ADR visible in status panel for user to review / override at their own pace
```

**Key principles:**
- Background agents never interrupt an active user conversation
- The main agent is the single point of contact between the user and all background work
- Items are surfaced **one at a time** — always (see Core principle above)

**Attention queue** — stored in harness state, contains:
- ADR UUID
- Which task raised it
- Priority / order raised
- Status: `waiting` | `surfaced` | `resolved`

**ADRs are always injected into agent context.** When any agent picks up a task, the
harness fetches all ADRs linked to that task (and its parent epic) and includes them in
the system prompt:
- `resolved` ADRs tell the agent what has been decided — implement accordingly
- `pending` ADRs tell the agent what is still open — do not proceed on those areas

This ensures agents never re-ask a question that has already been answered, and never
act on an area where a decision is still pending.

**Every ADR also produces a test.** When a clarification item is resolved, the agent
generates a unit test that:
- Validates that the chosen solution is correctly implemented
- Carries the problem statement and chosen solution in its doc comment
- Links to the originating task/issue and the ADR by UUID

This test is the living proof that the decision holds. If the implementation drifts, the
test fails and the ADR UUID in the failure points directly back to why the decision was made.

---

## Open Questions

- [x] **Orchestration layer** — custom Rust with `rig-core` as LLM/tool layer (AWS Strands rejected: Python-only)
- [x] **Multi-agent parallelism** — parallel supported, default WIP limit 1 (raise deliberately)
- [x] **Branch strategy** — one branch per task (`gitzi/<task-id>-<slug>`)
- [x] **Slack** — dropped; mobile app via federated git-native protocol (TBD)
- [x] **Test adapter** — not needed; `test_command` in `config.toml`
- [x] **Epic auto-generation** — LLM proposes epic breakdowns, task splits, and prioritization; human approves each step
- [ ] **Mobile protocol** — define the federated protocol for mobile ↔ git repo communication
