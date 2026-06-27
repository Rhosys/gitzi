# ADR-002: Bootstrap Is a Backend-Owned "Setup or Use" Gate

**Status:** Accepted  
**Date:** 2026-06-27

## Context

First-run discovery currently happens inside `Config::load`: on a missing
config file it synchronously runs `bootstrap::run()`, probes for local model
servers / SSO sessions, and writes a config with everything `enabled = false`
and no agents. Two problems follow from this:

1. **File existence is the wrong signal.** Discovery only runs when the config
   file is absent. A user who starts gitzi before installing an LLM (or before
   `aws sso login`) gets a config file written once; on every subsequent run
   the file exists, so we never re-check, and the app loads into a board with
   zero working agents — a dead app that silently does nothing.

2. **There is no good model we can ship inside the app.** So we cannot assume a
   guaranteed local fallback agent exists to *conversationally* walk the user
   through setup. The zero-state cannot be agent-driven (chicken-and-egg: the
   agent that would run setup needs the provider that setup creates).

The earlier mental model — "the discovery scan is non-blocking, so the app can
never block loading" — conflated two independent properties: the *scan* should
be fast and time-boxed, but the *gate* ("do we have a working LLM?") should
absolutely stop the user from entering a useless board. Non-blocking scan does
not imply always-load.

## Decision

Treat the entire experience as a binary state owned by the backend (daemon):
**setup an LLM** or **use an LLM**. There is no degraded in-between. The logic
lives in the daemon; the TUI is a thin renderer so frontends stay portable.

### The gate is binary, evaluated every load

On startup the daemon checks whether config resolves to an enabled provider
that exists. If not, it enters **setup mode**: the dispatcher idles, no agents
come alive, and the frontend shows setup the entire time. The gate is not
gated on file existence — it is re-evaluated every run.

### Backend-owned setup state machine

The daemon owns a bootstrap state machine and publishes its state over the
existing event bus:

```
Loading → NeedsProvider { candidates } → Error { messages, can_rescan } → Ready
```

Inbound commands from the frontend: `SelectProvider`, `Rescan`. Discovery,
activation, and validation all run in the daemon, asynchronously, so the
splash reflects real progress instead of blocking the process.

- **Loading** — the time-boxed scan (LM Studio / Ollama / SSO) runs.
- **NeedsProvider** — discovered candidates are offered; the user picks one to
  activate. This is the *only* human-facing setup step. Everything else (other
  roles, pipeline agents) is deferred until after a valid provider exists.
- **Error** — if the scan finds nothing, or activation fails, every error
  message is displayed with the ability to rescan. This is a real terminal
  state of the gate, not a silent fall-through to the board.
- **Ready** — a valid provider exists; agents come alive and the chat
  interface is presented.

### The fallback LLM is the control-plane brain

The provider activated at bootstrap is persisted as a *distinguished* fallback
provider, separate from whatever `main` is later pointed at (e.g. Bedrock). Its
job is narrow and specific:

1. Power the setup experience itself.
2. When `main`'s own provider is absent or not responding, run the "your main
   provider isn't working — what do you want to do?" conversation.

It is **not** a transparent failover for the user's real work. We never quietly
answer a `main` prompt with the fallback. If there is no valid LLM at all, we
are not in "use" — we are back in setup. The recovery conversation is itself
LLM-driven *by the fallback brain*, not a fixed menu.

### `Config::load` stops discovering

`Config::load` becomes pure read-and-report. The scan/activate/validate logic
moves into the daemon's setup phase. Load reporting "nothing valid" is a normal
outcome that drives the state machine into setup mode rather than an error.

### One implementation, two entry points

`gitzi_rediscover_providers` / `gitzi_activate_provider` become thin wrappers
over the same backend setup logic. The pre-agent TUI path (during bootstrap)
and the post-bootstrap agent-driven path share a single implementation, so
there is one source of truth for "discover" and "activate".

## Consequences

- The TUI is a switch over `SetupState` (splash / picker / error+rescan / chat)
  plus relaying `SelectProvider` / `Rescan`. No setup logic in the frontend —
  it can be ported to other frontends by re-rendering the same backend state.
- The daemon must support running with no live agents (setup mode) as a
  first-class state, not an error.
- A new distinguished "fallback provider" concept is added to config, with its
  own activation and persistence, separate from per-agent `provider` fields.
- Discovery moving out of `Config::load` means load no longer has side effects
  (no config rewrite to seed disabled providers); seeding happens in the setup
  phase instead.
- Every startup pays a scan only when the gate is unsatisfied; once a valid
  provider exists, startup goes straight to `Ready`.

## Alternatives Considered

1. **Transparent per-request failover to the fallback** — Rejected. It hides a
   broken `main` from the user and silently changes which model does their
   work. The binary setup/use model keeps the user in control: a broken `main`
   triggers an explicit recovery conversation, not a silent swap.

2. **Ship a guaranteed local fallback model so setup can be conversational from
   the zero-state** — Rejected. There is no model good enough to embed in the
   app. Setup must therefore be backend-logic-driven (no LLM required) with the
   TUI as a plain renderer.

3. **Keep setup orchestration in the TUI** — Rejected. "The human interacts
   here" is not "the logic lives here." Putting discovery/activation/validation
   in the frontend would have to be reimplemented for every future frontend.
   The backend owns the state machine; the frontend renders it.

4. **Gate on config-file existence (status quo)** — Rejected. It only checks
   once, so a user who sets up an LLM after first run is never re-discovered,
   and a user whose provider later breaks is dropped into a dead board. The
   gate must be re-evaluated every load against whether a provider actually
   resolves.
