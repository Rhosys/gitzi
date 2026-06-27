use std::collections::HashMap;
use std::path::Path;
use serde::{Deserialize, Serialize};
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

/// An LLM provider definition (e.g. LM Studio, Ollama, OpenAI-compatible endpoint).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDef {
    pub api_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub api_key: String,
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
            api_key: String::new(),
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
            return Ok(crate::bootstrap::run()?);
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
        let mut dirty = config.agents.len() != before_len;

        // Strip unknown WIP column names (e.g. removed "testing" column).
        let wip_before = config.wip_limits.overrides.len();
        config.wip_limits.overrides.retain(|name, _| {
            let valid = crate::dispatcher::board::WipLimits::is_valid_column(name);
            if !valid {
                tracing::warn!("stripping unknown WIP column '{name}' from config");
            }
            valid
        });
        if config.wip_limits.overrides.len() != wip_before {
            dirty = true;
        }

        if dirty {
            let _ = config.write(_repo_root);
        }

        config.validate()?;
        Ok(config)
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
                    api_key: String::new(),
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
}
