pub mod backend;
pub mod claude_code;
pub mod rig_agent;

pub use backend::{AgentBackend, AgentResult, RunContext};
pub use claude_code::ClaudeCodeCli;
pub use rig_agent::RigAgent;

use crate::config::{Config, DefaultAgent};
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

pub fn build_agent(config: &Config) -> Result<AnyAgent> {
    match config.default_agent {
        DefaultAgent::ClaudeCode => Ok(AnyAgent::ClaudeCode(ClaudeCodeCli)),
        DefaultAgent::Rig => {
            let key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| GitziError::Config("ANTHROPIC_API_KEY not set".into()))?;
            Ok(AnyAgent::Rig(RigAgent::new(key)))
        }
    }
}
