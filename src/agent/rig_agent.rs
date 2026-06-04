use rig::client::CompletionClient;
use rig::completion::Prompt;
use rig::providers::anthropic;
use crate::error::{GitziError, Result};
use crate::model::Task;
use super::backend::{AgentBackend, AgentResult, RunContext};

pub struct RigAgent {
    api_key: String,
    model: String,
}

impl RigAgent {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: anthropic::completion::CLAUDE_SONNET_4_6.to_string(),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
}

impl AgentBackend for RigAgent {
    fn run<'a>(
        &'a self,
        task: &'a Task,
        _ctx: &'a RunContext,
    ) -> impl std::future::Future<Output = Result<AgentResult>> + Send + 'a {
        async move {
            let client = anthropic::Client::new(&self.api_key)
                .map_err(|e| GitziError::AgentFailed(e.to_string()))?;

            let agent = client
                .agent(&self.model)
                .preamble(
                    "You are a software planning agent. You break down tasks, \
                     analyze requirements, and produce structured output.",
                )
                .build();

            let prompt = format!(
                "Analyze this task and describe how to implement it:\n\nTitle: {}\n{}",
                task.title,
                task.description.as_deref().unwrap_or("")
            );

            let response: String = agent
                .prompt(prompt.as_str())
                .await
                .map_err(|e| GitziError::AgentFailed(e.to_string()))?;

            Ok(AgentResult {
                success: true,
                output: response,
            })
        }
    }
}
