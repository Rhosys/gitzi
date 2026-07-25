//! End-to-end pipeline test with a mocked LLM backend.

use std::collections::HashMap;

use axum::response::{IntoResponse, Response};
use axum::{Json, Router, http::header, routing::post};
use serde_json::json;
use serial_test::serial;
use tempfile::TempDir;

use gitzi::config::{AgentDef, Config, ProviderDef};
use gitzi::dispatcher::{Column, Dispatcher};
use gitzi::state::home;

// ── Mock LLM server ───────────────────────────────────────────────────────────

/// Canned assistant text every mock turn returns.
const MOCK_REPLY: &str = "Task acknowledged. Working on it now.";

/// Mock the chat-completions endpoint. The main chat backend streams
/// (`turn_streaming` sends `stream: true`) while sub-agents use a plain
/// non-streaming call, so respond in the matching shape for each: a
/// Server-Sent Events body for streaming requests, a single JSON object
/// otherwise.
async fn mock_completions(Json(req): Json<serde_json::Value>) -> Response {
    let streaming = req
        .get("stream")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    if streaming {
        let body = format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({ "choices": [{ "delta": { "role": "assistant", "content": MOCK_REPLY } }] }),
            json!({ "choices": [{ "delta": {}, "finish_reason": "stop" }] }),
        );
        return ([(header::CONTENT_TYPE, "text/event-stream")], body).into_response();
    }

    Json(json!({
        "choices": [{
            "finish_reason": "stop",
            "message": {
                "role": "assistant",
                "content": MOCK_REPLY,
                "tool_calls": []
            }
        }]
    }))
    .into_response()
}

/// Respond to /v1/models so the health-check in llm_client works.
async fn mock_models() -> Json<serde_json::Value> {
    Json(json!({
        "data": [{ "id": "mock-model", "object": "model" }]
    }))
}

async fn start_mock_server() -> u16 {
    let app = Router::new()
        .route("/v1/chat/completions", post(mock_completions))
        .route("/v1/models", axum::routing::get(mock_models));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Brief pause for the listener to accept connections
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    port
}

// ── Test config ───────────────────────────────────────────────────────────────

fn test_config(port: u16) -> Config {
    let api_url = format!("http://127.0.0.1:{port}/v1");
    Config {
        providers: HashMap::from([(
            "mock".to_string(),
            ProviderDef {
                api_url: api_url.clone(),
                api_key: String::new(),
                ..ProviderDef::default()
            },
        )]),
        agents: vec![
            AgentDef {
                role: "main".to_string(),
                model: "mock-model".to_string(),
                api_url: Some(api_url.clone()),
                provider: None,
            },
            AgentDef {
                role: "coder".to_string(),
                model: "mock-model".to_string(),
                api_url: Some(api_url.clone()),
                provider: None,
            },
        ],
        ..Config::default()
    }
}

// ── HOME isolation ────────────────────────────────────────────────────────────

/// Override HOME to a fresh tempdir so tests don't pollute `~/.gitzi/`.
/// Returns the TempDir guard — keep it alive for the test duration.
fn with_test_home() -> TempDir {
    let tmp = TempDir::new().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };
    home::ensure_dirs().unwrap();
    tmp
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
#[serial]
async fn chat_with_mock_agent_returns_response() {
    let _tmp = with_test_home();
    let port = start_mock_server().await;
    let config = test_config(port);

    let dispatcher = Dispatcher::start(config).await.unwrap();

    let response = dispatcher
        .chat("Hello, what should we work on?", None)
        .await;
    assert!(
        response.is_ok(),
        "chat should succeed: {:?}",
        response.err()
    );
    let text = response.unwrap();
    assert!(!text.is_empty(), "response should not be empty");
}

#[tokio::test]
#[serial]
async fn create_epic_and_task_via_dispatcher() {
    let _tmp = with_test_home();
    let port = start_mock_server().await;
    let config = test_config(port);

    let dispatcher = Dispatcher::start(config).await.unwrap();

    // Create epic
    let epic = dispatcher
        .gitzi_create_epic(
            "Test Epic".to_string(),
            Some("A test epic for e2e".to_string()),
        )
        .await
        .unwrap();

    assert_eq!(epic.title, "Test Epic");
    assert_eq!(epic.description.as_deref(), Some("A test epic for e2e"));

    // Create task
    let task = dispatcher
        .gitzi_create_task(
            epic.id.clone(),
            "Implement login".to_string(),
            Some("Add OAuth2 login endpoint".to_string()),
            None,
            None,
        )
        .await
        .unwrap();

    assert_eq!(task.title, "Implement login");
    assert_eq!(task.epic, epic.id);

    // Verify task is on the board in Prioritized column
    let board = dispatcher.board.read().await;
    let prioritized = board.tasks_in(Column::Prioritized);
    assert!(
        prioritized.contains(&task.id),
        "task should be in Prioritized column"
    );
}

#[tokio::test]
#[serial]
async fn opening_status_succeeds_with_mock() {
    let _tmp = with_test_home();
    let port = start_mock_server().await;
    let config = test_config(port);

    // start() calls opening_status() internally — reaching here means it worked
    let dispatcher = Dispatcher::start(config).await.unwrap();

    // Also test calling it explicitly
    let status = dispatcher.opening_status().await;
    assert!(status.is_ok());
}
