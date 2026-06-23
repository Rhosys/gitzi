# Bootstrapping Spec

First-time experience when a user runs `gitzi` with no existing `~/.gitzi/`.

## Decided

### Q1: What does the user see first?
A loader/spinner while gitzi auto-discovers available LLM resources and generates config.

### Q2 (pending): Auto-discover repo_paths?
(Awaiting answer)

## Discovery priority order

1. **CLI-based LLMs already running** (highest priority — zero friction)
2. **Installed but not running** — gitzi starts them up
3. **Not installed** — skip, move to next

## LLM providers to detect

Scan for these binaries/processes (not exhaustive — expand over time):

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

## After discovery

- Generate `~/.gitzi/config.toml` with detected provider + sensible defaults
- Test connectivity (attempt a simple `/v1/models` or equivalent health check)
- If test fails → show error, suggest what to do
- If test passes → proceed to TUI

## Open Questions

- Q2: Should bootstrapper auto-discover repo_paths (scan ~/git/, ~/projects/, etc)?
- Q3–Q20: TBD
