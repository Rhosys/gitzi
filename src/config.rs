use std::collections::HashMap;
use std::path::{Path, PathBuf};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DefaultAgent {
    ClaudeCode,
    Rig,
}

impl Default for DefaultAgent {
    fn default() -> Self { DefaultAgent::ClaudeCode }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub wip_limits: WipLimits,
    #[serde(default)]
    pub default_agent: DefaultAgent,
    #[serde(default = "default_test_command")]
    pub test_command: String,
    #[serde(default = "default_dashboard_port")]
    pub dashboard_port: u16,
    #[serde(default)]
    pub integrations: HashMap<String, toml::Value>,
}

fn default_test_command() -> String { "cargo test".to_string() }
fn default_dashboard_port() -> u16 { 3000 }

impl Default for Config {
    fn default() -> Self {
        Self {
            wip_limits: WipLimits::default(),
            default_agent: DefaultAgent::default(),
            test_command: default_test_command(),
            dashboard_port: default_dashboard_port(),
            integrations: HashMap::new(),
        }
    }
}

impl Config {
    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = repo_root.join(".gitzi").join("config.toml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)?;
        Ok(toml::from_str(&text)?)
    }

    pub fn write(&self, repo_root: &Path) -> Result<()> {
        let path = repo_root.join(".gitzi").join("config.toml");
        let text = toml::to_string_pretty(self)?;
        atomic_write(&path, &text)
    }
}

pub fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn gitzi_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".gitzi")
}
