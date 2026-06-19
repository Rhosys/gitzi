use std::collections::HashMap;
use std::path::Path;
use serde::{Deserialize, Serialize};
use crate::dispatcher::AgentRole;
use crate::error::{GitziError, Result};

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

/// A named connection to a model API, referenced by `AgentDef::provider`.
/// Define as many as you like under `[providers.<name>]`.
///
/// Only the OpenAI-compatible chat-completions wire format is supported today,
/// which is what LM Studio (and most local model servers) speak:
///
/// ```toml
/// [providers.lmstudio]
/// base_url = "http://localhost:1234/v1"
/// # api_key_env = "LMSTUDIO_API_KEY"  # optional; omit if the server needs no auth
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderDef {
    pub base_url: String,
    /// Name of an environment variable to read the API key from at agent start.
    /// Omit for servers that don't require authentication (e.g. local LM Studio).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
}

/// An agent definition. Define as many as you like under `[[agents]]`.
/// The role is the identifier — reference it via `default_agent` or per-task.
///
/// ```toml
/// [[agents]]
/// role = "developer"
/// model = "claude-sonnet-4-6"
/// system_prompt = """
/// You are a disciplined coding agent. Make the smallest possible change.
/// No refactoring, no extras.
/// """
///
/// [[agents]]
/// role = "planner"
/// model = "qwen2.5-coder-32b"
/// provider = "lmstudio"
/// system_prompt = """
/// Break the epic into precise, minimal, independently shippable tasks.
/// """
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    pub role: String,
    #[serde(default = "default_model")]
    pub model: String,
    /// Base URL of the OpenAI-compatible API endpoint.
    /// Defaults to `http://localhost:1234/v1` (LM Studio).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Name of an entry in `[providers]` to talk to the model through.
    /// Omit to keep using the local `claude` CLI subprocess (the original behavior).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// System prompt sent before every task. Falls back to a sensible built-in default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

impl Default for AgentDef {
    fn default() -> Self {
        Self {
            role: "developer".to_string(),
            model: default_model(),
            base_url: None,
            provider: None,
            system_prompt: None,
        }
    }
}

impl AgentDef {
    /// Hardcoded default for the `role = "main"` chat harness agent.
    pub fn default_main() -> Self {
        Self {
            role: "main".to_string(),
            model: "local-model".to_string(),
            base_url: Some("http://localhost:1234/v1".to_string()),
            provider: None,
            system_prompt: Some(
                "You are the main coordination agent for gitzi, an AI-driven software \
                 development pipeline. Help the user manage their project through natural \
                 conversation. Create and refine epics, tasks, and work items. Surface what \
                 needs attention. Keep work moving. Never write code directly. \
                 Ask one question at a time — never more."
                    .to_string(),
            ),
        }
    }
}

fn default_model() -> String { "claude-sonnet-4-6".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub wip_limits: WipLimits,

    /// Name of the agent used when a task does not specify one.
    #[serde(default = "default_agent_name")]
    pub default_agent: String,

    /// All agent definitions. Referenced by name via `default_agent`
    /// or per-task via the `agent` field on a task.
    #[serde(default = "default_agents")]
    pub agents: Vec<AgentDef>,

    #[serde(default = "default_test_command")]
    pub test_command: String,
    #[serde(default)]
    pub integrations: HashMap<String, toml::Value>,

    /// Named model API connections, referenced by `AgentDef::provider`.
    #[serde(default)]
    pub providers: HashMap<String, ProviderDef>,
}

fn default_agent_name() -> String { "developer".to_string() }

fn default_agents() -> Vec<AgentDef> {
    vec![AgentDef::default()]
}

fn default_test_command() -> String { "cargo test".to_string() }

impl Default for Config {
    fn default() -> Self {
        Self {
            wip_limits: WipLimits::default(),
            default_agent: default_agent_name(),
            agents: default_agents(),
            test_command: default_test_command(),
            integrations: HashMap::new(),
            providers: HashMap::new(),
        }
    }
}

impl Config {
    /// Load from `~/.gitzi/config.toml`. Falls back to defaults if missing.
    /// Validates the result before returning.
    pub fn load(_repo_root: &Path) -> Result<Self> {
        let path = crate::state::home::global_config_file();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)?;
        let config: Self = toml::from_str(&text)?;
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

    /// Validate config on load. Returns error for unknown role names or
    /// unknown WIP column overrides.
    pub fn validate(&self) -> Result<()> {
        let mut valid_roles: Vec<String> =
            AgentRole::all().iter().map(|r| r.to_string()).collect();
        valid_roles.push("main".to_string());
        for agent in &self.agents {
            if !valid_roles.contains(&agent.role) {
                return Err(GitziError::Config(format!(
                    "unknown agent role '{}' in config — valid roles: {:?}",
                    agent.role, valid_roles
                )));
            }
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

    /// Find agent by role: config first, then hardcoded default. Never first-in-list.
    pub fn resolve_agent(&self, role: &str) -> AgentDef {
        self.agents
            .iter()
            .find(|a| a.role == role)
            .cloned()
            .unwrap_or_else(|| {
                if role == "main" {
                    return AgentDef::default_main();
                }
                AgentRole::all()
                    .iter()
                    .find(|r| r.to_string() == role)
                    .map(|r| r.default_agent_def())
                    .unwrap_or_else(|| AgentRole::Coder.default_agent_def())
            })
    }
}

pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // Feature: dispatcher-audit-fixes, Property 12: resolve_agent returns config override or hardcoded default
    // **Validates: Requirements 7.1, 7.3, 7.4**
    proptest! {
        #[test]
        fn resolve_agent_returns_config_override_or_hardcoded_default(
            include_flags in prop::collection::vec(any::<bool>(), 7..=7),
            custom_models in prop::collection::vec("[a-z]{3,10}", 7..=7),
            custom_prompts in prop::collection::vec("[a-z ]{5,30}", 7..=7),
        ) {
            let all_roles = AgentRole::all();

            // Build config agents vec: include only roles where flag is true
            let config_agents: Vec<AgentDef> = all_roles.iter().zip(include_flags.iter())
                .filter(|&(_, &include)| include)
                .enumerate()
                .map(|(i, (role, _))| AgentDef {
                    role: role.to_string(),
                    model: custom_models[i % custom_models.len()].clone(),
                    base_url: None,
                    provider: None,
                    system_prompt: Some(custom_prompts[i % custom_prompts.len()].clone()),
                })
                .collect();

            let config = Config {
                agents: config_agents.clone(),
                ..Config::default()
            };

            for (idx, role) in all_roles.iter().enumerate() {
                let role_name = role.to_string();
                let resolved = config.resolve_agent(&role_name);

                // Never returns a mismatched role
                prop_assert_eq!(
                    &resolved.role, &role_name,
                    "resolved agent role '{}' doesn't match queried role '{}'",
                    resolved.role, role_name
                );

                if include_flags[idx] {
                    // Config had an entry — should match the config entry
                    let config_entry = config_agents.iter()
                        .find(|a| a.role == role_name).unwrap();
                    prop_assert_eq!(
                        &resolved.model, &config_entry.model,
                        "model mismatch for role '{}'", role_name
                    );
                    prop_assert_eq!(
                        &resolved.system_prompt, &config_entry.system_prompt,
                        "system_prompt mismatch for role '{}'", role_name
                    );
                } else {
                    // No config entry — should match hardcoded default
                    let default_def = role.default_agent_def();
                    prop_assert_eq!(
                        &resolved.model, &default_def.model,
                        "model should be hardcoded default for role '{}'", role_name
                    );
                    prop_assert_eq!(
                        &resolved.system_prompt, &default_def.system_prompt,
                        "system_prompt should be hardcoded default for role '{}'",
                        role_name
                    );
                }
            }
        }
    }

    // Feature: dispatcher-audit-fixes, Property 11: Config role validation rejects unknown roles
    // **Validates: Requirements 7.2**
    proptest! {
        #[test]
        fn config_role_validation_rejects_unknown_roles(
            role_name in "[a-zA-Z0-9_-]{1,30}"
                .prop_filter("must not match any valid role name", |s| {
                    let valid = [
                        "prioritizer", "designer", "coder",
                        "reviewer", "tester", "auditor", "infrarian", "main",
                    ];
                    !valid.contains(&s.as_str())
                })
        ) {
            let config = Config {
                agents: vec![AgentDef {
                    role: role_name.clone(),
                    model: "claude-sonnet-4-6".to_string(),
                    base_url: None,
                    provider: None,
                    system_prompt: None,
                }],
                ..Config::default()
            };

            let result = config.validate();
            prop_assert!(
                result.is_err(),
                "validate() should reject unknown role '{}'", role_name
            );
            let err_msg = format!("{}", result.unwrap_err());
            prop_assert!(
                err_msg.contains(&role_name),
                "error should contain invalid role '{}', got: {}",
                role_name, err_msg
            );
        }
    }

    #[test]
    fn validate_rejects_agent_referencing_unknown_provider() {
        let config = Config {
            agents: vec![AgentDef {
                role: "coder".to_string(),
                provider: Some("lmstudio".to_string()),
                ..AgentDef::default()
            }],
            ..Config::default()
        };

        let err = config.validate().expect_err("unknown provider should be rejected");
        assert!(err.to_string().contains("lmstudio"));
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
                    base_url: "http://localhost:1234/v1".to_string(),
                    api_key_env: None,
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
                    base_url: "http://localhost:1234/v1".to_string(),
                    api_key_env: Some("LMSTUDIO_API_KEY".to_string()),
                },
            )]),
            ..Config::default()
        };

        let text = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        let provider = parsed.providers.get("lmstudio").unwrap();
        assert_eq!(provider.base_url, "http://localhost:1234/v1");
        assert_eq!(provider.api_key_env.as_deref(), Some("LMSTUDIO_API_KEY"));
    }
}
