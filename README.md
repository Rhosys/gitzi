# gitzi

A Kanban agent harness for software development pipelines. Orchestrates AI coding agents across a full development lifecycle — prioritization, design, coding, review, security audit, deployment — with human-in-the-loop approval at every stage transition.

## How it works

gitzi runs as a background daemon (systemd/launchd) that manages a Kanban board of tasks. Each column maps to a specialized LLM agent role:

```
Prioritized → Designing → [buffer] → Coding → [buffer] → Reviewing → [buffer] → Auditing → [buffer] → Deploying → Done
```

Buffer columns pause the pipeline and surface a review item to the human. You approve, reject with feedback, or answer agent questions through the TUI chat interface.

The **main agent** drives the conversation — you create epics, break them into tasks, and the pipeline handles the rest. Sub-agents (prioritizer, designer, coder, reviewer, auditor, infrarian) pick up work from their column when signaled.

## Architecture

- **Daemon** — Long-running process (Unix socket IPC). Owns the dispatcher, agent pool, event bus, and state persistence.
- **TUI** — ratatui terminal UI. Connects to the daemon socket. Shows chat (left), system panels (right: status, epic, kanban, task, logs).
- **MCP server** — JSON-RPC 2.0 over Unix socket (`~/.gitzi/mcp.sock`). Exposes `gitzi_*` tools for sub-agents to call back into the harness.
- **Dispatcher** — Central orchestrator. Manages the board projection, review queue, WIP limits, chat stack (with fork support), and agent lifecycle.
- **Pipeline** — State machine transitions with persistence. Tasks are TOML files on disk.

## Features

- **Fork sessions** — Interrupt mid-conversation with a different topic. The classifier (amend / queue / fork) routes your message appropriately.
- **WIP limits** — Configurable per-column caps prevent agent pile-up.
- **Multi-repo** — Discovers git repos from glob patterns, tracks per-repo merge strategy and test commands.
- **Provider auto-discovery** — Scans for LM Studio, Ollama, and AWS Bedrock (SSO) on first run. No manual config needed.
- **Keyring integration** — API keys migrate from plaintext config into the OS keyring automatically.
- **Branch management** — Creates task branches, fast-forward merges on completion, surfaces conflicts for human decision.

## Install

Download the binary from [GitHub Releases](https://github.com/user/gitzi/releases) or build from source:

```bash
cargo build --release --features tui
```

Targets: Linux (x86_64 musl), macOS (x86_64, aarch64), Windows (x86_64).

## Usage

```bash
gitzi                    # Launch TUI (starts daemon if needed)
gitzi status             # Print current WIP snapshot
gitzi log                # Fullscreen scrollable daemon journal viewer
gitzi generate-config    # Regenerate ~/.gitzi/config.toml from defaults
gitzi --uninstall        # Unregister the background service
```

### Task management (CLI)

```bash
gitzi epic create --title "Auth overhaul"
gitzi task create --epic <epic-id> --title "Add OAuth flow" --priority 50
gitzi advance <task-id> coding --note "Design approved"
```

## Configuration

Lives at `~/.gitzi/config.toml`. Created on first run via the setup wizard.

Key sections:

| Section | Purpose |
|---------|---------|
| `[providers.<name>]` | LLM endpoints (OpenAI-compatible or Bedrock) |
| `[[agents]]` | Agent role definitions with model overrides |
| `[[repos]]` | Per-repo merge strategy, test command, main branch |
| `[wip_limits]` | Per-column WIP cap overrides |
| `test_command` | Global test command (default: `cargo test`) |

See [kb/configuration.md](kb/configuration.md) for full field reference.

## Development

```bash
cargo test             # Run tests (includes property tests via proptest)
cargo clippy --all-features -- -D warnings
cargo fmt --check
```

CI runs on every push to `main` and on PRs. Releases are cut by pushing a `v*` tag.

## State directory

```
~/.gitzi/
├── config.toml          # Configuration
├── daemon.sock          # Daemon IPC socket
├── mcp.sock             # MCP server socket
├── epics/               # Epic TOML files
├── tasks/               # Task TOML files
├── reviews/             # Review item persistence
├── chat.jsonl           # Main chat history
└── service-installed    # Marker for systemd/launchd registration
```
