# gitzi — todo

## Principles

- **Minimal scope first.** Do the smallest thing that moves the task forward. Stop.
- **One question at a time.** Never ask two things at once. Wait for the answer.
- **Thoroughness over speed.** A slow correct step beats a fast wrong one.
- **Never assume.** If the next step is unclear, ask. Don't fill in gaps with guesses.
- **Do it right the first time.** Never any hacks. The user will be the only one to suggest and approve hacks.

---

## User stories

### Core pipeline

- [x] As a developer, I can create an epic with a title and description
- [x] As a developer, I can create a task under an epic with a title, priority, and optional description
- [x] As a developer, running `gitzi` auto-starts the daemon and launches the TUI
- [x] As a developer, the harness picks the highest-priority `prioritized` task automatically
- [x] As a developer, the harness creates a git branch and worktree for each task without touching my workspace
- [x] As a developer, the agent works inside the task's worktree and commits its output there
- [ ] As a developer, I see the task appear in the dashboard when it moves to `waiting-for-review`
- [ ] As a developer, I can view the diff for a task in the dashboard
- [ ] As a developer, I can approve a task from the dashboard, moving it to `in-testing`
- [ ] As a developer, I can reject a task with written feedback, sending it back to the agent
- [ ] As a developer, the harness runs the configured test command after I approve a task
- [ ] As a developer, the task moves to `done` automatically when tests pass
- [x] As a developer, I can run `gitzi status` to see the current WIP snapshot in my terminal

### Plan management

- [x] As a developer, runtime state lives in `~/.gitzi/` (flat layout, no sessions, no UUIDs)
- [ ] As a developer, the harness asks me one clarifying question at a time before starting work on a task
- [ ] As a developer, before a task moves from `prioritized` to `in-progress`, the harness
      runs an LLM pass over its description to identify vagueness, ambiguity, undefined
      scope boundaries, missing acceptance criteria, and implicit assumptions. Each gap is
      surfaced as a clarifying question the human must answer before work begins.

### Agent behavior

- [ ] As a developer, the agent is instructed (via system prompt / harness injection) that it is critically important to always write ongoing work, plans, ideas, and discussions to long-lived artifacts outside of the chat — context windows expire; files don't — every decision, design thought, or open question must land in a TODO, spec, ADR, or note, never exist only in conversation
- [ ] As a developer, when the agent starts a task the harness auto-discovers and injects
      repo-level convention/steering docs into the agent's context. Known file locations:
      `AGENTS.md` (root + subdirs), `CLAUDE.md`, `GEMINI.md`, `.kiro/steering/*.md`,
      `.cursor/rules/*.mdc`, `.cursorrules`, `.windsurfrules`, `.windsurf/rules/*.md`,
      `.github/copilot-instructions.md`, `CONVENTIONS.md`, `.junie/guidelines.md`,
      `devin.md`, `CONTRIBUTING.md`, `docs/ADR/*.md`, `ARCHITECTURE.md`, `DESIGN.md`.
      Resolve: (1) configurable discovery patterns per-repo in `[[repos]]` vs global
      default list, (2) dedup when multiple files say the same thing in different tool
      formats, (3) context budget — summarize or truncate when total exceeds threshold,
      (4) scope filtering — subdirectory-scoped docs injected only when task touches
      those paths vs always injecting root-level docs.
- [x] As a developer, the agent makes the smallest possible change that satisfies the task
- [x] As a developer, the agent stops and surfaces a question rather than guessing when scope is unclear
- [x] As a developer, the agent never refactors or extends beyond what the task explicitly asks for
- [x] As a developer, rejected tasks are retried with the feedback included in the agent's next prompt

### Dashboard

- [ ] As a developer, I see a live kanban board that updates without a page reload
- [ ] As a developer, I see WIP counts per column
- [ ] As a developer, I can navigate to a task detail page showing its diff and history
- [ ] As a developer, I can approve or reject directly from the task detail page

### TUI — Fork stack display

- [ ] As a developer, when a new fork is pushed onto the conversation stack, the TUI
      switches to a separate chat panel for that fork's messages
- [ ] As a developer, the left side of the TUI shows the stack of active forks (names)
      so I always know where I am in the conversation hierarchy
- [ ] As a developer, closing a fork pops the panel back to the parent conversation
- [ ] As a developer, the fork stack indicator shows depth and topic names
      (e.g. "main > provider-setup > model-choice")

### TUI — Streaming progress bar + tool display

- [ ] As a developer, while the LLM is generating a response, I see a progress bar that
      fills based on streamed token count (assume ~100 tokens for a full response)
- [ ] As a developer, the progress bar is linear from 0-80%, then exponential decay
      by 0.5 (80 → 90 → 95 → 97.5...) for the remaining range
- [ ] As a developer, when the model calls tools during the response, I see
      `Tools: [tool_name_1, tool_name_2]` at the top of the assistant message
- [ ] As a developer, the tool list persists in the rendered message after completion
- [ ] As a developer, switch MainAgent to `stream: true` and consume SSE token events
      for progress tracking (not for word-by-word display)

### TUI — Input field + thinking indicator

- [ ] Fix: pasting into the input field is slow (likely per-char redraw — batch paste events)
- [ ] Fix: remove `...` from input field — the input area is exclusively for user text
- [ ] After user submits (Enter), clear the input field immediately and show a spinner
      in the chat messages area as a pending assistant bubble (e.g. `⠋ thinking...`)
- [ ] The spinner animates until the response arrives, then is replaced by the actual text

---

## Context threading

When a user says something mid-interaction ("also I want to track x, y, z"), the harness
must not drop it and must not blindly fold it into the current work. Instead it classifies
the input and handles each part in the right context.

### Classification

- [x] As a developer, when I interject during an active task, the harness classifies my
      message into: **(a)** relevant to the current thread — handle inline, or **(b)** a
      new independent thread — split and handle separately
- [x] As a developer, the harness asks one clarifying question if the classification is
      ambiguous before deciding
- [x] As a developer, inline additions (same thread) are folded into the current task's
      context without interrupting execution

### Thread splitting

- [x] As a developer, when a new thread is identified the harness forks the current
      execution context: it saves the current thread state (task, stage, pending work),
      opens a new context for the new thread, and works through it completely before
      returning
- [x] As a developer, the forked context is a full copy of the relevant state — it knows
      what was in flight when it was created so it can reason about dependencies
- [x] As a developer, new-thread work that produces tasks/epics follows the normal
      pipeline (backlog → prioritized → ...) rather than bypassing it
- [x] As a developer, the new thread's completion is a hard gate — the original thread
      does not resume until the new one reaches `done` or is explicitly deferred

### Resumption

- [x] As a developer, after the new thread completes, the harness resumes the original
      thread from exactly the point it was paused — no repeated questions, no lost context
- [ ] As a developer, if the new thread produced changes that affect the original thread
      (e.g. a shared file was modified), the harness surfaces that conflict as a single
      question before resuming

### Implementation notes

Thread state is managed by the `chat_stack` field on the Dispatcher and persisted
per-fork in `~/.gitzi/chats/<fork-id>.jsonl`.
The active fork is always the top of the stack.
Completing a fork pops it and resumes its parent.

- [ ] LLM proposes task breakdown for a new epic; I approve before tasks are created
- [ ] Multiple tasks can be `in-progress` simultaneously (WIP limit raised deliberately)
- [ ] Kiro CLI backend
- [ ] Ollama backend via rig-core
- [ ] GitHub Issues sync (push task state to issues)
- [ ] Jira sync (push only)
- [ ] Linear sync (push only)
- [ ] Mobile app integration via federated git-native protocol (TBD)

---

## Scheduler nudges

Tasks left idle in a stage that requires human action should surface a reminder rather
than silently stall. The scheduler already ticks; nudges ride on top of it.

- [ ] As a developer, if a task stays in `waiting-for-review` longer than a configured
      threshold (default 24 h), the scheduler emits a nudge message into the chat log
- [ ] As a developer, if a task stays in `in-progress` without a commit for longer than
      a configured threshold, the scheduler emits a nudge asking whether to continue,
      pause, or reassign
- [ ] As a developer, I can configure per-stage idle thresholds in `config.toml`
- [ ] As a developer, nudges appear in the chat panel as a `system` role message so they
      are part of the permanent record and not ephemeral notifications
- [ ] As a developer, a nudge is not repeated until I acknowledge it or the task moves

### Implementation notes

Nudge state would be tracked in `~/.gitzi/tmp/nudges.toml`:
```
[[nudge]]
task_id = "..."
stage   = "waiting-for-review"
fired_at = "..."
acked   = false
```

---

## Conversation summarization

The chat history grows unboundedly; the right panel and any LLM context must always
reflect a coherent, up-to-date summary rather than raw transcript replay.

- [ ] As a developer, the session maintains a rolling summary stored at
      `~/.gitzi/chats/summary.md`
- [ ] As a developer, after every N messages (configurable, default 10) the summarizer
      runs and updates `summary.md` with what is known: active epics, tasks in flight,
      decisions made, open questions
- [ ] As a developer, the summary is injected as the first message in any new LLM
      context window so the agent never loses prior decisions
- [ ] As a developer, the right panel in the TUI shows the current summary when no
      specific task or epic is selected
- [ ] As a developer, I can trigger a manual re-summarize with `/summarize` in the
      chat input

### Implementation notes

The summarizer is itself an LLM call using the configured agent backend, given the
last N messages plus the previous summary as input. Output replaces `summary.md`.
The summary is append-logged to `chat.jsonl` as a `system` role message with a
`"summarized": true` marker so it can be distinguished from organic system messages.

---

## Skills

Skills are named, toggleable behaviors that alter how the agent or harness handles
messages. Some apply to the whole conversation; others are scoped to a single message.

- [ ] As a developer, I can see the list of active skills for the current session in
      the TUI chat panel (e.g. as a chip row above the input box)
- [ ] As a developer, I can type `/skills` to open a skill picker and toggle skills on
      or off for the rest of the session
- [ ] As a developer, I can prefix a single message with `@skill-name` to activate a
      skill for that message only without changing the session default
- [ ] As a developer, skill state is persisted in
      `~/.gitzi/config.toml` so a restart restores the same active set
- [ ] As a developer, I can define custom skills in `~/.gitzi/config.toml` as named
      system-prompt fragments that are injected when the skill is active

### Built-in skills (initial set)

| Skill | Effect |
|---|---|
| `strict-scope` | Agent refuses any change outside the task description |
| `ask-before-write` | Agent must confirm every file write before executing |
| `tdd` | Agent writes a failing test before any implementation |
| `explain` | Agent narrates each step in the chat before doing it |
| `no-commit` | Agent produces a diff but does not commit |

### Implementation notes

Skill activation for a session is a `Vec<String>` in session state. Per-message
activation is a parsed prefix: `@tdd fix the login timeout` strips `@tdd`, adds it
to the active set for that prompt only, then restores the prior set after the response.
Skills are injected into the agent's preamble as concatenated system-prompt fragments
in activation order.

---

## Audit: Recommendations, Questions & Concerns

Full codebase audit against the intended agile SDLC harness. Organized by severity and type.

---

### Bugs (code is wrong today)

- [ ] **Inverted priority logic** — `pick_next_task` uses `min_by_key(|t| t.priority)` and
      the field defaults to `100`. If lower number = higher priority (P1 beats P100), the
      sort is correct but the convention is undocumented and the `task create --priority`
      flag has no guidance. Decide the convention, document it, enforce it.
- [ ] **Empty commits allowed** — `git::ops::commit_all` stages and commits unconditionally;
      if the agent produced no file changes (e.g. only logged output) an empty commit is
      created, corrupting the task's git history. Guard with an index-is-clean check before
      committing.
- [ ] **Scheduler test path passes empty branch name** — the test runner loop resolves the
      worktree via `task.branch.as_deref().and_then(|_| ...)` but ignores the branch value
      and calls the path helper with `""`. Tests are run in the wrong directory or fail
      silently.
- [ ] **HTTP errors return 200** — `approve_task` and `reject_task` return `200 OK` with an
      error message in the body on failure. Dashboard JS reloads on any SSE message so the
      user never sees the error. Return appropriate 4xx/5xx status codes.
- [ ] **`Task.agent` field is never written** — the field exists for traceability (which
      backend ran the task) but the scheduler never populates it. Either populate it in
      `scheduler.tick()` after `build_agent()` or remove the field.

---

### Architecture concerns

- [x] **No parallelism within a tick** — RESOLVED: agent pool spawns concurrent tasks up to
      WIP limits using `tokio::spawn`.
- [ ] **No timeouts anywhere** — agent execution, test runs, git operations, and HTTP
      handlers all have no deadline. A hung subprocess blocks the scheduler indefinitely.
      Add per-operation timeouts (`tokio::time::timeout`) configurable in `config.toml`.
- [ ] **No crash recovery** — if the process is killed mid-tick a task can be left in
      `in-progress` with no worktree, or a worktree with no task record. On startup the
      scheduler should detect and repair inconsistent state (task in-progress but worktree
      missing → move back to Prioritized; worktree exists but task not in-progress →
      re-register or clean up).
- [x] **WipSnapshot can diverge from task files** — RESOLVED: replaced by in-memory
      `KanbanBoard` projection built from task files on boot. No separate snapshot file.
- [x] **No per-project config** — RESOLVED: `[[repos]]` config with per-repo overrides
      (merge strategy, test command, main branch) keyed by repo slug.
- [x] **Session model is unclear** — RESOLVED: sessions removed. Flat layout at
      `~/.gitzi/` with `chats/current.jsonl` for main history and per-fork files.
- [x] **Rejection re-queue with priority boost** — rejected tasks get `priority = 0` (front
      of queue) and `agent_feedback` is included in the next agent prompt.
- [x] **No state caching** — RESOLVED: `KanbanBoard` is the in-memory cache. Tasks are
      loaded once on boot and mutated in-memory; disk writes happen on state changes.
- [ ] **File watcher is dead code** — `state::watcher` watches the state directory and
      classifies events into `StateEvent` variants, but nothing subscribes to it in the
      scheduler or TUI. Wire it: TUI should call `app.reload()` automatically on
      `TaskChanged`/`WipChanged` rather than requiring the user to press `r`.

---

### Design questions

- [x] **What is the unit of a "session"?** — RESOLVED: no sessions. Single daemon process,
      single flat `~/.gitzi/` layout. Chat history is one continuous file plus per-fork
      files. No `gitzi new-session` concept needed.
- [ ] **How does a task relate to a branch after completion?** Once a task reaches `done`,
      the worktree is left on disk. Should it be cleaned up automatically? Should the
      branch be merged, deleted, or archived? The workflow is silent on post-done
      housekeeping.
- [ ] **Who triggers the move from Prioritized → InProgress?** Currently the scheduler
      auto-picks the top task. But the plan also describes human approval before agent
      work starts ("LLM proposes task breakdown; I approve before tasks are created").
      Is there a confirmation gate before the scheduler picks up a task, or is Prioritized
      an implicit approval?
- [ ] **What is the intended diff review flow?** The dashboard shows a diff, but the diff
      is between the branch HEAD and merge-base, not against main. If the agent commits
      incrementally, the diff grows. Should the review show only the latest commit diff,
      the full branch diff, or a structured summary of changed files?
- [ ] **Where does LLM-proposed epic breakdown live?** The todo.md item says "LLM proposes
      task breakdown for a new epic; I approve before tasks are created." This implies an
      intermediate state between epic creation and tasks existing. Where is this proposal
      stored? As draft tasks in a new stage? As a chat message? Define the data model.
- [ ] **Multi-agent parallelism boundary** — when two agents run concurrently on different
      tasks in the same repo, both create worktrees from the same HEAD. If both modify the
      same file, the second merge will conflict. What is the conflict resolution strategy?
      Does gitzi detect potential conflicts before assigning work?
- [ ] **Priority convention** — is `priority = 1` the highest or lowest priority? The
      `task create` help text doesn't say, the model defaults to `100`, and the sort
      direction is ascending. Pick a convention (P1 = highest is standard), rename the
      field to something explicit (e.g. `priority_rank`), and enforce it consistently.

---

### Missing infrastructure for a production harness

- [ ] **Agent timeout and watchdog** — the agent subprocess must have a hard wall-clock
      limit (`--max-turns` for Claude Code CLI; a `tokio::time::timeout` wrapper for Rig).
      When the timeout fires, kill the subprocess, log the partial output, and move the
      task back to Prioritized with a "timed out" note.
- [ ] **Structured agent output** — `AgentResult.output` is an unstructured string. The
      harness cannot extract file change lists, questions asked, or confidence signals from
      it. Define a lightweight schema (JSON envelope) that agent backends should emit and
      parse it in the scheduler for richer routing decisions.
- [ ] **Pre-flight checks before InProgress** — before creating the worktree, verify: the
      repo index is clean, the branch doesn't already exist (or is the expected one), disk
      space is adequate. A failed pre-flight should surface a warning, not a silent error.
- [ ] **Audit log** — approvals, rejections, stage transitions, and agent runs should be
      written to an append-only log (similar to `chat.jsonl`) that is separate from the
      task history embedded in each TOML file. This gives a project-level audit trail.
- [ ] **Diff size guard** — `git::ops::get_diff` produces unbounded output. Large diffs
      (generated files, data files, binary files) can exhaust memory. Add a byte limit and
      truncate with a note when exceeded. Exclude binary files from the patch.
- [ ] **Dashboard CSRF protection** — approve/reject are POST forms with no CSRF token. Any
      page on the same host can submit them. Add a session cookie + token even if auth is
      not implemented.
- [ ] **SSE keepalive** — the SSE stream has no heartbeat. Proxies and load balancers drop
      idle connections after 30–60 s. Send a comment (`: ping`) every 15 s to keep the
      connection alive, and add client-side reconnect logic.
- [ ] **Tracing export** — `tracing_subscriber` is initialized but no exporter is
      configured. Add optional OTLP export (behind a feature flag or env var) so the
      scheduler's tick timing and agent durations are observable in production.
- [ ] **Secure storage for `ProviderDef::api_key`** — today `[providers.*].api_key` is
      stored in plaintext in `~/.gitzi/config.toml` (see `src/config.rs`). On load,
      check whether the value is already in gitzi's secure-reference format (e.g.
      `keyring:<service>/<account>`); if not, move the raw key into the OS keyring /
      secure enclave (`keyring` crate or platform equivalent) and rewrite the field in
      config.toml to the secure-reference pointer instead of the plaintext key. Define
      the exact pointer format and pick the keyring backend per OS before implementing.

---

### Ideas & improvements

- [ ] **Priority bands instead of raw numbers** — replace `priority: u32` with a `Priority`
      enum (`Critical / High / Medium / Low / Backlog`) that serializes to a fixed integer.
      Easier for humans to reason about and eliminates the "is 1 high or low?" ambiguity.
- [ ] **Task templates** — allow epics to define a template for their tasks (default
      description skeleton, default agent backend, default test command override). Reduces
      repetition when creating many similar tasks.
- [ ] **`gitzi doctor` command** — validates state consistency: every task file referenced
      in the WIP snapshot exists; every task's epic exists; every worktree registered in
      git corresponds to an in-progress task; no orphaned branches. Runs checks and prints
      a report without modifying state.
- [ ] **`gitzi clean` command** — removes orphaned worktrees, stale WIP directories for
      completed tasks, and branches for `done` tasks (optionally merges them first).
- [x] **Inline agent questions via chat** — when an agent encounters ambiguity, it emits a
      structured question back through the harness via `gitzi_create_review_item`. The
      review queue surfaces it in the main chat, and the human's reply unblocks the agent.
- [ ] **Task dependency graph** — add an optional `depends_on: Vec<String>` field to Task.
      The scheduler respects dependencies: a task cannot be picked up until all its
      dependencies are `done`. Visualize dependencies as a DAG in the TUI right panel.
- [ ] **Diff-aware retries** — on rejection, pass not just the feedback text but a focused
      diff of what the agent changed vs. what the reviewer expected. Giving the agent a
      structured delta ("you changed X, the reviewer expected Y") is more useful than
      free-text feedback alone.
- [ ] **Per-stage time tracking** — record `entered_at` on each `StageTransition` and
      compute cycle time (time from InProgress to Done) and lead time (Prioritized to
      Done). Surface these in the TUI board column headers and the dashboard.
- [x] **Config hot-reload** — `watch_config()` watches `config.toml` for changes and
      reloads WIP limits without restarting the scheduler.
- [ ] **`gitzi status --json`** — machine-readable output from `cmd_status` for integration
      with shell scripts, CI steps, and external dashboards.
- [ ] **UI screenshotter** — need a way to capture screenshots of the dashboard/TUI for
      verifying UI changes (e.g. before/after a layout fix) without manual review. Decide
      whether this is a dev-only tool, a CI check, or both, and which capture mechanism
      (headless browser for the dashboard, terminal capture for the TUI) fits each.

---

## Design: Skills & Dynamic Tools

### Skills system (`gitzi skill install/uninstall SKILL`)

- Skills are installable convention/instruction docs that get injected into agent contexts
- Stored at `~/.gitzi/skills/`
- Public registry of community-contributed skills (mechanism TBD)
- Each skill has metadata: keywords, file patterns, languages it applies to
- At dispatch time, gitzi matches task/repo characteristics to skill metadata and injects relevant ones
- Skills are NOT tied to Kiro, not tied to any IDE — gitzi's own skill system
- `gitzi skill install <name>` — downloads from registry to `~/.gitzi/skills/`
- `gitzi skill uninstall <name>` — removes it
- Skills are injected as context (paths listed in system prompt, agent reads via `read_file` if needed)

### Dynamic tools (WRONG DIRECTION — DO NOT IMPLEMENT AS DESCRIBED)

Previous idea: "when gitzi observes repeated patterns in agent work, crystallize that
into a first-class tool." This is wrong because:
- It conflates observation with tool creation (who validates the tool is correct?)
- It assumes patterns are stable (they may be one-off)
- It creates magic that the user didn't ask for
- Tools should be explicitly authored, not auto-generated from behavior

The CORRECT direction for expanding gitzi's toolset:
- When an agent calls a tool that doesn't exist → create a review item asking the user
  whether to build it (already implemented)
- User-authored tools live in `~/.gitzi/tools/` as executable scripts
- gitzi exposes them to agents automatically (tool name = filename, description from
  a header comment in the script)
- No auto-generation. User decides. User builds. Or user tells gitzi to build it as a task.

### Agent context injection (how agents find conventions)

Open question: how does gitzi decide which skills/docs to inject into an agent's context
at dispatch time?

Approaches considered:
1. Hardcoded mapping (label → skill file) — too narrow, assumes MY skills exist for everyone
2. Metadata matching (skill declares keywords, repo has labels) — more general
3. Agent discovers on its own via `list_steering` tool — slower but most flexible
4. Combination: inject a short manifest of available skills (names + descriptions),
   let the agent `read_file` the ones it wants

Decision: TBD — needs further design.

---

## Bootstrapping

- [x] Implement first-run bootstrapper flow (`src/bootstrap.rs`) — quick, non-blocking
      scan; no loader/spinner needed since it never starts servers, loads models, or
      blocks on SSO login before the TUI launches
- [x] Status panel: first-run view shows providers + repos with summaries
- [x] Activation flow via main agent chat tools (`gitzi_rediscover_providers`,
      `gitzi_activate_provider`) instead of a separate blocking onboarding selection UI
- [ ] Detect installed-but-not-running providers and surface per-provider guidance text
      directly on the Status panel (today this status is only relayed through
      `gitzi_rediscover_providers`'s chat response)
- [ ] Add `design` field to Task model (markdown content from Designer agent)
- [ ] Worktree cleanup: delete `~/.gitzi/tmp/tasks/<id>/` when task reaches Done
- [ ] Reviewer rejection → direct to Coding (not CodingBuffer), respects WIP limit
- [ ] `gitzi skill install/uninstall SKILL` CLI commands
- [ ] User-authored tools in `~/.gitzi/tools/` (exposed to agents as tool name = filename)

---

## Multi-cloud credential providers (Azure, GCP)

AWS Bedrock support (`src/aws_sso.rs`) is the first cloud provider; Azure (Azure OpenAI /
Azure AI Foundry) and GCP (Vertex AI) should follow the same shape — drive an interactive
login, cache whatever the SDK returns in the OS keyring under `gitzi/<domain>/...`
(`secrets::service_name`), never cache the short-lived bearer/role token.

- [ ] Azure OpenAI / Azure AI Foundry provider + login flow
- [ ] GCP Vertex AI provider + login flow

### Implementation notes — candidate crates

| Cloud | Drives a *new* interactive login (≈ `aws_sso.rs`) | Reads an *existing* CLI login (≈ bootstrap.rs's AWS-cache detection) |
|---|---|---|
| Azure | [`azure-identity-helpers`](https://github.com/demoray/azure-identity-helpers) (unofficial, v0.2.0) — `DeviceCodeCredential` implements the device-code flow on top of official [`azure_identity`](https://github.com/azure/azure-sdk-for-rust) (v1.0.0, Microsoft-official) | `azure-identity-helpers`'s `AzureAuthCliCredential` / `default_azure_credential`, or `azure_identity`'s `AzureCliCredential` |
| GCP | [`yup-oauth2`](https://github.com/dermesser/yup-oauth2) (v12.1.2, mature) — implements the actual OAuth2 device/installed-app flows | [`gcp_auth`](https://github.com/djc/gcp_auth) (v0.12.7) — reads `GOOGLE_APPLICATION_CREDENTIALS`, `gcloud auth application-default login`'s cached file, the metadata server, or shells out to `gcloud`; **no Windows support**, drives no login of its own |

Architectural differences from AWS to account for when designing these (confirmed from
crate source, not just docs):

- **No dynamic client self-registration.** AWS SSO's `register_client` (cached at
  `gitzi/aws/sso-client`) has no Azure/GCP equivalent — both require a pre-registered
  app/client ID configured ahead of time. Nothing analogous to cache there.
- **No mandatory role-exchange step.** AWS needs SSO login *then* a separate
  `get_role_credentials` (STS-style) call to scope to an account/role. Azure and GCP skip
  that — the OAuth access token from login is usable directly as the API bearer token;
  scoping happens server-side via Azure RBAC role assignments or (optionally) GCP
  service-account impersonation, not a client-side exchange call.
- Both crates keep tokens **in-memory only** — gitzi would own persisting whatever they
  return into the keyring (`gitzi/azure/oauth-token` keyed by tenant ID, `gitzi/gcp/oauth-token`
  keyed by account email), same shape as `aws_sso.rs`'s `store_token`/`load_token`.
