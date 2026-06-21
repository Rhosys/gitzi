# gitzi Configuration

Configuration lives at `~/.gitzi/config.toml`. It is created automatically on first run.

## Fields

### `test_command`
The shell command gitzi runs to verify a task after coding completes.
- Type: string
- Default: `"cargo test"`
- Example: `test_command = "npm test"`

### `default_agent`
Which agent role handles tasks that don't specify one.
- Type: string
- Default: `"developer"`

### `fork_auto_close`
When true, the main agent can automatically close fork sessions when the topic is resolved.
- Type: boolean
- Default: `true`

### `repo_paths`
Glob patterns for discovering git repositories. Any directory matching that contains a `.git/` is registered.
- Type: array of strings
- Default: `[]`
- Example: `repo_paths = ["/home/user/projects/*"]`

### `[wip_limits]`
Per-column work-in-progress limit overrides. Columns not listed keep built-in defaults.
- Type: table (column name → integer)
- Example: `coding = 2`

### `[providers.<name>]`
Named LLM provider endpoints.
- `api_url`: OpenAI-compatible endpoint URL
- `api_key`: API key (plaintext — this file is never committed)

### `[[agents]]`
Agent definitions. Each entry defines a role with its model and behavior.
- `role`: One of: main, prioritizer, designer, coder, reviewer, tester, auditor, infrarian
- `model`: Model identifier passed to the API
- `api_url`: Direct endpoint (overrides provider)
- `provider`: Reference a named provider
- `system_prompt`: Override the built-in system prompt

### `[[repos]]`
Per-repository configuration overrides.
- `slug`: The repo slug (matches the cache slug, e.g. "email-catcher-backend")
- `merge_strategy`: One of: ff-only (default), gitzi-branch, merge-commit, pull-request, push-to-remote
- `test_command`: Override the global test command for this repo
- `main_branch`: Override the target branch for merges (default: "main")
