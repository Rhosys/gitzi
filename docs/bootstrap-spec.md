# Bootstrapping Spec

First-time experience when a user runs `gitzi` with no existing `~/.gitzi/`.

## Decided

### Q1: What does the user see first?
A loader/spinner while gitzi auto-discovers available LLM resources and generates config.

### Q2: Auto-discover repo_paths?
Yes. Scan obvious locations (`~/git/`, `~/projects/`, `~/code/`, `~/src/`, `~/repos/`).
If none found → ask the user or tell them to cd into a project and rerun.
When repos are found, compute the common ancestor path and write that as a glob in
`repo_paths`. Only auto-populate if no existing repos are known.

### Q3: Post-discovery — summary or straight to TUI?
Straight into TUI. Discovery results shown on the Status panel (right side).

### Q5: First-time opening message?
Main agent says: "So what are we going to do next?"

Status panel has multiple forms:
- **First run (LLM available):** shows discovered providers + discovered repos with summaries
- **First run (NO LLM):** shows error — "No LLM provider found. Install LM Studio, Ollama, or another provider to continue." Chat bar is HIDDEN.
- **Normal run:** shows current epic, in-progress tasks, queue counts (existing behavior)

The opening message only fires when there IS an LLM available.

### Q5a: Repo summaries on Status panel?
Yes. The bootstrapper/daemon calls `repo_cache::populate()` which generates heuristic
summaries (from package.json, Cargo.toml, README). These summaries are shown on the
Status panel's first-run view so the user can see what repos were discovered and decide
what to work on.

### Q4: Multiple providers found?
List ALL discovered providers in the `[providers.*]` section of config.toml.
If multiple providers exist, show a special onboarding selection flow (not the chat)
asking which provider to use. The chosen provider is set in each `[[agents]]` entry.
Providers section goes at the bottom of config.toml.

## Discovery priority order

1. **CLI-based LLMs already running** (highest priority — zero friction)
2. **Installed but not running** — gitzi starts them up
3. **Not installed** — skip, move to next

## LLM providers to detect

| Provider | Binary | How to detect | How to start |
|----------|--------|---------------|--------------|
| LM Studio | `~/.lmstudio/bin/lms` | `lms server status` | `lms server start` |
| Ollama | `ollama` | `ollama list` / check port 11434 | `ollama serve` |
| Claude CLI | `claude` | `which claude` | N/A (API key needed) |
| OpenCode | `opencode` | `which opencode` | TBD |
| Goose | `goose` | `which goose` | TBD |
| Aider | `aider` | `which aider` | TBD |

## Selection logic

- If only 1 provider found in a category → use it automatically
- If multiple found in same category → ask user which to use
- Optimize: prefer already-running over needs-start, prefer local over API-key-required

## Config architecture (three-layer system)

```
Layer 1: Hardcoded (compiled into binary)
  → System prompts for each role (user cannot override)
  → Pipeline structure (columns, roles)
  → Tool definitions
  → Safety invariants

Layer 2: Default values (written to config.toml on first creation)
  → Provider URLs, model names, WIP limits
  → Reasonable starting values

Layer 3: User config.toml
  → What the user has explicitly set
```

**Resolution order:**
```
Final = hardcoded_value ?? user_config.field ?? default_value
```

Hardcoded always wins. User config overrides defaults. Defaults are the fallback.

### Agent resolution

- User config defines `[[agents]]` with: `role`, `model`, `provider`, `api_url`
- `system_prompt` is NOT user-configurable — it's hardcoded per role (L1)
- Roles not defined in `[[agents]]` inherit model/provider from the `main` role
- Invalid roles in config → silently stripped and config.toml rewritten

### Removed fields

- `default_agent` — dead, removed
- `test_command` — removed; the tester agent discovers how to run tests from the repo
- `system_prompt` in `[[agents]]` — removed from user-facing config; hardcoded per role

## Config.toml scaffold (generated on first run)

Clean, with section headers. No commented-out fields. No redundant explanations.
Provider field populated (not commented). Pretty section headers.

## Open Questions

- Q7–Q20: TBD

### Q6: Auto-start providers or ask the user?
- **Installed but NOT running** → show on Status panel: "X is installed but not running.
  Please start it and load a model." Do NOT auto-start.
- **Running but no model loaded** → gitzi starts the API server (if needed) and
  force-loads the first downloaded text model (e.g. `lms load <model> -y`).
  Use `/v1/models` HTTP endpoint to check what's loaded (more reliable than CLI).
- **Running with model** → ready to go.

### Q7–Q10: Pipeline agent behavior
- Epic creation: conversational (10-20 questions), never one-shot
- Task creation: suggest titles, iterate, only create after user confirms each
- Coding agent: always runs tests+lint, fixes failures, never hands off broken code
- Reviewer (adversarial auditor): assumes coder did everything wrong, structured rejection

### Q11–Q14: Pipeline structure
- Tester role removed — merged into Reviewer as adversarial auditor
- Reviewer findings → surfaced to human in buffer → human triages → fix or ignore
- If fix needed → task goes directly to Coding (not CodingBuffer)
- Security Auditor: separate pass, same pattern (findings → human triage)
- Never skip stages. Every task goes through every stage. Agent decides if there's work.

### Q15–Q16: Infrarian + stage skipping
- Infrarian validates infrastructure: reliability, durability, cost, non-destructive
  migrations, enterprise patterns. Follows infrastructure skills.
- Never skip stages. The agent at each stage decides what to do.

### Q17: Agent context injection
- Skills system needed (gitzi's own, not Kiro's)
- Skills are NOT just markdown — can be executables, configs, structured data
- Open design question: how to match skills to tasks (see Design section in todo.md)

### Q18: Designer agent output
- Produces: UI/UX design, architecture, critical considerations, "how to do the work"
- Output goes in the `## Design` section of the task TOML (markdown content)
- Coder reads the Design section to know HOW to implement

### Q19: Prioritizer scope
- Only reorders existing tasks
- Epic splitting is exclusively a main agent + user conversation

### Q20: Worktree cleanup
- Delete `~/.gitzi/tmp/tasks/<id>/` when task reaches Done (after merge)

## Status Panel Forms

The Status panel on the right side has different renderings based on system state:

### Form 1: First time / Just bootstrapped
Shows:
- Discovered LLM providers (name, status: running/installed/not-found)
- Discovered repos with their heuristic summaries (from cache)
- "So what are we going to do next?" in the chat

### Form 2: No LLM available
Shows:
- Error: "No LLM provider found"
- List of supported providers and how to install them
- **Chat bar is hidden** — cannot interact without an LLM

### Form 3: Normal operation (has epics/tasks)
Shows:
- Current epic + progress
- Tasks in progress
- Waiting for you (buffer approvals)
- Clarification queue count
