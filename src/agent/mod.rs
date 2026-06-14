pub mod backend;
pub mod claude_code;
pub mod main_agent;
mod rig_agent;

pub use backend::{AgentBackend, AgentResult, RunContext};
pub use claude_code::ClaudeCodeCli;
pub use main_agent::{ChatTurn, MainAgent, OaiMessage, ToolCallRequest};

use crate::config::AgentDef;

/// Build a runnable pipeline agent from its definition.
pub fn build_agent(def: &AgentDef) -> ClaudeCodeCli {
    ClaudeCodeCli::new(Some(def.model.clone()), def.system_prompt.clone())
}

/// Build the main chat harness agent from its definition.
pub fn build_main_agent(def: &AgentDef) -> MainAgent {
    MainAgent::new(def)
}
