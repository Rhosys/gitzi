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

**Task file shape (sketch):**
```toml
id = "task-001"
epic = "epic-001"
title = "Add login endpoint"
stage = "in-progress"
agent = "claude-code"
wip_limit_blocked = false
created_at = "2026-06-01T00:00:00Z"
updated_at = "2026-06-01T00:00:00Z"

[history]
# append-only log of stage transitions
```

---

## Agent Runner (Pluggable)

Each backend implements a common `AgentBackend` trait:

```rust
trait AgentBackend {
    async fn run(&self, task: &Task, context: &RunContext) -> AgentResult;
}
```

| Backend        | Invocation                                      | Notes                            |
|----------------|-------------------------------------------------|----------------------------------|
| `claude-code`  | `claude --print --output-format stream-json`    | Primary; rich diff output        |
| `kiro`         | `kiro run <task-prompt>`                        | AWS IDE agent CLI                |
| `ollama`       | HTTP to local Ollama API                        | Local/offline tasks              |
| `chatlm-ui`    | HTTP API or CLI wrapper                         | ChatLM agent sessions            |

The backend for a task is set in the epic or task TOML, with a fallback to the
`config.toml` default.

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
| Slack          | Review notifications, approvals via reaction    | Push + inbound    |

---

## Open Decision: AWS Strands SDK

**Question:** Should AWS Strands SDK be the multi-agent coordination layer?

**Tradeoffs:**

| Factor          | AWS Strands                                          | Custom Rust orchestration              |
|-----------------|------------------------------------------------------|----------------------------------------|
| Language fit    | Python SDK — requires subprocess or FFI boundary     | Pure Rust, no boundary overhead        |
| Vendor lock-in  | AWS-centric; adds IAM, Bedrock dependencies          | Zero external deps for coordination    |
| Features        | Built-in memory, tool routing, agent handoff         | Must build these (simpler model)       |
| Maturity        | New (2025); API may shift                            | Stable once built                      |
| AWS services    | Great if using Bedrock, S3, Lambda as infrastructure | Not relevant unless using AWS infra    |
| Effort          | Integration work to cross the language boundary      | More Rust code to write upfront        |

**Recommendation:** Start with custom Rust orchestration. Strands adds value only if
the harness runs agents on AWS Bedrock or needs Strands' built-in memory/tool routing —
neither is required for the MVP. Strands can be added as a backend adapter later.

**Decision:** _pending — revisit once MVP agent runner is working_

---

## Tech Stack

| Layer              | Choice                          |
|--------------------|---------------------------------|
| Language           | Rust (2024 edition)             |
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

- [ ] **AWS Strands** — use as orchestration layer or custom Rust? (see section above)
- [ ] **Epic auto-generation** — should the harness propose task breakdowns using an LLM, or is that always human-driven?
- [ ] **Multi-agent parallelism** — can multiple tasks be In Progress simultaneously, or is it strictly one at a time to start?
- [ ] **Branch strategy** — one branch per task, or one branch per epic?
- [ ] **Approval via Slack** — emoji reaction (`:white_check_mark:`) or slash command?
- [ ] **Test adapter** — how does the harness know which test command to run for each project/task?
