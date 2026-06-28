use aws_config::BehaviorVersion;
use rig::client::CompletionClient;
use rig::completion::Prompt;
use rig_bedrock::client::Client;
use crate::aws_sso::SsoCredentialsProvider;
use crate::error::{GitziError, Result};
use crate::model::Task;
use super::backend::{AgentBackend, AgentResult, RunContext};
use super::prompt::{build_task_content, DEFAULT_PREAMBLE};

/// Runs a pipeline agent against AWS Bedrock, authenticating via
/// [`SsoCredentialsProvider`] — credentials are handed to the AWS SDK
/// directly, in-process; gitzi never writes a profile to `~/.aws/config`.
pub struct BedrockAgent {
    region: String,
    sso_start_url: String,
    sso_account_id: String,
    sso_role_name: String,
    model_id: String,
    system_prompt: Option<String>,
}

impl BedrockAgent {
    pub fn new(
        region: impl Into<String>,
        sso_start_url: impl Into<String>,
        sso_account_id: impl Into<String>,
        sso_role_name: impl Into<String>,
        model_id: impl Into<String>,
        system_prompt: Option<String>,
    ) -> Self {
        Self {
            region: region.into(),
            sso_start_url: sso_start_url.into(),
            sso_account_id: sso_account_id.into(),
            sso_role_name: sso_role_name.into(),
            model_id: model_id.into(),
            system_prompt,
        }
    }
}

impl AgentBackend for BedrockAgent {
    async fn run(&self, task: &Task, ctx: &RunContext) -> Result<AgentResult> {
        let credentials_provider = SsoCredentialsProvider::new(
            &self.region,
            &self.sso_start_url,
            &self.sso_account_id,
            &self.sso_role_name,
        );
        let sdk_config = aws_config::defaults(BehaviorVersion::latest())
            .region(aws_config::Region::new(self.region.clone()))
            .credentials_provider(credentials_provider)
            .load()
            .await;
        let aws_client = aws_sdk_bedrockruntime::Client::new(&sdk_config);
        let client = Client::from(aws_client);

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
