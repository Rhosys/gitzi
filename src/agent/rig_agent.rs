use rig::client::CompletionClient;
use rig::completion::Prompt;
use rig::providers::anthropic;
use crate::error::{GitziError, Result};
use crate::model::Task;
use super::backend::{AgentBackend, AgentResult, RunContext};

#[allow(dead_code)]
pub struct RigAgent {
    api_key: String,
    model: String,
    system_prompt: Option<String>,
}

#[allow(dead_code)]
impl RigAgent {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: anthropic::completion::CLAUDE_SONNET_4_6.to_string(),
            system_prompt: None,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
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

            let preamble = self.system_prompt.as_deref().unwrap_or(
                "You are a software planning agent. You break down tasks, \
                 analyze requirements, and produce structured output.",
            );

            let agent = client
                .agent(&self.model)
                .preamble(preamble)
                .build();

            let mut prompt = format!(
                "Task: {}\n",
                task.title,
            );
            if let Some(desc) = &task.description {
                prompt.push_str(&format!("\nDescription:\n{desc}\n"));
            }
            if let Some(feedback) = &task.agent_feedback {
                prompt.push_str(&format!(
                    "\nPrevious attempt was rejected. Feedback:\n{feedback}\n"
                ));
            }

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
