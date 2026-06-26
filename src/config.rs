use std::collections::HashMap;
use std::path::Path;
use serde::{Deserialize, Serialize};
use tracing::warn;
use crate::dispatcher::AgentRole;
use crate::error::{GitziError, Result};

/// Merge strategy for completed tasks.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum MergeStrategy {
    /// Fast-forward only. If not possible, create a review item.
    #[default]
    FfOnly,
    /// Merge into a dedicated gitzi branch (no main merge).
    GitziBranch,
    /// Merge with conflict resolution (create merge commit).
    MergeCommit,
    /// Create a pull/merge request on the remote.
    PullRequest,
    /// Push branch to remote without merging.
    PushToRemote,
}

/// Per-repo configuration overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub path: String,
    #[serde(default)]
    pub merge_strategy: MergeStrategy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_branch: Option<String>,
}

/// Resolved configuration for a repo (defaults applied).
pub struct ResolvedRepoConfig {
    pub merge_strategy: MergeStrategy,
    pub main_branch: String,
}

/// Which kind of model server a `[providers.*]` entry talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    /// Any server speaking the OpenAI chat-completions wire format (LM Studio,
    /// Ollama, etc.) — reached over `api_url` with `api_key`.
    #[default]
    OpenaiCompatible,
    /// AWS Bedrock, reached through the AWS SDK credential chain via a named
    /// profile (see `profile`) rather than a URL/key pair.
    Bedrock,
}

/// An LLM provider definition (e.g. LM Studio, Ollama, AWS Bedrock).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDef {
    #[serde(default)]
    pub kind: ProviderKind,

    /// Base URL for OpenAI-compatible providers. Unused for Bedrock.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_url: String,
    /// API key for OpenAI-compatible providers. May be plaintext (legacy
    /// configs) or a `keyring:<service>/<account>` pointer — always resolve
    /// with `crate::secrets::resolve_secret` before use. Unused for Bedrock.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,

    /// AWS region for Bedrock (e.g. "us-east-1").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Named AWS CLI profile carrying credentials for Bedrock. gitzi writes
    /// this profile into `~/.aws/config` with
    /// `credential_process = gitzi creds-helper aws --provider <name>`
    /// pointing back at the keyring-stored SSO session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// AWS SSO start URL — re-used to resume/refresh login and to key the
    /// keyring entry holding the SSO access token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sso_start_url: Option<String>,
    /// AWS account ID chosen during SSO role-credential exchange.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sso_account_id: Option<String>,
    /// AWS SSO permission-set/role name chosen during role-credential exchange.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sso_role_name: Option<String>,
    /// Bedrock model ID, e.g. "anthropic.claude-sonnet-4-6-v1:0".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,

    /// Whether this provider is actually wired up for use. Providers found
    /// during discovery are recorded here so they show up for the user to
    /// choose from, but stay `enabled = false` (and unreferenced by any
    /// agent) until explicitly activated — see `gitzi_rediscover_providers`
    /// and `gitzi_activate_provider`.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ProviderDef {
    fn default() -> Self {
        Self {
            kind: ProviderKind::default(),
            api_url: String::new(),
            api_key: String::new(),
            region: None,
            profile: None,
            sso_start_url: None,
            sso_account_id: None,
            sso_role_name: None,
            model_id: None,
            enabled: true,
        }
    }
}

/// Per-column WIP limit overrides, as configured in `config.toml`:
///
/// ```toml
/// [wip_limits]
/// coding = 2
/// coding-buffer = 3
/// ```
///
/// Keys are column names in kebab-case (matching how `Column` serializes).
/// Columns not listed keep their built-in default — see
/// `dispatcher::board::WipLimits::default`. Unknown keys are rejected by
/// `dispatcher::board::WipLimits::from_config` at load time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WipLimits {
    pub overrides: HashMap<String, u32>,
}

/// An agent definition. Define as many as you like under `[[agents]]`.
/// The role is the identifier — reference it per-task or via role fallback.
///
/// ```toml
/// [[agents]]
/// role = "main"
/// model = "local-model"
/// provider = "lmstudio"
///
/// [[agents]]
/// role = "coder"
/// model = "claude-sonnet-4-6"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    pub role: String,
    #[serde(default = "default_model")]
    pub model: String,
    /// Direct API URL for this agent. Mutually exclusive with `provider`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    /// Reference a named entry in `[providers]` instead of a direct `api_url`.
    /// Omit to keep using the local `claude` CLI subprocess (the original behavior).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

impl Default for AgentDef {
    fn default() -> Self {
        Self {
            role: String::new(),
            model: default_model(),
            api_url: None,
            provider: None,
        }
    }
}

fn default_model() -> String { "claude-sonnet-4-6".to_string() }

fn default_providers() -> HashMap<String, ProviderDef> {
    HashMap::from([
        ("lmstudio".to_string(), ProviderDef {
            api_url: "http://localhost:1234/v1".to_string(),
            ..ProviderDef::default()
        }),
    ])
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub wip_limits: WipLimits,

    /// Named LLM provider endpoints. Reference by name in an agent's `provider` field.
    #[serde(default = "default_providers")]
    pub providers: HashMap<String, ProviderDef>,

    /// All agent definitions. Override built-in defaults for any role.
    #[serde(default)]
    pub agents: Vec<AgentDef>,

    #[serde(default)]
    pub integrations: HashMap<String, toml::Value>,
    /// Glob patterns for discovering repos managed by gitzi.
    /// Each pattern is expanded and checked for a `.git/` directory.
    #[serde(default)]
    pub repo_paths: Vec<String>,
    /// Per-repository configuration overrides.
    #[serde(default)]
    pub repos: Vec<RepoConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            wip_limits: WipLimits::default(),
            providers: default_providers(),
            agents: Vec::new(),
            integrations: HashMap::new(),
            repo_paths: Vec::new(),
            repos: Vec::new(),
        }
    }
}

impl Config {
    /// Load from `~/.gitzi/config.toml`.
    /// On first run (file absent) writes a fully-commented scaffold and returns its values.
    /// Validates the result before returning.
    pub fn load(_repo_root: &Path) -> Result<Self> {
        let path = crate::state::home::global_config_file();
        if !path.exists() {
            return crate::bootstrap::run();
        }
        let text = std::fs::read_to_string(&path)?;
        let mut config: Self = toml::from_str(&text)?;

        // Strip unknown roles silently and rewrite config if changed.
        let mut valid_roles: Vec<String> =
            AgentRole::all().iter().map(|r| r.to_string()).collect();
        valid_roles.push("main".to_string());
        valid_roles.push("verifier".to_string());
        let before_len = config.agents.len();
        config.agents.retain(|a| valid_roles.contains(&a.role));
        let roles_changed = config.agents.len() != before_len;

        let secrets_changed = config.migrate_secrets();

        if roles_changed || secrets_changed {
            let _ = config.write(_repo_root);
        }

        config.validate()?;
        Ok(config)
    }

    /// Move any plaintext `api_key` values into the OS keyring, replacing
    /// them in-memory with `keyring:<service>/<account>` pointers. Returns
    /// `true` if anything changed (caller should persist the rewrite).
    /// Values already in pointer form, or providers that don't use
    /// `api_key` (Bedrock), are left untouched.
    fn migrate_secrets(&mut self) -> bool {
        let mut changed = false;
        for (name, provider) in self.providers.iter_mut() {
            if provider.kind != ProviderKind::OpenaiCompatible || provider.api_key.is_empty()
                || crate::secrets::is_pointer(&provider.api_key)
            {
                continue;
            }
            let service = format!("gitzi-provider-{name}");
            match crate::secrets::store_secret(&service, "api-key", &provider.api_key) {
                Ok(pointer) => {
                    provider.api_key = pointer;
                    changed = true;
                }
                Err(e) => {
                    warn!(provider = %name, error = %e, "failed to migrate api_key into the OS keyring — leaving it as plaintext in config.toml");
                }
            }
        }
        changed
    }

    pub fn write(&self, _repo_root: &Path) -> Result<()> {
        let path = crate::state::home::global_config_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        atomic_write(&path, &text)
    }



    /// Validate config on load. Returns error for provider references that don't exist
    /// or unknown WIP column overrides.
    pub fn validate(&self) -> Result<()> {
        for agent in &self.agents {
            if let Some(provider) = &agent.provider
                && !self.providers.contains_key(provider)
            {
                return Err(GitziError::Config(format!(
                    "agent '{}' references unknown provider '{}' — define it under [providers.{}]",
                    agent.role, provider, provider
                )));
            }
        }
        crate::dispatcher::board::WipLimits::from_config(&self.wip_limits.overrides)
            .map_err(GitziError::Config)?;
        Ok(())
    }

    /// Find agent by role: config first, then inherit from `main` entry's
    /// model/provider/api_url. Never returns a hardcoded system_prompt — those
    /// live in `AgentRole::default_system_prompt()` and `main_agent.rs`.
    pub fn resolve_agent(&self, role: &str) -> AgentDef {
        if let Some(agent) = self.agents.iter().find(|a| a.role == role) {
            return agent.clone();
        }
        // Fall back to main role's model/provider/api_url
        let main = self.agents.iter().find(|a| a.role == "main");
        AgentDef {
            role: role.to_string(),
            model: main.map(|m| m.model.clone())
                .unwrap_or_else(|| "local-model".to_string()),
            api_url: main.and_then(|m| m.api_url.clone())
                .or(Some("http://localhost:1234/v1".to_string())),
            provider: main.and_then(|m| m.provider.clone()),
        }
    }

    /// Resolve a provider by name, if it exists.
    pub fn resolve_provider(&self, name: &str) -> Option<&ProviderDef> {
        self.providers.get(name)
    }

    /// Resolve a provider's `api_key` field to its real secret value
    /// (following a `keyring:` pointer if present). Falls back to the raw
    /// stored value on lookup failure so callers degrade rather than panic;
    /// the underlying connection attempt will simply fail with a clear error.
    pub fn resolve_provider_api_key(provider: &ProviderDef) -> String {
        crate::secrets::resolve_secret(&provider.api_key).unwrap_or_else(|e| {
            warn!(error = %e, "failed to resolve provider api_key from keyring");
            provider.api_key.clone()
        })
    }

    /// Resolve config for a specific repo path. Falls back to global defaults.
    pub fn repo_config(&self, repo_path: &str) -> ResolvedRepoConfig {
        let repo = self.repos.iter().find(|r| r.path == repo_path);
        ResolvedRepoConfig {
            merge_strategy: repo.map(|r| r.merge_strategy.clone()).unwrap_or_default(),
            main_branch: repo
                .and_then(|r| r.main_branch.clone())
                .unwrap_or_else(|| "main".to_string()),
        }
    }
}

pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Build the default config.toml content with explanatory comments.
/// Only used in tests now that bootstrap generates config dynamically.
#[cfg(test)]
fn render_scaffold_toml() -> String {
    r#"# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# gitzi configuration
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━


# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Kanban
# Controls how many tasks can be active in each pipeline stage.
# Lower values keep focus tight; raise when you want parallelism.
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
[wip_limits]
designing = 1
coding = 1
reviewing = 1
auditing = 1
deploying = 1


# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Agents
# Each role handles one pipeline stage. Set model and provider only
# — system prompts are managed internally. Undefined roles inherit
# from the main agent.
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

[[agents]]
role = "main"
model = "local-model"
provider = "lmstudio"

[[agents]]
role = "prioritizer"
model = "local-model"
provider = "lmstudio"

[[agents]]
role = "designer"
model = "local-model"
provider = "lmstudio"

[[agents]]
role = "coder"
model = "local-model"
provider = "lmstudio"

[[agents]]
role = "reviewer"
model = "local-model"
provider = "lmstudio"

[[agents]]
role = "auditor"
model = "local-model"
provider = "lmstudio"

[[agents]]
role = "infrarian"
model = "local-model"
provider = "lmstudio"


# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Providers
# LLM endpoints that agents connect to. Reference by name in the
# agent's `provider` field. api_key is plaintext — never commit this.
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

[providers.lmstudio]
api_url = "http://localhost:1234/v1"


# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Repos
# Glob patterns for discovering git repositories. Then per-repo
# overrides for merge behavior.
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

# Directories matching these globs that contain .git/ are tracked.
repo_paths = []

# Per-repo overrides. Identified by filesystem path.
# merge_strategy options:
#   ff-only        — fast-forward merge into main. Fails if not possible.
#   gitzi-branch   — merge into a local "gitzi" branch (main untouched).
#   merge-commit   — create a merge commit on main (allows non-linear history).
#   pull-request   — push branch to remote + create PR via gh/glab CLI.
#   push-to-remote — push branch to remote, no merge or PR.
#
# [[repos]]
# path = "/home/user/projects/myapp"
# merge_strategy = "ff-only | gitzi-branch | merge-commit | pull-request | push-to-remote"
# main_branch = "main"


# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# General
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
"#.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn scaffold_toml_parses_and_validates() {
        let text = render_scaffold_toml();
        let config: Config = toml::from_str(&text)
            .unwrap_or_else(|e| panic!("scaffold failed to parse: {e}\n\n{text}"));
        config.validate().expect("scaffold config failed validate()");
        assert!(config.providers.contains_key("lmstudio"));
        assert!(!config.agents.is_empty(), "scaffold should include agent entries");
        let roles: Vec<&str> = config.agents.iter().map(|a| a.role.as_str()).collect();
        assert!(roles.contains(&"main"));
    }

    // Property: resolve_agent returns config override or falls back to main's model/provider
    proptest! {
        #[test]
        fn resolve_agent_returns_config_override_or_main_fallback(
            include_flags in prop::collection::vec(any::<bool>(), 6..=6),
            custom_models in prop::collection::vec("[a-z]{3,10}", 6..=6),
        ) {
            let all_roles = AgentRole::all();

            let config_agents: Vec<AgentDef> = all_roles.iter().zip(include_flags.iter())
                .filter(|&(_, &include)| include)
                .enumerate()
                .map(|(i, (role, _))| AgentDef {
                    role: role.to_string(),
                    model: custom_models[i % custom_models.len()].clone(),
                    api_url: None,
                    provider: None,
                })
                .collect();

            let config = Config {
                agents: config_agents.clone(),
                ..Config::default()
            };

            for (idx, role) in all_roles.iter().enumerate() {
                let role_name = role.to_string();
                let resolved = config.resolve_agent(&role_name);

                prop_assert_eq!(
                    &resolved.role, &role_name,
                    "resolved agent role '{}' doesn't match queried role '{}'",
                    resolved.role, role_name
                );

                if include_flags[idx] {
                    let config_entry = config_agents.iter()
                        .find(|a| a.role == role_name).unwrap();
                    prop_assert_eq!(
                        &resolved.model, &config_entry.model,
                        "model mismatch for role '{}'", role_name
                    );
                } else {
                    // Falls back to "local-model" (main fallback default)
                    prop_assert_eq!(
                        &resolved.model, "local-model",
                        "model should be main-fallback default for role '{}'",
                        role_name
                    );
                }
            }
        }
    }

    #[test]
    fn validate_rejects_agent_referencing_unknown_provider() {
        let config = Config {
            agents: vec![AgentDef {
                role: "coder".to_string(),
                provider: Some("nonexistent".to_string()),
                ..AgentDef::default()
            }],
            ..Config::default()
        };

        let err = config.validate().expect_err("unknown provider should be rejected");
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn validate_accepts_agent_referencing_known_provider() {
        let config = Config {
            agents: vec![AgentDef {
                role: "coder".to_string(),
                provider: Some("lmstudio".to_string()),
                ..AgentDef::default()
            }],
            providers: HashMap::from([(
                "lmstudio".to_string(),
                ProviderDef {
                    api_url: "http://localhost:1234/v1".to_string(),
                    ..ProviderDef::default()
                },
            )]),
            ..Config::default()
        };

        config.validate().expect("known provider reference should be accepted");
    }

    #[test]
    fn providers_table_round_trips_through_toml() {
        let config = Config {
            providers: HashMap::from([(
                "lmstudio".to_string(),
                ProviderDef {
                    api_url: "http://localhost:1234/v1".to_string(),
                    api_key: "sk-test".to_string(),
                    ..ProviderDef::default()
                },
            )]),
            ..Config::default()
        };

        let text = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        let provider = parsed.providers.get("lmstudio").unwrap();
        assert_eq!(provider.api_url, "http://localhost:1234/v1");
        assert_eq!(provider.api_key, "sk-test");
    }

    #[test]
    fn bedrock_provider_round_trips_through_toml() {
        let config = Config {
            providers: HashMap::from([(
                "bedrock".to_string(),
                ProviderDef {
                    kind: ProviderKind::Bedrock,
                    region: Some("us-east-1".to_string()),
                    profile: Some("gitzi-bedrock".to_string()),
                    model_id: Some("anthropic.claude-sonnet-4-6-v1:0".to_string()),
                    enabled: false,
                    ..ProviderDef::default()
                },
            )]),
            ..Config::default()
        };

        let text = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        let provider = parsed.providers.get("bedrock").unwrap();
        assert_eq!(provider.kind, ProviderKind::Bedrock);
        assert_eq!(provider.region.as_deref(), Some("us-east-1"));
        assert_eq!(provider.profile.as_deref(), Some("gitzi-bedrock"));
        assert!(!provider.enabled);
    }

    #[test]
    fn legacy_provider_toml_without_new_fields_still_parses() {
        // Configs written before this change have no `kind`/`enabled` keys.
        let text = r#"
            [providers.lmstudio]
            api_url = "http://localhost:1234/v1"
        "#;
        let config: Config = toml::from_str(text).unwrap();
        let provider = config.providers.get("lmstudio").unwrap();
        assert_eq!(provider.kind, ProviderKind::OpenaiCompatible);
        assert!(provider.enabled);
    }
}
