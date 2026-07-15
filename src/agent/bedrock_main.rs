//! Bedrock-backed main chat agent using the Converse API with tool support.
//!
//! Implements [`MainChatBackend`] by translating between the dispatcher's
//! OpenAI-shaped message types and Bedrock's Converse API wire format. This
//! keeps the dispatcher tool loop identical regardless of which backend drives
//! the conversation.

use aws_config::BehaviorVersion;
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ConversationRole, ConverseOutput, Message as BedrockMessage, StopReason,
    SystemContentBlock, Tool, ToolConfiguration, ToolInputSchema, ToolResultBlock,
    ToolResultContentBlock, ToolResultStatus, ToolSpecification, ToolUseBlock,
};
use aws_smithy_types::Document;
use tracing::debug;

use super::main_agent::{
    ChatTurn, OaiFunctionBody, OaiMessage, OaiTool, OaiToolCall, ToolCallRequest,
};
use super::main_chat::MainChatBackend;
use crate::aws_sso::SsoCredentialsProvider;
use crate::error::{GitziError, Result};

/// Main chat agent backed by AWS Bedrock Converse API. Authenticates via
/// [`SsoCredentialsProvider`] and translates between the dispatcher's
/// OAI-shaped message format and Bedrock's native Converse types.
pub struct BedrockMainAgent {
    region: String,
    sso_start_url: String,
    sso_account_id: String,
    sso_role_name: String,
    model_id: String,
    system_prompt: String,
}

impl BedrockMainAgent {
    pub fn new(
        region: impl Into<String>,
        sso_start_url: impl Into<String>,
        sso_account_id: impl Into<String>,
        sso_role_name: impl Into<String>,
        model_id: impl Into<String>,
        system_prompt: String,
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

    /// Build an authenticated Bedrock runtime client.
    async fn client(&self) -> aws_sdk_bedrockruntime::Client {
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
        aws_sdk_bedrockruntime::Client::new(&sdk_config)
    }
}

impl MainChatBackend for BedrockMainAgent {
    fn turn<'a>(
        &'a self,
        messages: &'a [OaiMessage],
        tools: &'a [OaiTool],
    ) -> super::main_chat::TurnFuture<'a> {
        Box::pin(self.do_turn(messages, tools))
    }

    fn model(&self) -> String {
        self.model_id.clone()
    }
}

impl BedrockMainAgent {
    async fn do_turn(
        &self,
        messages: &[OaiMessage],
        tools: &[OaiTool],
    ) -> Result<(OaiMessage, ChatTurn)> {
        let client = self.client().await;

        // Convert OAI tools → Bedrock ToolConfiguration
        let tool_config = convert_tools(tools)?;

        // Convert OAI messages → Bedrock messages, extracting system messages
        let bedrock_messages = convert_messages(messages);

        debug!(
            model = %self.model_id,
            message_count = bedrock_messages.len(),
            tool_count = tools.len(),
            "bedrock converse turn"
        );

        let mut system_blocks = vec![SystemContentBlock::Text(self.system_prompt.clone())];

        // Inject any inline system messages from the conversation (e.g. fallback notices)
        for msg in messages {
            if msg.role == "system"
                && let Some(content) = &msg.content
            {
                system_blocks.push(SystemContentBlock::Text(content.clone()));
            }
        }

        let request = client
            .converse()
            .model_id(&self.model_id)
            .set_system(Some(system_blocks))
            .set_messages(Some(bedrock_messages))
            .tool_config(tool_config);

        let response = request
            .send()
            .await
            .map_err(|e| GitziError::AgentFailed(format!("Bedrock Converse failed: {e}")))?;

        // Parse the response
        let output = response
            .output()
            .ok_or_else(|| GitziError::AgentFailed("Bedrock returned no output".into()))?;

        let assistant_message = match output {
            ConverseOutput::Message(msg) => msg,
            _ => {
                return Err(GitziError::AgentFailed(
                    "Bedrock returned unexpected output variant".into(),
                ));
            }
        };

        let stop_reason = response.stop_reason();
        let content_blocks = assistant_message.content();

        // Check if there are tool-use blocks
        let tool_use_blocks: Vec<&ToolUseBlock> = content_blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse(tu) => Some(tu),
                _ => None,
            })
            .collect();

        let has_tool_calls = !tool_use_blocks.is_empty() || *stop_reason == StopReason::ToolUse;

        if has_tool_calls {
            let oai_tool_calls: Vec<OaiToolCall> = tool_use_blocks
                .iter()
                .map(|tu| {
                    let arguments = tu
                        .input()
                        .as_object()
                        .map(|doc| serde_json::to_string(&doc_to_value(doc)).unwrap_or_default())
                        .unwrap_or_else(|| "{}".to_string());
                    OaiToolCall {
                        id: tu.tool_use_id().to_string(),
                        r#type: "function".to_string(),
                        function: OaiFunctionBody {
                            name: tu.name().to_string(),
                            arguments,
                        },
                    }
                })
                .collect();

            let text_content: Option<String> = content_blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .next();

            let raw_assistant = OaiMessage {
                role: "assistant".to_string(),
                content: text_content,
                tool_calls: oai_tool_calls.clone(),
                tool_call_id: None,
            };

            let calls: Vec<ToolCallRequest> = oai_tool_calls
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

            Ok((raw_assistant, ChatTurn::ToolCalls(calls)))
        } else {
            let text = content_blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(t) => Some(t.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");

            let raw_assistant = OaiMessage {
                role: "assistant".to_string(),
                content: Some(text.clone()),
                tool_calls: vec![],
                tool_call_id: None,
            };

            Ok((raw_assistant, ChatTurn::Text(text)))
        }
    }
}

/// Convert OAI tool definitions to Bedrock ToolConfiguration.
fn convert_tools(tools: &[OaiTool]) -> Result<ToolConfiguration> {
    let bedrock_tools: Vec<Tool> = tools
        .iter()
        .map(|t| {
            let json_val = serde_json::to_value(&t.function.parameters)
                .unwrap_or(serde_json::json!({"type": "object"}));
            let input_schema = ToolInputSchema::Json(json_to_document(&json_val));
            Tool::ToolSpec(
                ToolSpecification::builder()
                    .name(t.function.name)
                    .description(t.function.description)
                    .input_schema(input_schema)
                    .build()
                    .expect("tool spec builder should not fail with all fields set"),
            )
        })
        .collect();

    ToolConfiguration::builder()
        .set_tools(Some(bedrock_tools))
        .build()
        .map_err(|e| GitziError::AgentFailed(format!("failed to build tool config: {e}")))
}

/// Convert OAI-formatted messages to Bedrock Converse messages.
/// System messages are skipped (handled separately as system prompt blocks).
/// Tool-result messages become user-role messages with ToolResult content blocks.
fn convert_messages(messages: &[OaiMessage]) -> Vec<BedrockMessage> {
    let mut result = Vec::new();

    for msg in messages {
        match msg.role.as_str() {
            "system" => continue,
            "user" => {
                if let Some(content) = &msg.content {
                    let bedrock_msg = BedrockMessage::builder()
                        .role(ConversationRole::User)
                        .content(ContentBlock::Text(content.clone()))
                        .build()
                        .expect("user message build");
                    result.push(bedrock_msg);
                }
            }
            "assistant" => {
                let mut blocks = Vec::new();
                if let Some(text) = &msg.content
                    && !text.is_empty()
                {
                    blocks.push(ContentBlock::Text(text.clone()));
                }
                for tc in &msg.tool_calls {
                    let input_doc =
                        serde_json::from_str::<serde_json::Value>(&tc.function.arguments)
                            .unwrap_or(serde_json::json!({}));
                    let doc = json_to_document(&input_doc);
                    blocks.push(ContentBlock::ToolUse(
                        ToolUseBlock::builder()
                            .tool_use_id(&tc.id)
                            .name(&tc.function.name)
                            .input(doc)
                            .build()
                            .expect("tool use block build"),
                    ));
                }
                if !blocks.is_empty() {
                    let bedrock_msg = BedrockMessage::builder()
                        .role(ConversationRole::Assistant)
                        .set_content(Some(blocks))
                        .build()
                        .expect("assistant message build");
                    result.push(bedrock_msg);
                }
            }
            "tool" => {
                let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
                let content_text = msg.content.clone().unwrap_or_default();
                let tool_result = ToolResultBlock::builder()
                    .tool_use_id(&tool_call_id)
                    .status(ToolResultStatus::Success)
                    .content(ToolResultContentBlock::Text(content_text))
                    .build()
                    .expect("tool result block build");
                let bedrock_msg = BedrockMessage::builder()
                    .role(ConversationRole::User)
                    .content(ContentBlock::ToolResult(tool_result))
                    .build()
                    .expect("tool result message build");
                result.push(bedrock_msg);
            }
            _ => {}
        }
    }

    result
}

/// Convert a Bedrock Document object map to serde_json::Value.
fn doc_to_value(doc: &std::collections::HashMap<String, Document>) -> serde_json::Value {
    serde_json::Value::Object(
        doc.iter()
            .map(|(k, v)| (k.clone(), bedrock_doc_to_json(v)))
            .collect(),
    )
}

/// Recursively convert a Bedrock Document to serde_json::Value.
fn bedrock_doc_to_json(doc: &Document) -> serde_json::Value {
    match doc {
        Document::String(s) => serde_json::Value::String(s.clone()),
        Document::Number(n) => serde_json::Value::Number(
            serde_json::Number::from_f64(n.to_f64_lossy()).unwrap_or(serde_json::Number::from(0)),
        ),
        Document::Bool(b) => serde_json::Value::Bool(*b),
        Document::Null => serde_json::Value::Null,
        Document::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(bedrock_doc_to_json).collect())
        }
        Document::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), bedrock_doc_to_json(v)))
                .collect(),
        ),
    }
}

/// Recursively convert a serde_json::Value to a Bedrock Document.
fn json_to_document(val: &serde_json::Value) -> Document {
    match val {
        serde_json::Value::Null => Document::Null,
        serde_json::Value::Bool(b) => Document::Bool(*b),
        serde_json::Value::Number(n) => {
            Document::Number(aws_smithy_types::Number::Float(n.as_f64().unwrap_or(0.0)))
        }
        serde_json::Value::String(s) => Document::String(s.clone()),
        serde_json::Value::Array(arr) => {
            Document::Array(arr.iter().map(json_to_document).collect())
        }
        serde_json::Value::Object(map) => Document::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), json_to_document(v)))
                .collect(),
        ),
    }
}
