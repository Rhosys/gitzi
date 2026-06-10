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

/// Which runtime backs an agent definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AgentBackendKind {
    /// Spawn the `claude` CLI subprocess (default).
    #[default]
    ClaudeCode,
    /// Call the Anthropic API directly via rig-core.
    Rig,
}

/// A named agent definition stored under `[agents.<name>]` in config.toml.
///
/// Example:
/// ```toml
/// [agents.coder]
/// backend = "claude-code"
/// system_prompt = "You are a disciplined coding agent..."
///
/// [agents.planner]
/// backend = "rig"
/// model = "claude-opus-4-8"
/// system_prompt = "You are a software planning agent..."
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentDef {
    /// Which backend runs this agent.
    #[serde(default)]
    pub backend: AgentBackendKind,

    /// Model override. If omitted, each backend uses its own default.
    /// For `rig`: any Anthropic model ID (e.g. `"claude-opus-4-8"`).
    /// For `claude-code`: passed via `--model` to the `claude` CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// System prompt / preamble. Replaces the built-in default when set.
    /// The task description and any rejection feedback are always appended after.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub wip_limits: WipLimits,

    /// Name of the agent used when a task does not specify one.
    #[serde(default = "default_agent_name")]
    pub default_agent: String,

    /// Named agent definitions. Add as many as you like; refer to them by
    /// name via `default_agent` or on a task with `agent = "<name>"`.
    #[serde(default = "default_agents")]
    pub agents: HashMap<String, AgentDef>,

    #[serde(default = "default_test_command")]
    pub test_command: String,
    #[serde(default = "default_dashboard_port")]
    pub dashboard_port: u16,
    #[serde(default)]
    pub integrations: HashMap<String, toml::Value>,
}

fn default_agent_name() -> String { "default".to_string() }

fn default_agents() -> HashMap<String, AgentDef> {
    let mut m = HashMap::new();
    m.insert("default".to_string(), AgentDef::default());
    m
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

    /// Look up an agent by name, falling back to `AgentDef::default()` if not found.
    pub fn resolve_agent(&self, name: &str) -> AgentDef {
        self.agents.get(name).cloned().unwrap_or_default()
    }

    /// Resolve the session-wide default agent.
    pub fn default_agent_def(&self) -> AgentDef {
        self.resolve_agent(&self.default_agent)
    }
}

pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
