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

## Backlog (post-MVP)

- [ ] LLM proposes task breakdown for a new epic; I approve before tasks are created
- [ ] Multiple tasks can be `in-progress` simultaneously (WIP limit raised deliberately)
- [ ] Kiro CLI backend
- [ ] Ollama backend via rig-core
- [ ] GitHub Issues sync (push task state to issues)
- [ ] Jira sync (push only)
- [ ] Linear sync (push only)
- [ ] Mobile app integration via federated git-native protocol (TBD)
