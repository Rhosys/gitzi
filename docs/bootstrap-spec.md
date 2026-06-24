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

- Q5–Q20: TBD
