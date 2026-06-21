# gitzi project conventions

## KB articles

- KB articles (`kb/*.md`) must only document **implemented, working functionality**.
- Never document planned features, config fields without behavior, or aspirational designs in the KB.
- If a config field exists in the struct but has no runtime consumer, it does NOT belong in the KB.
- The KB is embedded in the binary and surfaced to users via the main agent — inaccurate KB is worse than no KB.

## Config struct

- Every field on the `Config` struct must have a runtime consumer. Dead config fields are removed.
- Adding a config field requires simultaneously implementing the behavior it controls.

## Merge strategy

- `MergeStrategy` is consumed in `Dispatcher::attempt_merge()` when a task reaches Done.
- `RepoConfig` is resolved via `Config::repo_config(slug)` and drives merge behavior per-repo.
