use super::backend::{AgentBackend, AgentResult, RunContext};
use super::prompt::{DEFAULT_PREAMBLE, build_task_content};
use crate::error::{GitziError, Result};
use crate::model::Task;
use rig::client::CompletionClient;
use rig::completion::Prompt;
use rig::providers::openai;

/// Runs a pipeline agent against an OpenAI-compatible chat-completions endpoint
/// (LM Studio, or anything else speaking the same wire format) via `rig`.
pub struct RigAgent {
    base_url: String,
    api_key: String,
    model: String,
    system_prompt: Option<String>,
}

impl RigAgent {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        system_prompt: Option<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            system_prompt,
        }
    }
}

impl AgentBackend for RigAgent {
    async fn run(&self, task: &Task, ctx: &RunContext) -> Result<AgentResult> {
        // LM Studio (and most other local model servers) only implement the
        // traditional Chat Completions wire format, not OpenAI's newer Responses
        // API — `openai::Client`'s default extension. Use `CompletionsClient`
        // to target `/chat/completions` instead of `/responses`.
        let client = openai::CompletionsClient::builder()
            .api_key(&self.api_key)
            .base_url(&self.base_url)
            .build()
            .map_err(|e| GitziError::AgentFailed(e.to_string()))?;

        let agent = client
            .agent(&self.model)
            .preamble(self.system_prompt.as_deref().unwrap_or(DEFAULT_PREAMBLE))
            .build();

        let prompt =
            build_task_content(task, ctx.resume_summary.as_deref(), &ctx.answered_questions);

        let response: String = agent
            .prompt(prompt.as_str())
            .await
            .map_err(|e| GitziError::AgentFailed(e.to_string()))?;

        Ok(AgentResult::Success { output: response })
    }
}
