use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::AgentDef;
use crate::error::{GitziError, Result};
use crate::state::chat::{ChatMessage, Role};
use super::main_chat::MainChatBackend;

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
    #[serde(default = "default_tool_type")]
    pub r#type: String,
    pub function: OaiFunctionBody,
}

fn default_tool_type() -> String {
    "function".to_string()
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
                        "view": { "type": "string", "enum": ["status", "epic", "kanban", "task", "logs"], "description": "The view to switch to." }
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
                        "priority": { "type": "integer", "description": "Optional initial priority. Defaults to 100." },
                        "repo": { "type": "string", "description": "Filesystem path of the repository this task operates in. Use gitzi_list_repos to find available repos." }
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
                description: "Send a task parked in a buffer column back to a work column for rework, with feedback for the agent that will redo the work. Only call this once you and the user have explicitly converged on the feedback — you must have restated the user's concern and had them confirm both what they want changed and how strongly they feel about it. Use target_column to send to a specific column (e.g. 'coding' from review-buffer when the review is valid and coder needs to fix issues).",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "The ID of the task currently parked in a buffer column." },
                        "feedback": { "type": "string", "description": "Concrete, actionable feedback for the agent that will rework this task." },
                        "target_column": { "type": "string", "description": "Optional. The kebab-case column name to send the task to (e.g. 'coding', 'reviewing', 'designing'). If omitted, sends back one column (the immediate predecessor)." }
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
                name: "gitzi_list_repos",
                description: "List all discovered repositories with their paths, summaries, and labels.",
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
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_rediscover_providers",
                description: "Quickly re-scan this machine for LLM providers (LM Studio, Ollama) \
                              and AWS Bedrock/SSO access, merging any newly-found ones into \
                              config.toml as disabled candidates. Use this when the user asks what \
                              model providers are available, or after they've installed/started \
                              something new. Returns every known provider with its status so you \
                              can present the list and ask which one(s) to activate with \
                              gitzi_activate_provider.",
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
                name: "gitzi_activate_provider",
                description: "Activate a discovered provider so agents actually use it. For \
                              OpenAI-compatible providers (LM Studio, Ollama) this is immediate. \
                              For Bedrock providers, this may take multiple calls: the first call \
                              kicks off (or resumes) an AWS SSO browser login and returns a \
                              verification code; once the user confirms login in the browser, call \
                              again to get the list of AWS accounts (pass account_id once chosen), \
                              then the list of roles in that account (pass role_name once chosen) — \
                              the final call with both account_id and role_name validates the \
                              credentials and activates the provider. A gitzi restart is required \
                              for the newly-wired agent to take effect; always tell the user this.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "The provider's name, as shown by gitzi_rediscover_providers." },
                        "account_id": { "type": "string", "description": "Bedrock only. AWS account ID chosen from the list returned by a previous call." },
                        "role_name": { "type": "string", "description": "Bedrock only. AWS SSO role/permission-set name chosen from the list returned by a previous call." }
                    },
                    "required": ["name"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_search_kb",
                description: "Search the gitzi knowledge base to answer questions about \
                              how gitzi works, its configuration, features, and behavior. \
                              Use this when the user asks about gitzi itself.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search term to look up in the knowledge base."
                        }
                    },
                    "required": ["query"]
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
            system_prompt: default_system_prompt(),
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

        // Resolve model: use configured value, or query the server for whatever's loaded
        let model = if self.model.is_empty() {
            query_loaded_model_async(&self.client, &self.base_url).await
                .ok_or_else(|| GitziError::AgentFailed(
                    format!("no model configured and none loaded at {}", self.base_url)
                ))?
        } else {
            self.model.clone()
        };

        let body = ChatRequest {
            model,
            messages: full_messages,
            tools: tools.to_vec(),
            temperature: None,
        };

        let parsed: ChatResponse =
            crate::agent::llm_client::post_with_retry(&self.client, &url, &body)
                .await
                .map_err(|e| GitziError::AgentFailed(format!("LLM call failed: {e}")))?;

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
            ChatTurn::Text(strip_special_tokens(&text))
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

impl MainChatBackend for MainAgent {
    fn turn<'a>(
        &'a self,
        messages: &'a [OaiMessage],
        tools: &'a [OaiTool],
    ) -> super::main_chat::TurnFuture<'a> {
        Box::pin(self.turn(messages, tools))
    }

    fn model(&self) -> String {
        self.model.clone()
    }

    fn base_url(&self) -> Option<String> {
        Some(self.base_url.clone())
    }
}

/// Strip model-specific special tokens that local LLMs sometimes leak into
/// their output text (BOS/EOS markers, thinking blocks).
fn strip_special_tokens(text: &str) -> String {
    let mut result = text.to_string();
    // BOS/EOS tokens from various model families
    for token in &[
        "<|begin_of_sentence|>",
        "<|end_of_sentence|>",
        "<|begin▁of▁sentence|>",
        "<|end▁of▁sentence|>",
        "<s>",
        "</s>",
        "<|im_start|>assistant",
        "<|im_end|>",
    ] {
        result = result.replace(token, "");
    }
    // Strip <think>...</think> blocks (Qwen3 reasoning traces)
    while let Some(start) = result.find("<think>") {
        if let Some(end) = result.find("</think>") {
            result = format!("{}{}", &result[..start], &result[end + 8..]);
        } else {
            // Unclosed think tag — strip from <think> to end
            result = result[..start].to_string();
            break;
        }
    }
    result.trim().to_string()
}

/// Async version of model query — queries GET /v1/models and returns the first
/// loaded model ID. Safe to call from inside tokio (unlike the blocking version
/// in bootstrap.rs which panics in async context).
async fn query_loaded_model_async(client: &Client, base_url: &str) -> Option<String> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("data")
        .and_then(|d| d.as_array())
        .and_then(|arr| arr.first())
        .and_then(|m| m.get("id"))
        .and_then(|id| id.as_str())
        .map(str::to_string)
}

pub fn default_system_prompt() -> String {
    "\
You are gitzi's main coordination agent. You help the user manage their software \
project through conversation. You have tools — USE THEM. Never guess at state you \
can look up.\n\n\
GITZI CONTEXT:\n\
gitzi is an agile SDLC harness. It has a Kanban board with columns: \
Prioritized → Designing → CodingBuffer → Coding → ReviewBuffer → Reviewing → \
SecurityAuditBuffer → Auditing → DeploymentBuffer → Deploying → Done. \
Buffer columns require human approval to advance. Work columns have AI agents \
assigned. You coordinate the human's interaction with this pipeline.\n\n\
TOOL USAGE — MANDATORY:\n\
You have tools for managing epics, tasks, and the pipeline. ALWAYS call the \
appropriate tool rather than asking the user for information you can look up:\n\
- gitzi_list_epics — list all epics\n\
- gitzi_list_tasks — list tasks (optionally filtered by epic)\n\
- gitzi_create_epic — create a new epic\n\
- gitzi_create_task — create a task in an epic\n\
- gitzi_update_task — update title or description\n\
- gitzi_prioritize_task — set task priority\n\
- gitzi_park_task — block a task with a reason\n\
- gitzi_request_rework — send a buffer-parked task back for rework\n\
- gitzi_get_review_item — get details of a review item\n\
- gitzi_list_repos — list discovered repositories\n\
- gitzi_switch_panel — change the TUI right panel view\n\
- gitzi_close_fork — close a fork session (only in forks)\n\
- gitzi_rediscover_providers — rescan for LLM providers\n\
- gitzi_activate_provider — activate a discovered provider\n\
- gitzi_search_kb — search the gitzi knowledge base\n\n\
If the user asks about tasks, epics, or board state — call the tool first, \
then answer from the result. NEVER say \"I don't have that information\" when \
a tool exists to fetch it.\n\n\
CORE RULES — NEVER VIOLATE:\n\
- One question per response. Never two. Never more.\n\
- Never ask either/or questions. Never append \"or something else?\"\n\
- When presenting options, assign a number to each. Never use \"or\" between them.\n\
- Fewer words are always better. Be concise. Terminal width is limited.\n\
- Never write code directly — coding agents handle implementation.\n\
- Never dump information. Optimize for conversation, not completeness.\n\n\
EPIC CREATION FLOW:\n\
When the user wants to build something:\n\
1. Suggest 3-5 things that could be part of the scope (short bullet list).\n\
2. Ask one question about priorities, constraints, or scope.\n\
3. Continue asking questions — expect 10-20 before the epic is fully understood.\n\
4. Only call gitzi_create_epic when you have enough clarity. Never rush it.\n\n\
TASK CREATION FLOW:\n\
After the epic exists:\n\
1. Suggest 5 task titles (title only, no descriptions yet).\n\
2. Let the user give feedback, add, remove, reorder.\n\
3. Go back and forth. This is a conversation, not a proposal.\n\
4. Only call gitzi_create_task after the user confirms each task.\n\n\
WHAT YOU SURFACE:\n\
- Tasks needing approval (buffer columns)\n\
- Blocked agents with questions\n\
- Pipeline progress\n\
- One thing at a time. Always."
        .to_string()
}
