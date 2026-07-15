use super::backend::{AgentBackend, AgentResult, RunContext};
use super::prompt::{DEFAULT_PREAMBLE, build_task_content};
use crate::error::{GitziError, Result};
use crate::model::Task;
use serde::Deserialize;
use tokio::process::Command;

pub struct ClaudeCodeCli {
    /// Passed via `--model` to the `claude` CLI when set.
    pub model: Option<String>,
    /// Replaces the built-in preamble when set. Task details and any
    /// rejection feedback are always appended after.
    pub system_prompt: Option<String>,
}

impl ClaudeCodeCli {
    pub fn new(model: Option<String>, system_prompt: Option<String>) -> Self {
        Self {
            model,
            system_prompt,
        }
    }
}

impl Default for ClaudeCodeCli {
    fn default() -> Self {
        Self::new(None, None)
    }
}

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
    async fn run(&self, task: &Task, ctx: &RunContext) -> Result<AgentResult> {
        let preamble = self.system_prompt.as_deref().unwrap_or(DEFAULT_PREAMBLE);
        let content =
            build_task_content(task, ctx.resume_summary.as_deref(), &ctx.answered_questions);
        let prompt = format!("{preamble}\n\n{content}");
        let mut cmd = Command::new("claude");
        cmd.args([
            "-p",
            "--output-format",
            "json",
            "--allowedTools",
            "Bash,Edit,Write,Read,Glob,Grep",
        ]);
        if let Some(model) = &self.model {
            cmd.args(["--model", model]);
        }
        cmd.arg(&prompt)
            .current_dir(&ctx.repo_root)
            .env("GIT_BRANCH", &ctx.branch);
        if let Some(token) = &ctx.mcp_token {
            cmd.env("GITZI_MCP_TOKEN", token);
            cmd.env(
                "GITZI_MCP_SOCKET",
                crate::state::home::mcp_socket_path()
                    .to_string_lossy()
                    .as_ref(),
            );
        }

        let output = cmd
            .output()
            .await
            .map_err(|e| GitziError::AgentFailed(format!("failed to spawn claude: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        if let Ok(result) = serde_json::from_str::<ClaudeResult>(stdout.trim()) {
            let output_text = result.result.unwrap_or_default();
            if result.is_error {
                Ok(AgentResult::Failure {
                    output: output_text,
                })
            } else {
                Ok(AgentResult::Success {
                    output: output_text,
                })
            }
        } else {
            let output_text = stdout.to_string();
            if output.status.success() {
                Ok(AgentResult::Success {
                    output: output_text,
                })
            } else {
                Ok(AgentResult::Failure {
                    output: output_text,
                })
            }
        }
    }
}
