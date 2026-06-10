pub mod backend;
pub mod claude_code;
pub mod rig_agent;

pub use backend::{AgentBackend, AgentResult, RunContext};
pub use claude_code::ClaudeCodeCli;
pub use rig_agent::RigAgent;

use crate::config::{AgentBackendKind, AgentDef};
use crate::error::{GitziError, Result};
use crate::model::Task;

pub enum AnyAgent {
    ClaudeCode(ClaudeCodeCli),
    Rig(RigAgent),
}

impl AgentBackend for AnyAgent {
    fn run<'a>(
        &'a self,
        task: &'a Task,
        ctx: &'a RunContext,
    ) -> impl std::future::Future<Output = Result<AgentResult>> + Send + 'a {
        async move {
            match self {
                AnyAgent::ClaudeCode(a) => a.run(task, ctx).await,
                AnyAgent::Rig(a) => a.run(task, ctx).await,
            }
        }
    }
}

/// Build a runnable agent from an `AgentDef`.
pub fn build_agent(def: &AgentDef) -> Result<AnyAgent> {
    match def.backend {
        AgentBackendKind::ClaudeCode => Ok(AnyAgent::ClaudeCode(ClaudeCodeCli::new(
            def.model.clone(),
            def.system_prompt.clone(),
        ))),
        AgentBackendKind::Rig => {
            let key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| GitziError::Config("ANTHROPIC_API_KEY not set".into()))?;
            let mut agent = RigAgent::new(key);
            if let Some(model) = &def.model {
                agent = agent.with_model(model.clone());
            }
            if let Some(prompt) = &def.system_prompt {
                agent = agent.with_system_prompt(prompt.clone());
            }
            Ok(AnyAgent::Rig(agent))
        }
    }
}
