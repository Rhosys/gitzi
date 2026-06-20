use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::AgentDef;
use crate::error::{GitziError, Result};
use crate::state::chat::{ChatMessage, Role};

pub struct MainAgent {
    client: Client,
    base_url: String,
    model: String,
    system_prompt: String,
}

// ── OpenAI-compatible message types ──────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OaiMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<OaiToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OaiToolCall {
    pub id: String,
    pub function: OaiFunctionBody,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OaiFunctionBody {
    pub name: String,
    pub arguments: String, // JSON string
}

// ── Tool definitions ──────────────────────────────────────────────────────────

#[derive(Serialize, Clone)]
pub struct OaiTool {
    pub r#type: &'static str,
    pub function: OaiFunctionDef,
}

#[derive(Serialize, Clone)]
pub struct OaiFunctionDef {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: serde_json::Value,
}

// ── Request / response types ──────────────────────────────────────────────────

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<OaiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OaiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    finish_reason: Option<String>,
    message: AssistantMessage,
}

#[derive(Deserialize)]
struct AssistantMessage {
    role: String,
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OaiToolCall>,
}

// ── Public result types ───────────────────────────────────────────────────────

pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

pub enum ChatTurn {
    Text(String),
    ToolCalls(Vec<ToolCallRequest>),
}

// ── Tool list ─────────────────────────────────────────────────────────────────

fn main_agent_tools() -> Vec<OaiTool> {
    vec![
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_create_epic",
                description: "Create a new epic. An epic is the top-level unit of work containing one or more tasks.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Short title for the epic." },
                        "description": { "type": "string", "description": "Optional detailed description." }
                    },
                    "required": ["title"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_prioritize_task",
                description: "Set the priority of a task. Lower numbers are worked first (1=highest, 100=default).",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "The ID of the task to reprioritize." },
                        "priority": { "type": "integer", "description": "New priority value. Lower is worked first." }
                    },
                    "required": ["task_id", "priority"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_switch_panel",
                description: "Switch the right panel to a different view.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "view": { "type": "string", "enum": ["board", "review"], "description": "The view to switch to." }
                    },
                    "required": ["view"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_create_task",
                description: "Create a new task inside the specified epic.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "epic_id": { "type": "string", "description": "The ID of the epic this task belongs to." },
                        "title": { "type": "string", "description": "Short, imperative title for the task." },
                        "description": { "type": "string", "description": "Optional detailed description." },
                        "priority": { "type": "integer", "description": "Optional initial priority. Defaults to 100." }
                    },
                    "required": ["epic_id", "title"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_update_task",
                description: "Update the title or description of a task.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "The ID of the task to update." },
                        "title": { "type": "string", "description": "New title for the task. Omit to leave unchanged." },
                        "description": { "type": "string", "description": "New description. Omit to leave unchanged." }
                    },
                    "required": ["task_id"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_park_task",
                description: "Park (block) a task, providing a reason why it cannot progress.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "The ID of the task to park." },
                        "reason": { "type": "string", "description": "A clear explanation of why the task is blocked." }
                    },
                    "required": ["task_id", "reason"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_list_epics",
                description: "List all epics in the project.",
                parameters: json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_list_tasks",
                description: "List tasks, optionally filtered to a single epic.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "epic_id": { "type": "string", "description": "If provided, only return tasks belonging to this epic." }
                    },
                    "required": []
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_request_rework",
                description: "Send a task parked in a buffer column back to its previous work column for rework, with feedback for the agent that will redo the work. Only call this once you and the user have explicitly converged on the feedback — you must have restated the user's concern and had them confirm both what they want changed and how strongly they feel about it.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "The ID of the task currently parked in a buffer column." },
                        "feedback": { "type": "string", "description": "Concrete, actionable feedback for the agent that will rework this task." }
                    },
                    "required": ["task_id", "feedback"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_get_review_item",
                description: "Retrieve a single review item by its ID. Returns the question, context, and any actions taken on it.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "The unique ID of the review item to retrieve." }
                    },
                    "required": ["id"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_close_fork",
                description: "Close the current fork session. Call this when the forked topic \
                              is resolved and no further discussion is needed. Only available \
                              inside a fork.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "summary": {
                            "type": "string",
                            "description": "One-sentence summary of what was decided in this fork."
                        }
                    },
                    "required": ["summary"]
                }),
            },
        },
    ]
}

// ── MainAgent ─────────────────────────────────────────────────────────────────

/// Return the tools list appropriate for the current chat context.
/// Only includes `gitzi_close_fork` when we're in a fork and auto-close is enabled.
pub fn tools_for_context(in_fork: bool, fork_auto_close: bool) -> Vec<OaiTool> {
    let mut tools = main_agent_tools();
    if !(in_fork && fork_auto_close) {
        tools.retain(|t| t.function.name != "gitzi_close_fork");
    }
    tools
}

impl MainAgent {
    pub fn new(def: &AgentDef) -> Self {
        Self {
            client: Client::new(),
            base_url: def
                .api_url
                .clone()
                .unwrap_or_else(|| "http://localhost:1234/v1".to_string()),
            model: def.model.clone(),
            system_prompt: def
                .system_prompt
                .clone()
                .unwrap_or_else(default_system_prompt),
        }
    }

    /// The LLM endpoint base URL (for reuse by the classifier).
    pub fn base_url(&self) -> String {
        self.base_url.clone()
    }

    /// The model identifier (for reuse by the classifier).
    pub fn model(&self) -> String {
        self.model.clone()
    }

    /// Convert stored chat history + a new user message into OaiMessage format.
    pub fn history_to_messages(history: &[ChatMessage], user_message: &str) -> Vec<OaiMessage> {
        let mut messages = Vec::new();

        // System prompt is injected by the caller via the first OaiMessage
        // (we don't store it here — MainAgent injects it in turn())

        // Include the last 20 turns to keep context manageable
        let recent = if history.len() > 20 {
            &history[history.len() - 20..]
        } else {
            history
        };

        for msg in recent {
            let role = match msg.role {
                Role::User => "user",
                Role::Agent => "assistant",
                Role::System => continue,
            };
            messages.push(OaiMessage {
                role: role.to_string(),
                content: Some(msg.content.clone()),
                tool_calls: vec![],
                tool_call_id: None,
            });
        }

        messages.push(OaiMessage {
            role: "user".to_string(),
            content: Some(user_message.to_string()),
            tool_calls: vec![],
            tool_call_id: None,
        });

        messages
    }

    /// Single API call with tools. Prepends the system prompt.
    /// Returns (raw assistant OaiMessage, ChatTurn).
    pub async fn turn(
        &self,
        messages: &[OaiMessage],
        tools: &[OaiTool],
    ) -> Result<(OaiMessage, ChatTurn)> {
        // Prepend system message
        let mut full_messages = vec![OaiMessage {
            role: "system".to_string(),
            content: Some(self.system_prompt.clone()),
            tool_calls: vec![],
            tool_call_id: None,
        }];
        full_messages.extend_from_slice(messages);

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let body = ChatRequest {
            model: self.model.clone(),
            messages: full_messages,
            tools: tools.to_vec(),
            temperature: None,
        };

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| GitziError::AgentFailed(format!("LM Studio request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(GitziError::AgentFailed(format!(
                "LM Studio returned {status}: {text}"
            )));
        }

        let parsed: ChatResponse = resp
            .json()
            .await
            .map_err(|e| GitziError::AgentFailed(format!("failed to parse LM Studio response: {e}")))?;

        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| GitziError::AgentFailed("LM Studio returned empty choices".to_string()))?;

        let is_tool_call = choice.finish_reason.as_deref() == Some("tool_calls")
            || !choice.message.tool_calls.is_empty();

        let raw_assistant = OaiMessage {
            role: choice.message.role.clone(),
            content: choice.message.content.clone(),
            tool_calls: choice.message.tool_calls.clone(),
            tool_call_id: None,
        };

        let turn = if is_tool_call {
            let calls = choice
                .message
                .tool_calls
                .into_iter()
                .map(|tc| {
                    let arguments = serde_json::from_str(&tc.function.arguments)
                        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                    ToolCallRequest {
                        id: tc.id,
                        name: tc.function.name,
                        arguments,
                    }
                })
                .collect();
            ChatTurn::ToolCalls(calls)
        } else {
            let text = choice
                .message
                .content
                .unwrap_or_default();
            ChatTurn::Text(text)
        };

        Ok((raw_assistant, turn))
    }

    /// Send a user message and return the model's response (no tool loop — compatibility shim).
    pub async fn chat(&self, history: &[ChatMessage], message: &str) -> Result<String> {
        let messages = Self::history_to_messages(history, message);
        let tools = main_agent_tools();
        let (_raw, turn) = self.turn(&messages, &tools).await?;
        match turn {
            ChatTurn::Text(t) => Ok(t),
            ChatTurn::ToolCalls(_) => Ok(String::new()), // shouldn't happen without loop
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
