use std::collections::HashMap;
use std::path::Path;
use serde::{Deserialize, Serialize};
use crate::error::Result;

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

/// A named agent. Define as many as you like under `[[agents]]`.
///
/// ```toml
/// [[agents]]
/// name = "coder"
/// model = "claude-sonnet-4-6"
/// role = "developer"
/// system_prompt = """
/// You are a disciplined coding agent. Make the smallest possible change.
/// """
///
/// [[agents]]
/// name = "planner"
/// model = "claude-opus-4-8"
/// role = "planner"
/// system_prompt = "Break down the epic into precise, minimal tasks."
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    pub name: String,
    #[serde(default = "default_model")]
    pub model: String,
    /// What this agent is for — used by the scheduler for role-based routing.
    /// Free-form string: e.g. "developer", "planner", "reviewer".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// System prompt sent before every task. Falls back to a sensible built-in default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

impl Default for AgentDef {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            model: default_model(),
            role: None,
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

fn default_agent_name() -> String { "default".to_string() }

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

    /// Find an agent by name. Returns the first match or a built-in default.
    pub fn resolve_agent(&self, name: &str) -> &AgentDef {
        self.agents.iter().find(|a| a.name == name)
            .or_else(|| self.agents.first())
            .unwrap_or_else(|| {
                // Safety: only reachable if agents is empty AND first() returned None.
                // Return a static default; the scheduler will fall back to built-in behaviour.
                static FALLBACK: std::sync::OnceLock<AgentDef> = std::sync::OnceLock::new();
                FALLBACK.get_or_init(AgentDef::default)
            })
    }
}

pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
