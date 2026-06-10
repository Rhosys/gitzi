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
        let prompt = build_prompt(task, self.system_prompt.as_deref());
        let mut cmd = Command::new("claude");
        cmd.args(["-p", "--output-format", "json", "--allowedTools", "Bash,Edit,Write,Read,Glob,Grep"]);
        if let Some(model) = &self.model {
            cmd.args(["--model", model]);
        }
        cmd.arg(&prompt)
            .current_dir(&ctx.repo_root)
            .env("GIT_BRANCH", &ctx.branch);

        let output = cmd.output().await
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

fn build_prompt(task: &Task, system_prompt: Option<&str>) -> String {
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

    if let Some(feedback) = &task.agent_feedback {
        prompt.push_str(&format!(
            "\nPrevious attempt was rejected. Feedback from reviewer:\n{feedback}\n\
            Address only the feedback. Do not change anything else.\n"
        ));
    }

    prompt
}
