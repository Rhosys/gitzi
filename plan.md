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

- Clarification queue → one review item surfaced at a time
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

## State: `~/.gitzi/` (global, no init required)

All pipeline state lives as TOML files in the user's home directory. There is no
`gitzi init` — the daemon and CLI create directories lazily on first access.

There are no sessions or UUIDs — one global set of epics, tasks, and board state
shared across all repos and agents.

```
~/.gitzi/
├── config.toml              # harness config (WIP limits, agents, repo_paths)
├── board.toml               # derived: which task is in which column
├── plan/
│   ├── epics/<id>.toml      # epic metadata + child task list
│   ├── tasks/<id>.toml      # task metadata, stage, assigned agent, history
│   └── reviews/<uuid>.toml  # review items (clarifications, decisions)
├── chats/
│   └── <id>.jsonl           # one file per chat session (user ↔ main agent)
└── tmp/
    ├── daemon.sock          # unix socket for TUI ↔ daemon IPC
    ├── mcp.sock             # unix socket for MCP HTTP server
    ├── cache/
    │   └── repos/<slug>.toml  # repo summaries, labels, commit counter
    └── tasks/<id>/
        ├── worktrees/       # git linked worktrees per active task
        └── agent.log        # raw agent output for this task
```

### Lazy initialization

Every path helper (`plan_dir()`, `board_file()`, etc.) calls `create_dir_all` on the
parent before reading or writing. If `~/.gitzi/` doesn't exist, the first operation
creates it. No explicit init step exists.

### Repo discovery

Repos are discovered dynamically from glob patterns in `config.toml`:

```toml
repo_paths = [
    "/home/warren/git/claude/*",
    "/home/warren/projects/side-*",
]
```

On startup (and periodically via `watch_config`), the daemon:
1. Expands each glob pattern
2. Filters to directories containing a `.git/` directory
3. Loads cached summaries from `tmp/cache/repos/`
4. For new or stale repos, generates a summary:
   - **Heuristics first:** reads package.json, Cargo.toml, README first line, directory name
   - **LLM fallback:** for repos where heuristics produce insufficient labels
5. Writes cache entries

**Repo cache entry (`tmp/cache/repos/<slug>.toml`):**
```toml
path = "/home/warren/git/claude/email-catcher/backend"
slug = "email-catcher-backend"
summary = "TypeScript Lambda API using Hono, processes incoming emails"
labels = ["typescript", "aws", "hono", "email"]
commits_by_gitzi = 12
last_scanned = 2026-06-20T10:00:00Z
```

`commits_by_gitzi` is incremented per repo after each completed task that touched it.

### Task branches

Each task gets its own branch: `gitzi/<task-id>-<slug>` (e.g. `gitzi/task-001-add-login`).
Tasks may span multiple repos. The harness creates linked worktrees in
`tmp/tasks/<id>/worktrees/<repo-slug>/` — one per repo the task touches.

The harness **never commits to main directly**. Every write goes to a branch the human
reviews first.

**Task file shape:**
```toml
id = "task-001"
epic = "epic-001"
title = "Add login endpoint"
stage = "in-progress"
agent = "claude-code"
branch = "gitzi/task-001-add-login"
repos = ["email-catcher-backend", "email-catcher-infrastructure"]
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
by creating epics, tasks, and review items from natural conversation. You never write code
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
| `gitzi_create_review_item` | Raise a clarification item / pending review item |
| `gitzi_update_task` | Edit task title, description, work items |
| `gitzi_prioritize_task` | Set task priority / order |
| `gitzi_park_task` | Park a task and record its state |
| `gitzi_list_epics` / `list_tasks` | Read project state |
| `gitzi_get_review_item` | Fetch review item details |

Resolving a review item (recording the human's decision) is never an agent-callable
tool — it only happens through the TUI's review pane, per the one-thing-at-a-time
principle. Agents raise review items; only the human resolves them.

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
| `gitzi_create_review_item` | ✓ | ✓ (raise clarification items) |
| `gitzi_update_task` | ✓ | ✓ (own task only) |
| `gitzi_park_task` | ✓ | ✓ (own task only) |
| `gitzi_prioritize_task` | ✓ | — |
| `gitzi_list_epics` / `list_tasks` | ✓ | ✓ (read project context) |
| `gitzi_get_review_item` | ✓ | ✓ (read decisions) |
| `gitzi_switch_panel` | ✓ | — |
| `Bash`, `Edit`, `Write`, `Read`, `Glob`, `Grep` | — | ✓ |

Coding agents have enough gitzi access to manage their own work autonomously — creating
dependencies, surfacing uncertainty as review items, parking and resuming — without going back
through the chat harness for every action.

### Idle state — proactive planning mode

When all queues are empty and no agents are active, the main agent does not go silent.
It shifts into proactive planning:

1. **First:** check if any background agents created new tasks or epics via the async
   queue — if so, propose those first (they represent work already identified and queued)
2. **Otherwise:** review recently completed work and the backlog, find improvement
   opportunities, and produce one recommendation for what to work on next with reasoning

In both cases the main agent surfaces **one item** to the user and switches the right
panel to show the proposed artifact — Task detail if a task, Epic detail if a new epic.

The user can accept, redirect, defer, or start a new conversation. One recommendation
at a time — never a ranked list.

### Async queue — needs detailed design



### Interrupt classification (message sent while LLM is processing)

The chat input is never blocked. When the user submits a message while the main agent
is mid-turn, a lightweight classifier determines what to do:

```
User sends message while chat_pending == true
    │
    ▼
Classifier call (same LLM endpoint, minimal prompt, single-word response):
  Input: the pending user message, the new message, last 5 turns of context
  Output: one of { amend, queue, fork }
    │
    ├─ amend → cancel in-flight request, concatenate both messages, retry as one turn
    ├─ queue → hold new message, deliver it after the current response arrives
    └─ fork  → spawn a new chat session (clone last 50 turns), process new message
               there, pop back to main session when the fork completes
```

**Classifier prompt:**

```
System: You are a message router. Given a conversation context, a pending message
currently being processed, and a new message from the user, classify the new message.
Respond with exactly one word: amend, queue, or fork.

- amend: the new message updates, corrects, or supersedes the pending message
- queue: the new message is related and can wait until the current response finishes
- fork: the new message is a completely different thought unrelated to the pending topic

Respond with one word only.
```

**Implementation details:**
- Uses the same LLM provider as the main agent (no separate model needed)
- No reasoning tokens, no tool use — prompt designed for minimal output
- If the classifier fails or times out, default to `queue` (safest)
- `amend`: requires cancellation support on the in-flight HTTP request (use
  `tokio::select!` with a cancel token or `reqwest` request abort)
- `fork`: creates a new file in `~/.gitzi/chats/<uuid>.jsonl`, copies last 50 turns,
  processes there. TUI shows a visual indicator that a fork is active. When the fork
  response arrives, it is displayed inline (with a separator) and the fork session is
  marked complete. Main session resumes normally.

### Fork sessions

Forks are conversation branches that run in parallel. When the classifier returns `fork`,
the harness:

1. **Names the fork** — a second quick LLM call generates a 2-4 word topic name from
   the new message.
2. **Pushes onto the fork stack** — the stack is unbounded. Each entry has:
   - `id` (uuid)
   - `name` (topic label)
   - `chat_history` (own message list, seeded from last 50 turns of parent)
   - `abort_handle` (for amend/cancel)
3. **Routes all subsequent messages to the top of stack** — the classifier always operates
   against the topmost active entry. This means forks nest: a fork-within-a-fork pushes
   another entry. No depth limit.

**TUI rendering with active forks:**

```
┌──────────────────────────────────────────────────────────────────────┐
│ gitzi ●  -- #3: OAuth Provider Config                                │
├───┬──────────────────────────┬───────────────────────────────────┬───┤
│ 1 │                          │                                   │ S │
│ 2 │   Chat (fork #3 context) │   Right panel                     │ E │
│[3]│                          │                                   │ K │
│   │                          │                                   │ T │
│   ├──────────────────────────┤                                   │ L │
│   │ > input           [esc]  │                                   │   │
├───┴──────────────────────────┴───────────────────────────────────┴───┘
│ [enter] send  [esc] close fork  [arrows] board  [ctrl+q] quit        │
└──────────────────────────────────────────────────────────────────────┘
```

- **Left strip** (3 chars, between chat and main left edge): numbered fork entries,
  active one highlighted. Hidden when stack is empty (no forks active).
- **Header**: shows `#N: Fork Name` to identify the active context.
- **Esc**: closes the current fork (pops the stack). Always available to the user.

**Fork closure:**

```toml
# config.toml
fork_auto_close = true  # default: agent can close forks
```

- **`fork_auto_close = true`** (default): The main agent receives a `gitzi_close_fork`
  tool when operating inside a fork. After responding, if the agent believes the thread
  is resolved, it calls `gitzi_close_fork` with a summary. This pushes a closing message
  to the fork's chat and pops the stack. If the fork has unresolved ambiguity, the agent
  doesn't call it and the fork stays open for more user input.
- **`fork_auto_close = false`**: The agent never receives the `gitzi_close_fork` tool.
  Only the user can close forks (Esc in TUI, button in web UI).
- **User can always close**: Esc/button works regardless of the config. Agent close is
  additive, not exclusive.
- **Race condition**: If the agent calls `gitzi_close_fork` but the user has already
  submitted a new message, the close is a noop — the user's new message takes priority.

**`gitzi_close_fork` tool:**
```json
{
  "name": "gitzi_close_fork",
  "description": "Close the current fork session. Call this when the forked topic is resolved.",
  "parameters": {
    "type": "object",
    "properties": {
      "summary": { "type": "string", "description": "One-sentence summary of what was decided." }
    },
    "required": ["summary"]
  }
}
```

**Storage:** Each fork's chat lives at `~/.gitzi/chats/<fork-uuid>.jsonl`. On close,
the fork file is kept (for audit/history) but marked complete via a final system message.
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
   - All review items linked to the current task and its parent epic (both `pending` and `resolved`)
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

The harness walks through pending review items **one at a time** in chat. Each message
includes the review item UUID so it is visible to the user. The backend tracks which
review item is currently awaiting a response; the user's next reply is automatically
mapped to it — no explicit reference required from the user.

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

Three-region split: **chat (left) / right panel (center-right) / sidebar (far right).**

```
┌─────────────────────────┬──────────────────────────────────────┬───┐
│                         │                                      │ S │
│   Chat (35%)            │   Right panel (62%)                  │ E │
│   - main agent dialog   │   - system state display             │ K │
│   - all decisions       │   - mostly read-only                 │ T │
│   - questions/approvals │   - Epic/Task editable               │ L │
│                         │                                      │   │
└─────────────────────────┴──────────────────────────────────────┴───┘
```

**Left side = interaction.** All questions, approvals, decisions, and conversation happen
through the main agent in chat. The agent drives the review queue, surfaces items one at
a time, and asks the user directly.

**Right side = state of system.** Shows context for what's being discussed. The main agent
switches the panel programmatically to show relevant state. The user can also switch
manually via sidebar keys.

**Sidebar** — vertical strip (3 chars wide) on the far right. Each panel has a
single-letter box. Active panel is highlighted. User presses the letter key to switch:

| Key | Panel | Content |
|-----|-------|---------|
| **S** | Status | Overview: current epic, in-flight tasks, queue counts |
| **E** | Epic | Selected epic detail, task list, progress |
| **K** | Kanban | Board columns with tasks |
| **T** | Task | Selected task: description, diff, branch, stage history |
| **L** | Logs | Daemon activity, agent runs, errors |

**Epic and Task panels are editable** — when the right panel shows an epic or task, the
user can directly edit text fields (title, description) in place. All other panels are
read-only state displays.

### Opening status panel

When gitzi opens, the right panel renders a structured **status card** assembled directly
from harness state — not an AI-generated message:

- **Current epic** — title, progress (tasks done / total)
- **In progress** — tasks currently being worked on by agents
- **Waiting for you** — tasks in Waiting for Review (need approval/rejection)
- **Clarification queue** — count of pending review items awaiting your answer
- **Followups** — unresolved items carried forward from the previous session summary

The chat pane starts empty and ready for input. The panel is what the user reads first;
the chat is where they act on it.

### Diff review

Diff review happens in the **Task detail** panel. The right panel shows the diff; the
chat asks the user to approve/reject inline via conversation.

### Review items

A review item is a first-class artifact type, stored separately from tasks and epics.
It is raised the moment a clarification is needed and carries the full decision record
once resolved — there is no separate "ADR" concept; the review item is the decision
record.

Review items can also be created directly from chat when a significant design decision
is made outside of a task context.

**Storage:** `~/.gitzi/plan/reviews/<uuid>.toml`

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

**Linking:** Tasks and epics carry a `review_items = ["<uuid>", ...]` field.
The chat surfaces relevant review items when working on related tasks.

**A review item is created immediately when a clarification is raised — before the
user answers.** It starts in `pending` status with the question, context, and candidate
options filled in. When the user answers, it is updated to `resolved` with the decision
and rationale.

**Review item lifecycle — two resolution paths:**

```
Human-resolved (agent needs user input):
  background agent hits uncertainty
    → creates pending review item
    → enqueues in attention queue
    → agent parks and waits
  main agent becomes idle
    → harness pulls next item from attention queue
    → surfaces review item to user through chat
  user answers
    → review item resolved
    → background agent resumes

Agent self-resolved (agent decides autonomously):
  background agent makes a structural decision (dependency task, park, etc.)
    → creates review item with its own reasoning as the answer
    → marked agent-resolved
    → proceeds immediately
    → review item visible in status panel for user to review / override at their own pace
```

**Key principles:**
- Background agents never interrupt an active user conversation
- The main agent is the single point of contact between the user and all background work
- Items are surfaced **one at a time** — always (see Core principle above)

**Attention queue** — stored in harness state, contains:
- Review item UUID
- Which task raised it
- Priority / order raised
- Status: `waiting` | `surfaced` | `resolved`

**Review items are always injected into agent context.** When any agent picks up a task,
the harness fetches all review items linked to that task (and its parent epic) and
includes them in the system prompt:
- `resolved` review items tell the agent what has been decided — implement accordingly
- `pending` review items tell the agent what is still open — do not proceed on those areas

This ensures agents never re-ask a question that has already been answered, and never
act on an area where a decision is still pending.

**Every resolved review item also produces a test.** The agent generates a unit test that:
- Validates that the chosen solution is correctly implemented
- Carries the problem statement and chosen solution in its doc comment
- Links to the originating task/issue and the review item by UUID

This test is the living proof that the decision holds. If the implementation drifts, the
test fails and the review item UUID in the failure points directly back to why the
decision was made.

---

## Open Questions

- [x] **Orchestration layer** — custom Rust with `rig-core` as LLM/tool layer (AWS Strands rejected: Python-only)
- [x] **Multi-agent parallelism** — parallel supported, default WIP limit 1 (raise deliberately)
- [x] **Branch strategy** — one branch per task (`gitzi/<task-id>-<slug>`)
- [x] **Slack** — dropped; mobile app via federated git-native protocol (TBD)
- [x] **Test adapter** — not needed; `test_command` in `config.toml`
- [x] **Epic auto-generation** — LLM proposes epic breakdowns, task splits, and prioritization; human approves each step
- [ ] **Mobile protocol** — define the federated protocol for mobile ↔ git repo communication


---

## First-run bootstrap

When a user runs `gitzi` for the first time (no `~/.gitzi/` exists), the harness runs a
bootstrap sequence before launching the TUI. No wizard, no manual config editing — it
discovers what's available and generates `config.toml` automatically.

### LLM provider discovery

The bootstrapper scans for available LLM providers in priority order:

1. **CLI-based LLMs already running** (highest priority — zero setup)
2. **Installed but not running** (can be started automatically)
3. **Not installed** (skip, try next)

**Known providers to detect:**

| Provider | Detection | Start command |
|----------|-----------|---------------|
| LM Studio | `~/.lmstudio/bin/lms` exists | `lms server start` |
| Ollama | `which ollama` | `ollama serve` |
| Claude CLI | `which claude` | N/A (subprocess per-call) |
| OpenCode | `which opencode` | TBD |
| Goose | `which goose` | TBD |
| LocalAI | `which local-ai` | TBD |
| llama.cpp server | `which llama-server` | TBD |

**Decision logic:**
- If exactly one provider is found → use it, no prompt
- If multiple found in the same priority tier → ask the user which to use
- For "installed but not running" providers → attempt to start them
- Test connectivity: hit the provider's health endpoint to confirm it responds

### Q&A decisions (to be answered)

<!-- Record answers to bootstrap questions below as they're decided -->

1. **Q: Auto-discover repo_paths?**
   A: (pending)

2. **Q: ...**
   A: (pending)

---

## Bootstrapping

First-time experience when a user runs `gitzi` with no existing `~/.gitzi/`.

### First-run flow

```
loader/spinner (auto-discover LLM providers + repos)
    → generate config.toml
    → if multiple providers: onboarding selection UI
    → launch TUI (Status panel shows discovery results)
    → main agent: "So what are we going to do next?"
```

### Provider discovery priority order

1. **CLI-based LLMs already running** (highest — zero friction)
2. **Installed but not running** — show guidance, do NOT auto-start
3. **Not installed** — skip

| Provider | Binary | Detection | Start |
|----------|--------|-----------|-------|
| LM Studio | `~/.lmstudio/bin/lms` | `lms server status` | `lms server start` |
| Ollama | `ollama` | `ollama list` / port 11434 | `ollama serve` |
| Claude CLI | `claude` | `which claude` | N/A (API key) |
| OpenCode | `opencode` | `which opencode` | TBD |
| Goose | `goose` | `which goose` | TBD |
| Aider | `aider` | `which aider` | TBD |

For "installed but not running": display guidance on Status panel ("X is installed but
not running. Please start it and load a model."). For "running but no model loaded":
use `/v1/models` HTTP endpoint to check, then force-load first downloaded text model.

### Config three-layer architecture

```
Layer 1: Hardcoded (compiled into binary)
  → System prompts per role (not user-overridable)
  → Pipeline structure, tool definitions, safety invariants

Layer 2: Default values (written to config.toml on first creation)
  → Provider URLs, model names, WIP limits

Layer 3: User config.toml
  → What the user has explicitly set
```

Resolution: `Final = hardcoded ?? user_config ?? default`

- `system_prompt` is NOT in user config — hardcoded per role (L1)
- Roles not defined in `[[agents]]` inherit model/provider from `main`
- Invalid roles → silently stripped, config.toml rewritten
- Removed fields: `default_agent`, `test_command`, `system_prompt` in `[[agents]]`

### Selection logic

- Single provider found → auto-use, no question
- Multiple in same category → ask user which to use (onboarding selection flow)
- Prefer: already-running > needs-start, local > API-key-required

### Status panel states

| State | Rendering |
|-------|-----------|
| **First run (LLM available)** | Discovered providers + repos with heuristic summaries |
| **First run (NO LLM)** | Error + install instructions. Chat bar HIDDEN. |
| **Normal operation** | Current epic, in-progress tasks, queue counts |

### Agent behavior decisions

- **Epic creation**: conversational (10-20 questions), never one-shot
- **Task creation**: suggest titles, iterate, create only after user confirms each
- **Coding agent**: always runs tests+lint, fixes failures, never hands off broken code
- **Reviewer**: adversarial auditor — assumes coder did everything wrong, structured rejection
- **Tester role**: removed — merged into Reviewer
- **Reviewer rejection flow**: findings → human triage in buffer → fix or ignore. Fix → direct to Coding (not CodingBuffer), respects WIP limit
- **Security Auditor**: separate pass, same triage pattern
- **Stage skipping**: never. Every task traverses every stage. Agent at each stage decides if there's work.
- **Infrarian**: validates infrastructure (reliability, durability, cost, non-destructive migrations, enterprise patterns)
- **Designer output**: UI/UX design, architecture, critical considerations, "how to do the work" → stored in task's `## Design` section (markdown)
- **Prioritizer scope**: reorders existing tasks only. Epic splitting is exclusively main agent + user conversation.
- **Skills**: gitzi's own system — not just markdown, can be executables, configs, structured data. Matching logic TBD.
- **Worktree cleanup**: delete `~/.gitzi/tmp/tasks/<id>/` when task reaches Done (after merge)
