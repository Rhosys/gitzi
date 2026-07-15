//! End-to-end test for the Bedrock provider flow.
//!
//! Verifies:
//! 1. SSO credentials provider resolves (using cached credentials)
//! 2. MainChatBackend::turn() succeeds with a text response
//! 3. Tool calls are correctly parsed when the model returns them
//!
//! Requires: valid AWS SSO session cached in the OS keyring (run `gitzi` once
//! interactively first). Marked `#[ignore]` — run explicitly with:
//!   cargo test bedrock_e2e -- --ignored

use gitzi::agent::bedrock_main::BedrockMainAgent;
use gitzi::agent::main_agent::{ChatTurn, OaiFunctionDef, OaiMessage, OaiTool};
use gitzi::agent::main_chat::MainChatBackend;

/// Build a BedrockMainAgent with the rhosys account configuration.
fn build_bedrock_agent() -> BedrockMainAgent {
    BedrockMainAgent::new(
        "us-east-1",
        "https://d-9a672c0294.awsapps.com/start",
        "REDACTED",
        "AdministratorAccess",
        "us.anthropic.claude-sonnet-4-20250514-v1:0",
        "You are a test assistant. Respond concisely.".to_string(),
    )
}

/// Minimal tool definition for testing tool-call parsing.
fn echo_tool() -> Vec<OaiTool> {
    vec![OaiTool {
        r#type: "function",
        function: OaiFunctionDef {
            name: "echo",
            description: "Echoes the input back to the user",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "description": "Text to echo"
                    }
                },
                "required": ["text"]
            }),
        },
    }]
}

#[tokio::test]
#[ignore]
async fn bedrock_basic_text_response() {
    let agent = build_bedrock_agent();
    let messages = vec![OaiMessage {
        role: "user".to_string(),
        content: Some("Say hello in exactly 3 words.".to_string()),
        tool_calls: vec![],
        tool_call_id: None,
    }];

    let (_raw, turn) = agent
        .turn(&messages, &[])
        .await
        .expect("bedrock turn should succeed with cached SSO credentials");

    match turn {
        ChatTurn::Text(text) => {
            assert!(!text.is_empty(), "response should not be empty");
            println!("Bedrock response: {text}");
        }
        ChatTurn::ToolCalls(_) => {
            panic!("expected text response, got tool calls");
        }
    }
}

#[tokio::test]
#[ignore]
async fn bedrock_tool_call_response() {
    let agent = build_bedrock_agent();
    let messages = vec![OaiMessage {
        role: "user".to_string(),
        content: Some(
            "Use the echo tool to echo 'hello world'. \
             Do not respond with text, only call the tool."
                .to_string(),
        ),
        tool_calls: vec![],
        tool_call_id: None,
    }];

    let tools = echo_tool();
    let (_raw, turn) = agent
        .turn(&messages, &tools)
        .await
        .expect("bedrock turn should succeed");

    match turn {
        ChatTurn::ToolCalls(calls) => {
            assert!(!calls.is_empty(), "should have at least one tool call");
            let call = &calls[0];
            assert_eq!(call.name, "echo");
            let text = call.arguments.get("text").and_then(|v| v.as_str());
            assert!(text.is_some(), "echo tool should receive a 'text' argument");
            println!("Tool call: echo({:?})", text.unwrap());
        }
        ChatTurn::Text(text) => {
            panic!("expected tool call, got text: {text}");
        }
    }
}

#[tokio::test]
#[ignore]
async fn bedrock_multi_turn_with_tool_result() {
    let agent = build_bedrock_agent();
    let tools = echo_tool();

    // Turn 1: ask for tool call
    let messages = vec![OaiMessage {
        role: "user".to_string(),
        content: Some(
            "Call the echo tool with 'test'. Then after I give you the result, \
             summarize what happened."
                .to_string(),
        ),
        tool_calls: vec![],
        tool_call_id: None,
    }];

    let (raw_assistant, turn) = agent
        .turn(&messages, &tools)
        .await
        .expect("first turn should succeed");

    let calls = match turn {
        ChatTurn::ToolCalls(calls) => calls,
        ChatTurn::Text(t) => panic!("expected tool call in turn 1, got: {t}"),
    };

    // Turn 2: provide tool result
    let mut messages2 = messages.clone();
    messages2.push(raw_assistant);
    messages2.push(OaiMessage {
        role: "tool".to_string(),
        content: Some("test".to_string()),
        tool_calls: vec![],
        tool_call_id: Some(calls[0].id.clone()),
    });

    let (_raw2, turn2) = agent
        .turn(&messages2, &tools)
        .await
        .expect("second turn should succeed");

    match turn2 {
        ChatTurn::Text(text) => {
            assert!(!text.is_empty(), "summary should not be empty");
            println!("Multi-turn response: {text}");
        }
        ChatTurn::ToolCalls(_) => {
            // Acceptable — model may chain another tool call
            println!("Model chained another tool call (acceptable)");
        }
    }
}
