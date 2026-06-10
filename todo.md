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
