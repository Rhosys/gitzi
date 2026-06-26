use rig::client::CompletionClient;
use rig::completion::Prompt;
use rig_bedrock::client::Client;
use crate::error::{GitziError, Result};
use crate::model::Task;
use super::backend::{AgentBackend, AgentResult, RunContext};
use super::prompt::{build_task_content, DEFAULT_PREAMBLE};

/// Runs a pipeline agent against AWS Bedrock via `rig-bedrock`, authenticating
/// through a named AWS profile in `~/.aws/config` whose `credential_process`
/// resolves to `gitzi creds-helper aws` — see `crate::aws_sso`. The profile's
/// `region` line (also written by gitzi) controls which Bedrock region is used.
pub struct BedrockAgent {
    profile_name: String,
    model_id: String,
    system_prompt: Option<String>,
}

impl BedrockAgent {
    pub fn new(
        profile_name: impl Into<String>,
        model_id: impl Into<String>,
        system_prompt: Option<String>,
    ) -> Self {
        Self {
            profile_name: profile_name.into(),
            model_id: model_id.into(),
            system_prompt,
        }
    }
}

impl AgentBackend for BedrockAgent {
    async fn run(&self, task: &Task, ctx: &RunContext) -> Result<AgentResult> {
        let client = Client::with_profile_name(&self.profile_name);

        let agent = client
            .agent(&self.model_id)
            .preamble(self.system_prompt.as_deref().unwrap_or(DEFAULT_PREAMBLE))
            .build();

        let prompt = build_task_content(task, ctx.resume_summary.as_deref(), &ctx.answered_questions);

        let response: String = agent
            .prompt(prompt.as_str())
            .await
            .map_err(|e| GitziError::AgentFailed(e.to_string()))?;

        Ok(AgentResult::Success { output: response })
    }
}
