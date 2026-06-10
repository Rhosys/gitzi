pub mod backend;
pub mod claude_code;
mod rig_agent;

pub use backend::{AgentBackend, AgentResult, RunContext};
pub use claude_code::ClaudeCodeCli;

use crate::config::AgentDef;

/// Build a runnable agent from its definition.
pub fn build_agent(def: &AgentDef) -> ClaudeCodeCli {
    ClaudeCodeCli::new(Some(def.model.clone()), def.system_prompt.clone())
}
