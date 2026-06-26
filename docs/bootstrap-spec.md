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

## Discovery priority order (display only — never auto-selects)

Discovery never starts a server, loads a model, or logs into anything. Every provider
found is recorded `enabled = false`; the priority order below only affects display sort,
running+model-loaded providers are listed first:

1. **Running with a model loaded** (zero friction to activate)
2. **Running but no model loaded**
3. **Installed but not running**
4. **Not installed** — skipped, not recorded

## Providers detected today

| Provider | How detected | Kind |
|----------|--------------|------|
| LM Studio | `~/.lmstudio/bin/lms` exists; port 1234 + `/v1/models` for running/model-loaded | `openai-compatible` |
| Ollama | `which ollama`; port 11434 + `/v1/models` for running/model-loaded | `openai-compatible` |
| AWS Bedrock | `[sso-session NAME]` blocks in `~/.aws/config` (one candidate per session), else a generic candidate if the `aws` CLI is installed | `bedrock` |

Claude CLI/OpenCode/Goose/Aider are not auto-discovered providers in this scheme — they
remain available as the implicit fallback (the local `claude` CLI subprocess) for any
agent role that isn't wired to a provider.

## Activation logic

Nothing is enabled or wired into `[[agents]]` by discovery. The user activates a
provider explicitly, through the main agent's chat tools (`gitzi_rediscover_providers`,
`gitzi_activate_provider` — see `docs/bootstrap.md`):

- 1 or many providers found → all listed, none enabled; ask the user which (if any) to
  activate.
- Activating an OpenAI-compatible provider is immediate.
- Activating a Bedrock provider walks the user through AWS SSO login, then account and
  role selection, across multiple tool calls.
- A gitzi restart is required after activation for the rewired agent to take effect.

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
Never auto-start, never force-load a model, never auto-log-in. Discovery only records
status; the Status panel shows it as-is:
- **Installed but NOT running** → "X is installed but not running. Start it and load a
  model, then ask me to rediscover providers."
- **Running but no model loaded** → "X is running but has no model loaded. Load one,
  then ask me to rediscover providers." (`/v1/models` is used to check what's loaded —
  more reliable than the CLI.)
- **Running with model** → ready to activate via `gitzi_activate_provider`.
- **Bedrock** → never auto-logs in; SSO login only happens when the user asks to
  activate the provider.

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
