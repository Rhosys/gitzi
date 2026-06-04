use serde::Deserialize;
use tokio::process::Command;
use crate::error::{GitziError, Result};
use crate::model::Task;
use super::backend::{AgentBackend, AgentResult, RunContext};

pub struct ClaudeCodeCli;

#[derive(Debug, Deserialize)]
struct ClaudeResult {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    kind: String,
    result: Option<String>,
    #[serde(default)]
    is_error: bool,
}

impl AgentBackend for ClaudeCodeCli {
    fn run<'a>(
        &'a self,
        task: &'a Task,
        ctx: &'a RunContext,
    ) -> impl std::future::Future<Output = Result<AgentResult>> + Send + 'a {
        async move {
            let prompt = build_prompt(task);
            let output = Command::new("claude")
                .args([
                    "-p",
                    "--output-format", "json",
                    "--allowedTools", "Bash,Edit,Write,Read,Glob,Grep",
                    &prompt,
                ])
                .current_dir(&ctx.repo_root)
                .env("GIT_BRANCH", &ctx.branch)
                .output()
                .await
                .map_err(|e| GitziError::AgentFailed(format!("failed to spawn claude: {e}")))?;

            let stdout = String::from_utf8_lossy(&output.stdout);

            if let Ok(result) = serde_json::from_str::<ClaudeResult>(stdout.trim()) {
                Ok(AgentResult {
                    success: !result.is_error,
                    output: result.result.unwrap_or_default(),
                })
            } else {
                Ok(AgentResult {
                    success: output.status.success(),
                    output: stdout.to_string(),
                })
            }
        }
    }
}

fn build_prompt(task: &Task) -> String {
    let mut prompt = format!("You are working on task: {}\n\n", task.title);
    if let Some(desc) = &task.description {
        prompt.push_str(&format!("Description: {desc}\n\n"));
    }
    if let Some(feedback) = &task.agent_feedback {
        prompt.push_str(&format!("Previous attempt was rejected. Feedback: {feedback}\n\n"));
    }
    prompt.push_str(
        "Implement the task. Make small, focused changes. \
        Write tests if applicable. Commit your changes when done.",
    );
    prompt
}
