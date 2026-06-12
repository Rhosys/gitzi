use std::collections::HashMap;
use std::path::Path;
use serde::{Deserialize, Serialize};
use crate::dispatcher::AgentRole;
use crate::error::{GitziError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WipLimits {
    #[serde(default = "default_wip_in_progress")]
    pub in_progress: u32,
    #[serde(default = "default_wip_waiting")]
    pub waiting_for_review: u32,
    #[serde(default = "default_wip_testing")]
    pub in_testing: u32,
}

impl Default for WipLimits {
    fn default() -> Self {
        Self {
            in_progress: default_wip_in_progress(),
            waiting_for_review: default_wip_waiting(),
            in_testing: default_wip_testing(),
        }
    }
}

fn default_wip_in_progress() -> u32 { 1 }
fn default_wip_waiting() -> u32 { 3 }
fn default_wip_testing() -> u32 { 3 }

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
/// model = "claude-opus-4-8"
/// system_prompt = """
/// Break the epic into precise, minimal, independently shippable tasks.
/// """
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    pub role: String,
    #[serde(default = "default_model")]
    pub model: String,
    /// System prompt sent before every task. Falls back to a sensible built-in default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

impl Default for AgentDef {
    fn default() -> Self {
        Self {
            role: "developer".to_string(),
            model: default_model(),
            system_prompt: None,
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
    #[serde(default = "default_dashboard_port")]
    pub dashboard_port: u16,
    #[serde(default)]
    pub integrations: HashMap<String, toml::Value>,
}

fn default_agent_name() -> String { "developer".to_string() }

fn default_agents() -> Vec<AgentDef> {
    vec![AgentDef::default()]
}

fn default_test_command() -> String { "cargo test".to_string() }
fn default_dashboard_port() -> u16 { 3000 }

impl Default for Config {
    fn default() -> Self {
        Self {
            wip_limits: WipLimits::default(),
            default_agent: default_agent_name(),
            agents: default_agents(),
            test_command: default_test_command(),
            dashboard_port: default_dashboard_port(),
            integrations: HashMap::new(),
        }
    }
}

impl Config {
    /// Load from `~/.gitzi/config.toml`. Falls back to defaults if missing.
    pub fn load(_repo_root: &Path) -> Result<Self> {
        let path = crate::state::home::global_config_file();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)?;
        Ok(toml::from_str(&text)?)
    }

    pub fn write(&self, _repo_root: &Path) -> Result<()> {
        let path = crate::state::home::global_config_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        atomic_write(&path, &text)
    }

    /// Validate config on load. Returns error for unknown role names.
    pub fn validate(&self) -> Result<()> {
        let valid_roles: Vec<String> = AgentRole::all().iter().map(|r| r.to_string()).collect();
        for agent in &self.agents {
            if !valid_roles.contains(&agent.role) {
                return Err(GitziError::Config(format!(
                    "unknown agent role '{}' in config — valid roles: {:?}",
                    agent.role, valid_roles
                )));
            }
        }
        Ok(())
    }

    /// Find agent by role: config first, then hardcoded default. Never first-in-list.
    pub fn resolve_agent(&self, role: &str) -> AgentDef {
        self.agents
            .iter()
            .find(|a| a.role == role)
            .cloned()
            .unwrap_or_else(|| {
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
                .prop_filter("must not match any valid AgentRole display name", |s| {
                    let valid = [
                        "prioritizer", "designer", "coder",
                        "reviewer", "tester", "auditor", "infrarian",
                    ];
                    !valid.contains(&s.as_str())
                })
        ) {
            let config = Config {
                agents: vec![AgentDef {
                    role: role_name.clone(),
                    model: "claude-sonnet-4-6".to_string(),
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
}
