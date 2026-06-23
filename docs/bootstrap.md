# Bootstrap — First-Time Experience

## Overview

When a user runs `gitzi` for the first time (no `~/.gitzi/` exists), the bootstrapper
runs before the TUI launches. It auto-discovers available LLM infrastructure and generates
a working `config.toml` with zero or minimal user input.

## Phase 1: LLM Provider Discovery

The bootstrapper shows a loader while scanning for available LLM providers. Priority order:

### Category 1: CLI-based LLM agents (already running or hot-startable)

Check in order:
1. **LM Studio** — `~/.lmstudio/bin/lms` exists → check `lms server status`
2. **Ollama** — `which ollama` → check `ollama list` (running) or start it
3. **Claude CLI** — `which claude` → verify auth
4. **OpenCode** — `which opencode`
5. **Goose** — `which goose`
6. **Aider** — `which aider`
7. (others TBD — maintain a discoverable list)

### Category 2: Installed but not running

If a provider binary exists but its server isn't running:
- Attempt to start it automatically
- Wait for health check to pass
- If it fails, move to next candidate

### Selection Logic

- **One provider found** → use it, no question asked
- **Multiple in same category** → ask user which to use (numbered list, pick one)
- **None found** → error with installation instructions for LM Studio (simplest path)

## Phase 2: Repo Path Discovery

(Question pending — TBD)

## Decisions Captured

| Question | Answer |
|----------|--------|
| First thing user sees? | Loader while auto-discovering resources |
| Discovery priority? | CLI-based running > installed not running |
| Multiple found? | Ask user to pick one |
| Single found? | Auto-select, test it works |
| None found? | Error with install instructions |
