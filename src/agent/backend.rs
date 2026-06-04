use std::path::PathBuf;
use crate::error::Result;
use crate::model::Task;

#[derive(Debug, Clone)]
pub struct RunContext {
    pub repo_root: PathBuf,
    pub branch: String,
}

#[derive(Debug, Clone)]
pub struct AgentResult {
    pub success: bool,
    pub output: String,
}

pub trait AgentBackend: Send + Sync {
    fn run<'a>(
        &'a self,
        task: &'a Task,
        ctx: &'a RunContext,
    ) -> impl std::future::Future<Output = Result<AgentResult>> + Send + 'a;
}
