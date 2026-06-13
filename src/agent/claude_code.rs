use serde::Deserialize;
use tokio::process::Command;
use crate::error::{GitziError, Result};
use crate::model::Task;
use super::backend::{AgentBackend, AgentResult, RunContext};

pub struct ClaudeCodeCli {
    /// Passed via `--model` to the `claude` CLI when set.
    pub model: Option<String>,
    /// Replaces the built-in preamble when set. Task details and any
    /// rejection feedback are always appended after.
    pub system_prompt: Option<String>,
}

impl ClaudeCodeCli {
    pub fn new(model: Option<String>, system_prompt: Option<String>) -> Self {
        Self { model, system_prompt }
    }
}

impl Default for ClaudeCodeCli {
    fn default() -> Self { Self::new(None, None) }
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
        let prompt = build_prompt(task, self.system_prompt.as_deref(), ctx.resume_summary.as_deref());
        let mut cmd = Command::new("claude");
        cmd.args(["-p", "--output-format", "json", "--allowedTools", "Bash,Edit,Write,Read,Glob,Grep"]);
        if let Some(model) = &self.model {
            cmd.args(["--model", model]);
        }
        cmd.arg(&prompt)
            .current_dir(&ctx.repo_root)
            .env("GIT_BRANCH", &ctx.branch);
        if let Some(token) = &ctx.mcp_token {
            cmd.env("GITZI_MCP_TOKEN", token);
            cmd.env("GITZI_MCP_SOCKET", crate::state::home::mcp_socket_path().to_string_lossy().as_ref());
        }

        let output = cmd.output().await
            .map_err(|e| GitziError::AgentFailed(format!("failed to spawn claude: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        if let Ok(result) = serde_json::from_str::<ClaudeResult>(stdout.trim()) {
            let output_text = result.result.unwrap_or_default();
            if result.is_error {
                Ok(AgentResult::Failure { output: output_text })
            } else {
                Ok(AgentResult::Success { output: output_text })
            }
        } else {
            let output_text = stdout.to_string();
            if output.status.success() {
                Ok(AgentResult::Success { output: output_text })
            } else {
                Ok(AgentResult::Failure { output: output_text })
            }
        }
    }
}

fn build_prompt(task: &Task, system_prompt: Option<&str>, resume_summary: Option<&str>) -> String {
    let preamble = system_prompt.unwrap_or(
        "You are a disciplined coding agent with one rule above all others: \
        do the smallest change that satisfies the task. Nothing more.\n\n\
        Rules:\n\
        - Implement only what the task explicitly states. No refactoring, no cleanup, \
          no \"while I'm here\" changes.\n\
        - If anything about the task is unclear, ask ONE simple question and stop. \
          Do not guess. Do not fill in gaps.\n\
        - Never attempt to finish quickly. A slow correct step beats a fast wrong one.\n\
        - When done, commit only the files you changed for this task.",
    );

    let mut prompt = format!("{preamble}\n\n");

    prompt.push_str(&format!("Task: {}\n", task.title));

    if let Some(desc) = &task.description {
        prompt.push_str(&format!("\nDescription:\n{desc}\n"));
    }

    if let Some(summary) = resume_summary {
        prompt.push_str(&format!(
            "\nResuming from previous session. Existing work state:\n{summary}\n\
            Review this state before making changes. Continue from where the previous session left off.\n"
        ));
    }

    if let Some(feedback) = &task.agent_feedback {
        prompt.push_str(&format!(
            "\nPrevious attempt was rejected. Feedback from reviewer:\n{feedback}\n\
            Address only the feedback. Do not change anything else.\n"
        ));
    }

    prompt
}
