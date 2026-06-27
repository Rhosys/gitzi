pub mod backend;
mod bedrock_agent;
pub mod classifier;
pub mod claude_code;
pub mod coding_agent;
pub mod coding_tools;
pub mod llm_client;
pub mod main_agent;
mod prompt;
mod rig_agent;

pub use backend::{AgentBackend, AgentResult, RunContext};
pub use bedrock_agent::BedrockAgent;
pub use claude_code::ClaudeCodeCli;
pub use main_agent::{ChatTurn, MainAgent, OaiMessage, OaiTool, ToolCallRequest};
pub use rig_agent::RigAgent;

use crate::config::{AgentDef, Config, ProviderKind};
use crate::dispatcher::AgentRole;
use crate::error::Result;
use crate::model::Task;

/// A runnable pipeline agent backend, selected per-`AgentDef` via `build_agent`.
pub enum PipelineAgent {
    /// Shells out to the local `claude` CLI subprocess (the original, default behavior).
    ClaudeCode(ClaudeCodeCli),
    /// Talks directly to a `[providers.*]` entry over its OpenAI-compatible API (e.g. LM Studio).
    Rig(RigAgent),
    /// Talks to AWS Bedrock via a `[providers.*]` entry with `kind = "bedrock"`.
    Bedrock(BedrockAgent),
    /// Direct LLM tool-calling loop with sandboxed filesystem tools.
    CodingLoop(AgentDef),
}

impl AgentBackend for PipelineAgent {
    async fn run(&self, task: &Task, ctx: &RunContext) -> Result<AgentResult> {
        match self {
            PipelineAgent::ClaudeCode(b) => b.run(task, ctx).await,
            PipelineAgent::Rig(b) => b.run(task, ctx).await,
            PipelineAgent::Bedrock(b) => b.run(task, ctx).await,
            PipelineAgent::CodingLoop(def) => {
                let task_prompt = format!(
                    "Task: {}\n\nDescription: {}\n\nWork in the current directory. \
                     Read existing code first, then make changes. Run tests when done.",
                    task.title,
                    task.description.as_deref().unwrap_or("No description provided."),
                );

                match coding_agent::run(def, &task_prompt, &ctx.repo_root).await {
                    Ok(output) => Ok(AgentResult::Success { output }),
                    Err(e) => Ok(AgentResult::Failure { output: e.to_string() }),
                }
            }
        }
    }
}

/// Build a runnable pipeline agent from its definition. If `def.provider` names
/// an enabled entry in `config.providers`, the agent talks to that endpoint
/// directly via `rig` (OpenAI-compatible) or `rig-bedrock` (AWS Bedrock);
/// otherwise it falls back to the local `claude` CLI subprocess. A provider
/// that exists but isn't `enabled` (discovered but not yet activated) is
/// treated the same as no provider at all.
pub fn build_agent(config: &Config, def: &AgentDef) -> PipelineAgent {
    match def.provider.as_ref().and_then(|name| config.providers.get(name)) {
        Some(provider) if provider.enabled => {
            // System prompt comes from the hardcoded role default, not config.
            let role_prompt = AgentRole::all()
                .iter()
                .find(|r| r.to_string() == def.role)
                .map(|r| r.default_system_prompt().to_string());
            match provider.kind {
                ProviderKind::Bedrock => PipelineAgent::Bedrock(BedrockAgent::new(
                    provider.region.clone().unwrap_or_default(),
                    provider.sso_start_url.clone().unwrap_or_default(),
                    provider.sso_account_id.clone().unwrap_or_default(),
                    provider.sso_role_name.clone().unwrap_or_default(),
                    provider.model_id.clone().unwrap_or_else(|| def.model.clone()),
                    role_prompt,
                )),
                ProviderKind::OpenaiCompatible => PipelineAgent::Rig(RigAgent::new(
                    provider.api_url.clone(),
                    Config::resolve_provider_api_key(provider),
                    def.model.clone(),
                    role_prompt,
                )),
            }
        }
        _ => PipelineAgent::CodingLoop(def.clone()),
    }
}

/// Build the main chat harness agent from its definition, resolving a named
/// `[providers.*]` reference into the concrete endpoint URL the harness talks
/// to. Without this the harness would silently fall back to its localhost
/// default even after the user activated a provider (e.g. Ollama on :11434).
/// The harness speaks OpenAI-compatible HTTP; Bedrock-backed main agents are
/// not yet supported here (pipeline agents handle Bedrock separately).
pub fn build_main_agent(config: &Config, def: &AgentDef) -> MainAgent {
    let mut def = def.clone();
    if def.api_url.is_none()
        && let Some(provider) = def.provider.as_ref().and_then(|n| config.providers.get(n))
        && provider.enabled
        && provider.kind == ProviderKind::OpenaiCompatible
        && !provider.api_url.is_empty()
    {
        def.api_url = Some(provider.api_url.clone());
    }
    MainAgent::new(&def)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderDef;
    use std::collections::HashMap;

    #[test]
    fn build_agent_without_provider_uses_coding_loop() {
        let config = Config::default();
        let def = AgentDef { provider: None, ..AgentDef::default() };

        assert!(matches!(build_agent(&config, &def), PipelineAgent::CodingLoop(_)));
    }

    #[test]
    fn build_agent_with_known_provider_uses_rig() {
        let config = Config {
            providers: HashMap::from([(
                "lmstudio".to_string(),
                ProviderDef {
                    api_url: "http://localhost:1234/v1".to_string(),
                    ..ProviderDef::default()
                },
            )]),
            ..Config::default()
        };
        let def = AgentDef { provider: Some("lmstudio".to_string()), ..AgentDef::default() };

        assert!(matches!(build_agent(&config, &def), PipelineAgent::Rig(_)));
    }

    #[test]
    fn build_agent_with_unconfigured_provider_falls_back_to_coding_loop() {
        let config = Config::default();
        let def = AgentDef { provider: Some("nonexistent".to_string()), ..AgentDef::default() };

        assert!(matches!(build_agent(&config, &def), PipelineAgent::CodingLoop(_)));
    }

    #[test]
    fn build_agent_with_disabled_provider_falls_back_to_coding_loop() {
        let config = Config {
            providers: HashMap::from([(
                "lmstudio".to_string(),
                ProviderDef {
                    api_url: "http://localhost:1234/v1".to_string(),
                    enabled: false,
                    ..ProviderDef::default()
                },
            )]),
            ..Config::default()
        };
        let def = AgentDef { provider: Some("lmstudio".to_string()), ..AgentDef::default() };

        assert!(matches!(build_agent(&config, &def), PipelineAgent::CodingLoop(_)));
    }

    #[test]
    fn build_agent_with_bedrock_provider_uses_bedrock() {
        let config = Config {
            providers: HashMap::from([(
                "bedrock".to_string(),
                ProviderDef {
                    kind: crate::config::ProviderKind::Bedrock,
                    region: Some("us-east-1".to_string()),
                    sso_start_url: Some("https://example.awsapps.com/start".to_string()),
                    sso_account_id: Some("123456789012".to_string()),
                    sso_role_name: Some("AdministratorAccess".to_string()),
                    model_id: Some("anthropic.claude-sonnet-4-6-v1:0".to_string()),
                    ..ProviderDef::default()
                },
            )]),
            ..Config::default()
        };
        let def = AgentDef { provider: Some("bedrock".to_string()), ..AgentDef::default() };

        assert!(matches!(build_agent(&config, &def), PipelineAgent::Bedrock(_)));
    }
}
