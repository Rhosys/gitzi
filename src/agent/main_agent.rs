use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::config::AgentDef;
use crate::error::{GitziError, Result};
use crate::state::chat::{ChatMessage, Role};

pub struct MainAgent {
    client: Client,
    base_url: String,
    model: String,
    system_prompt: String,
}

// ── OpenAI-compatible request/response types ──────────────────────────────────

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Serialize, Deserialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

// ── MainAgent ─────────────────────────────────────────────────────────────────

impl MainAgent {
    pub fn new(def: &AgentDef) -> Self {
        Self {
            client: Client::new(),
            base_url: def
                .base_url
                .clone()
                .unwrap_or_else(|| "http://localhost:1234/v1".to_string()),
            model: def.model.clone(),
            system_prompt: def
                .system_prompt
                .clone()
                .unwrap_or_else(default_system_prompt),
        }
    }

    /// Send a user message and return the model's response.
    /// History contains prior User + Agent turns; System messages are skipped.
    pub async fn chat(&self, history: &[ChatMessage], message: &str) -> Result<String> {
        let mut messages = vec![Message {
            role: "system".to_string(),
            content: self.system_prompt.clone(),
        }];

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
            messages.push(Message {
                role: role.to_string(),
                content: msg.content.clone(),
            });
        }

        messages.push(Message {
            role: "user".to_string(),
            content: message.to_string(),
        });

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let body = ChatRequest {
            model: self.model.clone(),
            messages,
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

        parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| GitziError::AgentFailed("LM Studio returned empty choices".to_string()))
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
