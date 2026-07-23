# gitzi — Project Guidelines for Claude

## Core principle: one thing at a time

The user is never shown more than one thing requiring their attention at once.
Queues exist; they are worked through one item at a time, sequentially.
This is non-negotiable and applies to every queue in the system.

## Core principle: context relevance is topic-driven, not time-driven

Context priority is determined by what the user is actively discussing — never by
recency or age. A decision from 50 messages ago is more important than a tool output
from 2 messages ago if the user is still operating within that decision's scope.
Timestamps, message age, and "staleness" are never valid signals for what to keep
or discard in a summary. The user's current focus is the only relevance signal.

## Tool naming convention

All tools that gitzi exposes to agents — whether for managing project state, navigating
the TUI, or interacting with the harness — are prefixed with `gitzi_`.

Examples:
- `gitzi_create_epic`
- `gitzi_create_task`
- `gitzi_create_adr`
- `gitzi_resolve_adr`
- `gitzi_update_task`
- `gitzi_park_task`
- `gitzi_prioritize_task`
- `gitzi_list_epics`
- `gitzi_list_tasks`
- `gitzi_get_adr`
- `gitzi_switch_panel`

Third-party tools (file system, shell, etc.) keep their own naming: `Bash`, `Edit`,
`Write`, `Read`, `Glob`, `Grep`.

When adding a new tool that gitzi owns, always use the `gitzi_` prefix.
