use serde::Deserialize;
use tokio::process::Command;

use crate::config::AgentDef;
use crate::error::{GitziError, Result};
use crate::state::chat::{ChatMessage, Role};

pub struct MainAgent {
    model: Option<String>,
    system_prompt: String,
}

#[derive(Debug, Deserialize)]
struct ClaudeOutput {
    #[serde(rename = "type")]
    #[allow(dead_code)]
    kind: String,
    result: Option<String>,
    #[serde(default)]
    is_error: bool,
}

impl MainAgent {
    pub fn new(def: &AgentDef) -> Self {
        Self {
            model: Some(def.model.clone()),
            system_prompt: def.system_prompt.clone().unwrap_or_else(default_system_prompt),
        }
    }

    /// Send a message and return the agent's response.
    /// `history` contains prior turns (User + Agent messages only; System is skipped).
    pub async fn chat(&self, history: &[ChatMessage], message: &str) -> Result<String> {
        let prompt = build_prompt(&self.system_prompt, history, message);

        let mut cmd = Command::new("claude");
        cmd.args(["-p", "--output-format", "json"]);
        if let Some(model) = &self.model {
            cmd.args(["--model", model]);
        }
        cmd.arg(&prompt);

        let output = cmd
            .output()
            .await
            .map_err(|e| GitziError::AgentFailed(format!("failed to spawn claude: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        if let Ok(parsed) = serde_json::from_str::<ClaudeOutput>(stdout.trim()) {
            if parsed.is_error {
                return Err(GitziError::AgentFailed(
                    parsed.result.unwrap_or_else(|| "unknown error".to_string()),
                ));
            }
            Ok(parsed.result.unwrap_or_default())
        } else if output.status.success() {
            Ok(stdout.trim().to_string())
        } else {
            Err(GitziError::AgentFailed(stdout.trim().to_string()))
        }
    }
}

fn default_system_prompt() -> String {
    "\
You are the main coordination agent for gitzi, an AI-driven software development pipeline. \
You help the user manage their project through natural conversation.\n\n\
Your responsibilities:\n\
- Understand the user's intent and translate it into project actions\n\
- Create and refine epics, tasks, and work items\n\
- Surface what needs the user's attention: approvals, blocked agents, open questions\n\
- Keep work moving through the pipeline\n\n\
Rules:\n\
- Never write code directly — coding agents do that\n\
- When anything is unclear, ask one question and stop\n\
- Never batch multiple questions\n\
- Be concise: the user reads in a terminal"
        .to_string()
}

fn build_prompt(system_prompt: &str, history: &[ChatMessage], message: &str) -> String {
    let mut prompt = format!("{system_prompt}\n\n");

    // Include the last 20 turns so context stays manageable
    let recent = if history.len() > 20 {
        &history[history.len() - 20..]
    } else {
        history
    };

    for msg in recent {
        match msg.role {
            Role::User => prompt.push_str(&format!("User: {}\n\n", msg.content)),
            Role::Agent => prompt.push_str(&format!("Assistant: {}\n\n", msg.content)),
            Role::System => {} // system messages are context, not turns
        }
    }

    prompt.push_str(&format!("User: {message}\n\nAssistant:"));
    prompt
}
