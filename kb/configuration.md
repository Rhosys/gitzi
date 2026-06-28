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

### `fallback_provider`
The distinguished "control-plane" provider chosen during bootstrap setup
(see ADR-002). It powers the setup experience and the recovery conversation
when the main agent's own provider is absent or not responding. Set
automatically on the first provider activation; names a key in `[providers]`.
- Type: string (optional)
- You normally never edit this by hand — it's written during setup.

### `[providers.<name>]`
Named LLM provider endpoints. On first run gitzi has no valid provider and
enters **setup mode** (ADR-002): the daemon scans for LM Studio / Ollama / AWS
SSO sessions and the TUI shows a one-step picker to activate one. The chosen
provider is wired into the main agent and recorded as `fallback_provider`.
Providers discovered later are recorded here with `enabled = false` — use the
main agent's `gitzi_rediscover_providers`/`gitzi_activate_provider` chat tools to
see what's available and turn one on, rather than editing this file by hand.
- `kind`: `"openai-compatible"` (default) or `"bedrock"`
- `api_url`: OpenAI-compatible endpoint URL. Unused for `bedrock`.
- `api_key`: API key for OpenAI-compatible providers — plaintext or a
  `keyring:<service>/<account>` pointer (gitzi migrates plaintext keys into the OS
  keyring automatically on load).
- `region`: AWS region, for `bedrock` providers.
- `sso_start_url`, `sso_account_id`, `sso_role_name`: AWS SSO identifiers for `bedrock`
  providers, filled in during activation. Credentials are handed to the AWS SDK
  in-process via `crate::aws_sso::SsoCredentialsProvider` — gitzi never writes to
  `~/.aws/config` or any other cloud CLI's config files.
- `model_id`: Bedrock model ID, e.g. `"anthropic.claude-sonnet-4-6-v1:0"`.
- `enabled`: Whether this provider is actually wired into any agent. Discovered
  providers default to `false` until explicitly activated.

### `[[agents]]`
Agent definitions. Each entry defines a role with its model and behavior.
- `role`: One of: main, prioritizer, designer, coder, reviewer, auditor, infrarian
- `model`: Model identifier passed to the API
- `api_url`: Direct endpoint (overrides provider)
- `provider`: Reference a named provider

### `[[repos]]`
Per-repository configuration overrides.
- `slug`: The repo slug (matches the cache slug, e.g. "email-catcher-backend")
- `merge_strategy`: One of: ff-only (default), gitzi-branch, merge-commit, pull-request, push-to-remote
- `test_command`: Override the global test command for this repo
- `main_branch`: Override the target branch for merges (default: "main")
