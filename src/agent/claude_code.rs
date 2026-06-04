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
    let mut prompt = String::from(
        "You are a disciplined coding agent with one rule above all others: \
        do the smallest change that satisfies the task. Nothing more.\n\n\
        Rules:\n\
        - Implement only what the task explicitly states. No refactoring, no cleanup, \
          no \"while I'm here\" changes.\n\
        - If anything about the task is unclear, ask ONE simple question and stop. \
          Do not guess. Do not fill in gaps.\n\
        - Never attempt to finish quickly. A slow correct step beats a fast wrong one.\n\
        - When done, commit only the files you changed for this task.\n\n",
    );

    prompt.push_str(&format!("Task: {}\n", task.title));

    if let Some(desc) = &task.description {
        prompt.push_str(&format!("\nDescription:\n{desc}\n"));
    }

    if let Some(feedback) = &task.agent_feedback {
        prompt.push_str(&format!(
            "\nPrevious attempt was rejected. Feedback from reviewer:\n{feedback}\n\
            Address only the feedback. Do not change anything else.\n"
        ));
    }

    prompt
}
