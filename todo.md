# gitzi — todo

## Principles

- **Minimal scope first.** Do the smallest thing that moves the task forward. Stop.
- **One question at a time.** Never ask two things at once. Wait for the answer.
- **Thoroughness over speed.** A slow correct step beats a fast wrong one.
- **Never assume.** If the next step is unclear, ask. Don't fill in gaps with guesses.

---

## User stories

### Core pipeline

- [ ] As a developer, I can run `gitzi init` in any repo to set up `.gitzi/`
- [ ] As a developer, I can create an epic with a title and description
- [ ] As a developer, I can create a task under an epic with a title, priority, and optional description
- [ ] As a developer, I can run `gitzi run` to start the scheduler and dashboard
- [ ] As a developer, the harness picks the highest-priority `prioritized` task automatically
- [ ] As a developer, the harness creates a git branch and worktree for each task without touching my workspace
- [ ] As a developer, the agent works inside the task's worktree and commits its output there
- [ ] As a developer, I see the task appear in the dashboard when it moves to `waiting-for-review`
- [ ] As a developer, I can view the diff for a task in the dashboard
- [ ] As a developer, I can approve a task from the dashboard, moving it to `in-testing`
- [ ] As a developer, I can reject a task with written feedback, sending it back to the agent
- [ ] As a developer, the harness runs the configured test command after I approve a task
- [ ] As a developer, the task moves to `done` automatically when tests pass
- [ ] As a developer, I can run `gitzi status` to see the current WIP snapshot in my terminal

### Plan management

- [ ] As a developer, I can commit `.gitzi/plan/` to git to checkpoint my epic and task state
- [ ] As a developer, `.gitzi/wip/` is automatically gitignored so runtime state never pollutes my commits
- [ ] As a developer, the harness asks me one clarifying question at a time before starting work on a task

### Agent behavior

- [ ] As a developer, the agent makes the smallest possible change that satisfies the task
- [ ] As a developer, the agent stops and surfaces a question rather than guessing when scope is unclear
- [ ] As a developer, the agent never refactors or extends beyond what the task explicitly asks for
- [ ] As a developer, rejected tasks are retried with the feedback included in the agent's next prompt

### Dashboard

- [ ] As a developer, I see a live kanban board that updates without a page reload
- [ ] As a developer, I see WIP counts per column
- [ ] As a developer, I can navigate to a task detail page showing its diff and history
- [ ] As a developer, I can approve or reject directly from the task detail page

---

## Context threading

When a user says something mid-interaction ("also I want to track x, y, z"), the harness
must not drop it and must not blindly fold it into the current work. Instead it classifies
the input and handles each part in the right context.

### Classification

- [ ] As a developer, when I interject during an active task, the harness classifies my
      message into: **(a)** relevant to the current thread — handle inline, or **(b)** a
      new independent thread — split and handle separately
- [ ] As a developer, the harness asks one clarifying question if the classification is
      ambiguous before deciding
- [ ] As a developer, inline additions (same thread) are folded into the current task's
      context without interrupting execution

### Thread splitting

- [ ] As a developer, when a new thread is identified the harness forks the current
      execution context: it saves the current thread state (task, stage, pending work),
      opens a new context for the new thread, and works through it completely before
      returning
- [ ] As a developer, the forked context is a full copy of the relevant state — it knows
      what was in flight when it was created so it can reason about dependencies
- [ ] As a developer, new-thread work that produces tasks/epics follows the normal
      pipeline (backlog → prioritized → ...) rather than bypassing it
- [ ] As a developer, the new thread's completion is a hard gate — the original thread
      does not resume until the new one reaches `done` or is explicitly deferred

### Resumption

- [ ] As a developer, after the new thread completes, the harness resumes the original
      thread from exactly the point it was paused — no repeated questions, no lost context
- [ ] As a developer, if the new thread produced changes that affect the original thread
      (e.g. a shared file was modified), the harness surfaces that conflict as a single
      question before resuming
- [ ] As a developer, thread history is stored in `.gitzi/wip/` so a restart does not
      lose a paused thread

### Implementation notes

Thread state is a stack entry in `.gitzi/wip/threads/<thread-id>/`:
```
  context.toml     frozen execution state (active task id, pending items, parent thread id)
  inbox.toml       raw user inputs received while the thread was paused
```
The scheduler maintains a thread stack; the active thread is always the top of the stack.
Completing a thread pops it and resumes its parent.

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

Nudge state is tracked in the session wip so restarts do not re-fire the same nudge:
```
~/.gitzi/<session>/wip/nudges.toml
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

- [ ] As a developer, the session maintains a rolling summary stored alongside
      `chat.jsonl` at `~/.gitzi/<session>/summary.md`
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
      `~/.gitzi/<session>/skills.toml` so a restart restores the same active set
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
- [ ] **`wip_limit_blocked` is set but never read** — `orchestrator.advance_task` marks the
      field but `pick_next_task` guards on `!t.wip_limit_blocked` while simultaneously
      doing its own WIP check. The field and the check are duplicated and the flag is never
      cleared after a slot opens. Remove the field or make it the single source of truth.
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

- [ ] **No parallelism within a tick** — the scheduler runs one agent at a time even though
      WIP limits explicitly allow more than one task in-progress. Each `tick()` picks at
      most one Prioritized task and awaits the agent sequentially. Use `tokio::spawn` or
      `FuturesUnordered` to run up to `wip_limits.in_progress` agents concurrently.
- [ ] **No timeouts anywhere** — agent execution, test runs, git operations, and HTTP
      handlers all have no deadline. A hung subprocess blocks the scheduler indefinitely.
      Add per-operation timeouts (`tokio::time::timeout`) configurable in `config.toml`.
- [ ] **No crash recovery** — if the process is killed mid-tick a task can be left in
      `in-progress` with no worktree, or a worktree with no task record. On startup the
      scheduler should detect and repair inconsistent state (task in-progress but worktree
      missing → move back to Prioritized; worktree exists but task not in-progress →
      re-register or clean up).
- [ ] **WipSnapshot can diverge from task files** — `rebuild_wip()` is the only
      reconciliation path and it is only called after writes, not on startup. A crash or
      concurrent write can leave the snapshot stale. Either remove the snapshot (derive it
      from task files every time) or add a startup validation pass.
- [ ] **No per-project config** — all repos share a single `~/.gitzi/config.toml`: same WIP
      limits, same test command, same agent backend. Projects differ; the config should
      support a project-level `config.toml` that overrides the global one, keyed off the
      repo root path or an explicit project ID stored in `.gitzi/project.toml` in the repo.
- [ ] **Session model is unclear** — the session UUID in `~/.gitzi/current` maps to a single
      conversation + plan directory. Questions: Does one session span one repo or many?
      Does starting gitzi in a new repo replace the session? Is the intention to run
      multiple sessions in parallel? Define and document the session lifecycle clearly.
- [ ] **Rejection re-queue lacks priority boost** — a rejected task goes back to
      `in-progress` (via `reject_task`) but the scheduler does not preferentially pick it
      up again. It competes equally with all other Prioritized tasks. Tasks with
      `agent_feedback` set should be boosted to the front of the queue.
- [ ] **No state caching** — every `reader::load_all_tasks()` call opens and parses every
      `.toml` file on disk. With O(100) tasks this is acceptable; with O(1000) it will
      noticeably lag the TUI. Consider an in-process cache invalidated by the file watcher
      (which is already implemented but never connected to anything).
- [ ] **File watcher is dead code** — `state::watcher` watches the state directory and
      classifies events into `StateEvent` variants, but nothing subscribes to it in the
      scheduler or TUI. Wire it: TUI should call `app.reload()` automatically on
      `TaskChanged`/`WipChanged` rather than requiring the user to press `r`.

---

### Design questions

- [ ] **What is the unit of a "session"?** Is it a conversation, a workday, a sprint?
      Should there be a `gitzi new-session` command to start a fresh context while keeping
      the plan directory? The current `new_session()` replaces the active pointer, which
      loses the previous session's chat history from the TUI.
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
- [ ] **Inline agent questions via chat** — when an agent encounters ambiguity, instead of
      guessing or failing, it should emit a structured question back through the harness.
      The scheduler pauses the task, the question appears in the TUI chat panel, and the
      human's reply is injected as context before the agent resumes. This is the "ask
      before write" skill made native to the pipeline.
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
- [ ] **Config hot-reload** — watch `config.toml` for changes and reload without restarting
      the scheduler. WIP limit changes should take effect on the next tick.
- [ ] **`gitzi status --json`** — machine-readable output from `cmd_status` for integration
      with shell scripts, CI steps, and external dashboards.
