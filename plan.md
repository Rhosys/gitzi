# gitzi — Agent Harness Plan

## Overview

**gitzi** is a Rust-based Kanban pipeline harness that orchestrates AI coding agents
across a software development lifecycle. It tracks epics and tasks, assigns work to
agents, enforces WIP limits, and optimizes every interaction with the human reviewer
by surfacing small, reviewable diffs and driving test-first development.

---

## Goals

1. **Pipeline orchestration** — move work through Kanban stages automatically by
   dispatching agents and reacting to outcomes.
2. **Epic & task management** — first-class breakdown of epics into tasks; splitting,
   prioritizing, and allocating work without requiring an external tracker.
3. **WIP tracking** — enforce limits at each stage; surface bottlenecks; prevent
   runaway parallel work.
4. **User-optimized review loop** — every agent output is a small, testable diff that
   the human can approve or reject before the pipeline continues.
5. **Pluggable agent backends** — Claude Code CLI, AWS Kiro CLI, Ollama, ChatLM UI,
   and others can be swapped in per task without changing the harness.
6. **Optional external sync** — GitHub Issues, Jira, and Linear are sync targets, not
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

All pipeline state lives as TOML files committed to the repo. This makes the state
auditable, diffable, and branchable.

```
.gitzi/
  config.toml          # harness config (WIP limits, integrations, agent defaults)
  epics/
    <epic-id>.toml     # epic metadata + child task list
  tasks/
    <task-id>.toml     # task metadata, stage, assigned agent, history
  wip.toml             # current stage snapshot (auto-generated, not hand-edited)
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

## Open Questions

- [x] **Orchestration layer** — custom Rust with `rig-core` as LLM/tool layer (AWS Strands rejected: Python-only)
- [x] **Multi-agent parallelism** — parallel supported, default WIP limit 1 (raise deliberately)
- [x] **Branch strategy** — one branch per task (`gitzi/<task-id>-<slug>`)
- [x] **Slack** — dropped; mobile app via federated git-native protocol (TBD)
- [x] **Test adapter** — not needed; `test_command` in `config.toml`
- [ ] **Epic auto-generation** — should the harness propose task breakdowns using an LLM, or is that always human-driven?
- [ ] **Mobile protocol** — define the federated protocol for mobile ↔ git repo communication
