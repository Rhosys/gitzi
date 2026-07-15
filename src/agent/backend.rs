use crate::error::Result;
use crate::model::Task;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct RunContext {
    pub repo_root: PathBuf,
    pub branch: String,
    /// Work state summary from a previous session (branch commits, diff stats).
    /// Populated on boot resume when recoverable state is found.
    pub resume_summary: Option<String>,
    /// Bearer token for the MCP server. Injected as `GITZI_MCP_TOKEN` env var
    /// when spawning agent subprocesses.
    pub mcp_token: Option<String>,
    /// Answered questions from the human review queue for this task.
    /// Each entry is (question, answer). Injected into the agent prompt so the
    /// agent implements decisions already made without re-asking them.
    pub answered_questions: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub enum AgentResult {
    Success { output: String },
    Failure { output: String },
    Blocked { question: String },
}

pub trait AgentBackend: Send + Sync {
    fn run(
        &self,
        task: &Task,
        ctx: &RunContext,
    ) -> impl std::future::Future<Output = Result<AgentResult>> + Send;
}
