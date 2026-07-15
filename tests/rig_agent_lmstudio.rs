//! End-to-end check that `RigAgent` actually speaks LM Studio's wire format:
//! `POST /v1/chat/completions` with an OpenAI-compatible body, not the newer
//! Responses API (`/v1/responses`) that most local model servers don't implement.

use axum::{Json, Router, routing::post};
use gitzi::agent::RigAgent;
use gitzi::agent::backend::{AgentBackend, AgentResult, RunContext};
use gitzi::model::Task;
use serde_json::{Value, json};
use std::net::SocketAddr;
use tokio::net::TcpListener;

async fn chat_completions(Json(body): Json<Value>) -> Json<Value> {
    let model = body["model"].as_str().unwrap_or_default().to_string();
    Json(json!({
        "id": "chatcmpl-test",
        "object": "chat.completion",
        "created": 1_700_000_000,
        "model": model,
        "choices": [{
            "index": 0,
            "finish_reason": "stop",
            "logprobs": null,
            "message": { "role": "assistant", "content": "mock lm studio response" }
        }]
    }))
}

async fn spawn_mock_server() -> SocketAddr {
    let app = Router::new().route("/v1/chat/completions", post(chat_completions));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn rig_agent_calls_chat_completions_on_lmstudio_compatible_server() {
    let addr = spawn_mock_server().await;
    let base_url = format!("http://{addr}/v1");

    let agent = RigAgent::new(base_url, "unused-key", "managed-by-provider", None);
    let task = Task::new("task-1", "epic-1", "Do the thing");
    let ctx = RunContext::default();

    let result = agent
        .run(&task, &ctx)
        .await
        .expect("RigAgent should reach the mock server");

    match result {
        AgentResult::Success { output } => {
            assert_eq!(output, "mock lm studio response");
        }
        other => panic!("expected Success, got {other:?}"),
    }
}
