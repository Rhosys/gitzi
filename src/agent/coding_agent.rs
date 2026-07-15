//! Coding agent — runs in a worktree, calls LLM in a tool loop until done.

use reqwest::Client;
use std::path::Path;
use tracing::{info, warn};

use crate::agent::coding_tools::{self, ToolResult, coding_agent_tools};
use crate::agent::llm_client;
use crate::agent::main_agent::{OaiMessage, OaiToolCall};
use crate::config::AgentDef;
use crate::error::{GitziError, Result};

/// Maximum number of tool-call rounds before we force-stop.
const MAX_ROUNDS: usize = 50;

/// Run the coding agent loop for a task.
/// Returns the final text response from the agent (or an error).
pub async fn run(agent_def: &AgentDef, task_prompt: &str, worktree_root: &Path) -> Result<String> {
    let client = Client::new();
    let base_url = agent_def
        .api_url
        .as_deref()
        .unwrap_or("http://localhost:1234/v1");
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let model = &agent_def.model;
    let system_prompt = "\
You are a coding agent. Implement the task described below.\n\n\
RULES:\n\
- Use the provided tools to read, write, and edit files.\n\
- Make the smallest correct change that satisfies the task.\n\
- ALWAYS run tests and lint after making changes.\n\
- If tests or lint fail, fix the problems immediately. Do not stop until they pass.\n\
- Only report done (respond with text, no tool calls) when tests and lint pass.\n\
- If you cannot fix a failure after 3 attempts, raise a clarification item.\n\
- Never refactor code you were not asked to change.\n\
- Never guess about intent — if unclear, use gitzi_create_review_item to ask.";

    let tools = coding_agent_tools();

    let mut messages: Vec<OaiMessage> = vec![
        OaiMessage {
            role: "system".to_string(),
            content: Some(system_prompt.to_string()),
            tool_calls: vec![],
            tool_call_id: None,
        },
        OaiMessage {
            role: "user".to_string(),
            content: Some(task_prompt.to_string()),
            tool_calls: vec![],
            tool_call_id: None,
        },
    ];

    for round in 0..MAX_ROUNDS {
        let body = serde_json::json!({
            "model": model,
            "messages": messages,
            "tools": tools,
        });

        let parsed: LlmResponse = llm_client::post_with_retry(&client, &url, &body)
            .await
            .map_err(|e| {
                GitziError::AgentFailed(format!("LLM call failed (round {round}): {e}"))
            })?;

        let choice = parsed
            .choices
            .first()
            .ok_or_else(|| GitziError::AgentFailed("no choices in response".to_string()))?;

        // If the model returned text (no tool calls), we're done
        if choice.message.tool_calls.is_empty() {
            let text = choice.message.content.clone().unwrap_or_default();
            info!(round, "coding agent finished with text response");
            return Ok(text);
        }

        // Add the assistant message (with tool_calls) to conversation
        messages.push(OaiMessage {
            role: "assistant".to_string(),
            content: choice.message.content.clone(),
            tool_calls: choice.message.tool_calls.clone(),
            tool_call_id: None,
        });

        // Execute each tool call
        for call in &choice.message.tool_calls {
            let args: serde_json::Value =
                serde_json::from_str(&call.function.arguments).unwrap_or_default();

            let result = coding_tools::execute_tool(&call.function.name, &args, worktree_root);

            let output = match result {
                ToolResult::Output(s) => s,
                ToolResult::DelegateToDispatcher => {
                    // For now, return a message saying the tool was acknowledged.
                    // Full dispatcher integration happens via the MCP path.
                    format!("ok: {} dispatched", call.function.name)
                }
                ToolResult::UnknownTool(name) => {
                    warn!(tool = %name, "agent called unknown tool");
                    format!(
                        "error: tool '{}' does not exist. \
                         A review item has been created to consider adding it.",
                        name,
                    )
                }
            };

            messages.push(OaiMessage {
                role: "tool".to_string(),
                content: Some(output),
                tool_calls: vec![],
                tool_call_id: Some(call.id.clone()),
            });
        }
    }

    Err(GitziError::AgentFailed(format!(
        "coding agent hit {MAX_ROUNDS} rounds without completing"
    )))
}

// ── Response types for deserialization ────────────────────────────────────────

#[derive(serde::Deserialize)]
struct LlmResponse {
    choices: Vec<LlmChoice>,
}

#[derive(serde::Deserialize)]
struct LlmChoice {
    message: LlmMessage,
}

#[derive(serde::Deserialize)]
struct LlmMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OaiToolCall>,
}
