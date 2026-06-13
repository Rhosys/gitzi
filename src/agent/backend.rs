use std::path::PathBuf;
use crate::error::Result;
use crate::model::Task;

#[derive(Debug, Clone)]
pub struct RunContext {
    pub repo_root: PathBuf,
    pub branch: String,
    /// Work state summary from a previous session (branch commits, diff stats).
    /// Populated on boot resume when recoverable state is found.
    pub resume_summary: Option<String>,
    /// Bearer token for the MCP server. Injected as `GITZI_MCP_TOKEN` env var
    /// when spawning agent subprocesses.
    pub mcp_token: Option<String>,
}

#[derive(Debug, Clone)]
pub enum AgentResult {
    Success { output: String },
    Failure { output: String },
    Blocked { question: String },
}

pub trait AgentBackend: Send + Sync {
    fn run(&self, task: &Task, ctx: &RunContext)
        -> impl std::future::Future<Output = Result<AgentResult>> + Send;
}
