# Bootstrap — First-Time Experience

## Overview

When a user runs `gitzi` for the first time (no `~/.gitzi/` exists), the bootstrapper
(`src/bootstrap.rs`) runs once before the TUI launches. It's a **quick, non-blocking
scan** — it never starts servers, loads models, or opens a browser for SSO login. It
writes a starting `config.toml` and the TUI launches immediately afterward.

Every provider it finds is recorded under `[providers.*]` with `enabled = false`. None
are wired into `[[agents]]`. Every agent role falls back to the local `claude` CLI
subprocess until the user explicitly activates a provider — see "Activation" below.
This is deliberate: onboarding must never get stuck waiting on a server to start, a
model to load, or a login to complete.

## Discovery

`discover_providers()` checks, in order:

1. **LM Studio** — `~/.lmstudio/bin/lms` exists → records it, with `running`/`model_loaded`
   flags from a port check + `/v1/models`. Never started or loaded automatically.
2. **Ollama** — `which ollama` → same running/model-loaded probing, never started.
3. **AWS Bedrock** — if `~/.aws/config` has one or more `[sso-session NAME]` blocks with
   a `sso_start_url`, one `bedrock-<name>` candidate is recorded per session (region and
   start URL pre-filled from the file). If no SSO sessions are configured but the `aws`
   CLI is installed, a single generic `bedrock` candidate is recorded with no region/start
   URL — the user fills those in (or re-runs discovery after configuring an SSO session).

Results are sorted (running + model loaded first) purely for display; no automatic
selection happens.

`discover_repo_paths()` scans common locations (`~/git/`, `~/projects/`, `~/code/`,
`~/src/`, `~/repos/`) and writes the common ancestor as a glob in `repo_paths`, same as
before.

## Activation

Discovery is informational only. The main agent has two tools (see
`src/agent/main_agent.rs`, dispatched in `src/dispatcher/mod.rs`) for turning a
discovered provider into one that's actually used:

- **`gitzi_rediscover_providers`** — re-runs `discover_providers()` and merges any
  newly-found providers into `~/.gitzi/config.toml` as disabled entries (existing
  entries, including ones the user has already activated, are left untouched). Returns
  the full list with status for the agent to present to the user.
- **`gitzi_activate_provider`** — activates a named provider:
  - **OpenAI-compatible** (LM Studio, Ollama): sets `enabled = true` and points the
    `main` agent's `provider` field at it. Immediate.
  - **Bedrock**: may take several calls. The first call starts (or resumes) an AWS SSO
    device-authorization login, opening a browser and blocking until the user approves
    it. Once logged in, the next call lists AWS accounts (the user picks one and the
    agent re-calls with `account_id` set), then lists SSO roles within that account (the
    user picks one and the agent re-calls with `role_name` set too). The final call
    exchanges for real credentials once to validate them, writes a
    `credential_process`-based profile into `~/.aws/config` (see `crate::aws_sso`), and
    activates the provider.

Both tools read and write `~/.gitzi/config.toml` directly rather than going through the
running daemon's in-memory `Config` (which is an immutable `Arc` for the daemon's
lifetime) — activating a provider always requires a `gitzi` restart to take effect, and
the tool's response says so.

## Status Panel

The TUI's Status panel shows discovered providers and repos on first run (see
`docs/bootstrap-spec.md` for the exact panel forms). Providers stay visible there
whether or not they're enabled, so the user can see what's available without asking.
