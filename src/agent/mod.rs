pub mod backend;
pub mod claude_code;
pub mod main_agent;
mod prompt;
mod rig_agent;

pub use backend::{AgentBackend, AgentResult, RunContext};
pub use claude_code::ClaudeCodeCli;
pub use main_agent::{ChatTurn, MainAgent, OaiMessage, ToolCallRequest};
pub use rig_agent::RigAgent;

use crate::config::{AgentDef, Config};
use crate::error::Result;
use crate::model::Task;

/// A runnable pipeline agent backend, selected per-`AgentDef` via `build_agent`.
pub enum PipelineAgent {
    /// Shells out to the local `claude` CLI subprocess (the original, default behavior).
    ClaudeCode(ClaudeCodeCli),
    /// Talks directly to a `[providers.*]` entry over its OpenAI-compatible API (e.g. LM Studio).
    Rig(RigAgent),
}

impl AgentBackend for PipelineAgent {
    async fn run(&self, task: &Task, ctx: &RunContext) -> Result<AgentResult> {
        match self {
            PipelineAgent::ClaudeCode(b) => b.run(task, ctx).await,
            PipelineAgent::Rig(b) => b.run(task, ctx).await,
        }
    }
}

/// Build a runnable pipeline agent from its definition. If `def.provider` names
/// an entry in `config.providers`, the agent talks to that endpoint directly via
/// `rig`; otherwise it falls back to the local `claude` CLI subprocess.
pub fn build_agent(config: &Config, def: &AgentDef) -> PipelineAgent {
    match def.provider.as_ref().and_then(|name| config.providers.get(name)) {
        Some(provider) => {
            let api_key = provider
                .api_key_env
                .as_ref()
                .and_then(|var| std::env::var(var).ok())
                .unwrap_or_default();
            PipelineAgent::Rig(RigAgent::new(
                provider.base_url.clone(),
                api_key,
                def.model.clone(),
                def.system_prompt.clone(),
            ))
        }
        None => PipelineAgent::ClaudeCode(ClaudeCodeCli::new(
            Some(def.model.clone()),
            def.system_prompt.clone(),
        )),
    }
}

/// Build the main chat harness agent from its definition.
pub fn build_main_agent(def: &AgentDef) -> MainAgent {
    MainAgent::new(def)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderDef;
    use std::collections::HashMap;

    #[test]
    fn build_agent_without_provider_uses_claude_code_cli() {
        let config = Config::default();
        let def = AgentDef { provider: None, ..AgentDef::default() };

        assert!(matches!(build_agent(&config, &def), PipelineAgent::ClaudeCode(_)));
    }

    #[test]
    fn build_agent_with_known_provider_uses_rig() {
        let config = Config {
            providers: HashMap::from([(
                "lmstudio".to_string(),
                ProviderDef {
                    base_url: "http://localhost:1234/v1".to_string(),
                    api_key_env: None,
                },
            )]),
            ..Config::default()
        };
        let def = AgentDef { provider: Some("lmstudio".to_string()), ..AgentDef::default() };

        assert!(matches!(build_agent(&config, &def), PipelineAgent::Rig(_)));
    }

    #[test]
    fn build_agent_with_unconfigured_provider_falls_back_to_claude_code_cli() {
        let config = Config::default();
        let def = AgentDef { provider: Some("nonexistent".to_string()), ..AgentDef::default() };

        assert!(matches!(build_agent(&config, &def), PipelineAgent::ClaudeCode(_)));
    }
}
