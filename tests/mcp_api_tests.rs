// MCP HTTP API integration tests
// Exercises the Axum router from `gitzi::mcp::build_router` via tower oneshot.
// **Validates: MCP protocol correctness, auth enforcement, tool dispatch, scope isolation**

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};
use tower::ServiceExt; // for .oneshot()

use gitzi::agent::build_main_agent;
use gitzi::config::Config;
use gitzi::dispatcher::{
    Dispatcher,
    agent_pool::AgentPool,
    board::{KanbanBoard, WipLimits},
    event_bus::EventBus,
    review_queue::HumanReviewQueue,
};
use gitzi::mcp::auth::TokenStore;

// ── Test helpers ──────────────────────────────────────────────────────────────

/// Build a minimal in-memory Dispatcher and the shared TokenStore, then wrap
/// them in an Axum router via `gitzi::mcp::build_router`.
async fn build_test_app() -> (axum::Router, Arc<TokenStore>) {
    let token_store = Arc::new(TokenStore::new());

    let event_bus = Arc::new(EventBus::new(256));
    let board = Arc::new(RwLock::new(KanbanBoard::from_tasks(vec![])));
    let review_queue = Arc::new(Mutex::new(HumanReviewQueue::new()));
    let config = Arc::new(Config::default());
    let wip_limits = Arc::new(RwLock::new(WipLimits::default()));
    let wip_waiting = Arc::new(Mutex::new(HashMap::new()));

    let agent_pool = AgentPool::inert();

    let main_agent_def = config.resolve_agent("main");
    let main_agent = build_main_agent(&config, &main_agent_def);

    let store: std::sync::Arc<dyn gitzi::state::store::StateStore> =
        std::sync::Arc::new(gitzi::state::store::InMemoryStore::new());

    let dispatcher = Arc::new(Dispatcher {
        event_bus,
        board,
        review_queue,
        agent_pool,
        config,
        wip_limits,
        wip_waiting,
        chat_history: Arc::new(Mutex::new(vec![])),
        main_agent,
        fallback_agent: None,
        token_store: Arc::clone(&token_store),
        store,
        chat_stack: Mutex::new(Vec::new()),
    });

    let router = gitzi::mcp::build_router(Arc::clone(&dispatcher), Arc::clone(&token_store));
    (router, token_store)
}

/// POST /mcp without an Authorization header. Returns (status, parsed JSON body).
async fn post_mcp(app: axum::Router, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// POST /mcp with an `Authorization: Bearer <token>` header.
async fn post_mcp_authed(app: axum::Router, token: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

// ── MCP lifecycle tests ───────────────────────────────────────────────────────

#[tokio::test]
async fn initialize_returns_correct_protocol_version() {
    let (app, _) = build_test_app().await;
    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "0.1.0" }
        }
    });

    let (status, body) = post_mcp(app, req).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(body["result"]["serverInfo"]["name"], "gitzi");
    assert!(body["error"].is_null(), "initialize should not return an error");
}

#[tokio::test]
async fn notifications_initialized_is_accepted() {
    let (app, _) = build_test_app().await;
    let req = json!({
        "jsonrpc": "2.0",
        "id": null,
        "method": "notifications/initialized",
        "params": {}
    });

    let (status, body) = post_mcp(app, req).await;

    assert_eq!(status, StatusCode::OK);
    assert!(body["error"].is_null(), "notification should not produce an error");
}

#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let (app, _) = build_test_app().await;
    let req = json!({
        "jsonrpc": "2.0",
        "id": 99,
        "method": "not/a/real/method",
        "params": {}
    });

    let (status, body) = post_mcp(app, req).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32601, "unknown method should return -32601");
    assert_eq!(body["id"], 99, "response id should match request id");
}

// ── Tool discovery tests ──────────────────────────────────────────────────────

#[tokio::test]
async fn tools_list_returns_all_gitzi_tools() {
    let (app, _) = build_test_app().await;
    let req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });

    let (status, body) = post_mcp(app, req).await;

    assert_eq!(status, StatusCode::OK);
    let tools = body["result"]["tools"]
        .as_array()
        .expect("result.tools should be an array");
    assert_eq!(tools.len(), 8, "should expose exactly 8 gitzi_* tools");

    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for expected in &[
        "gitzi_list_epics",
        "gitzi_list_tasks",
        "gitzi_get_review_item",
        "gitzi_create_task",
        "gitzi_create_review_item",
        "gitzi_update_task",
        "gitzi_park_task",
        "gitzi_block_task",
    ] {
        assert!(names.contains(expected), "missing tool: {expected}");
    }
}

#[tokio::test]
async fn tools_list_entries_have_required_fields() {
    let (app, _) = build_test_app().await;
    let req = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/list",
        "params": {}
    });

    let (status, body) = post_mcp(app, req).await;
    assert_eq!(status, StatusCode::OK);

    let tools = body["result"]["tools"].as_array().unwrap();
    for tool in tools {
        assert!(tool["name"].is_string(), "each tool must have a name");
        assert!(tool["description"].is_string(), "each tool must have a description");
        assert!(tool["inputSchema"].is_object(), "each tool must have an inputSchema");
    }
}

// ── Auth enforcement tests ────────────────────────────────────────────────────

#[tokio::test]
async fn tools_call_without_bearer_returns_401() {
    let (app, _) = build_test_app().await;
    let req = json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/call",
        "params": { "name": "gitzi_list_epics", "arguments": {} }
    });

    let (status, body) = post_mcp(app, req).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], -32001);
}

#[tokio::test]
async fn tools_call_with_malformed_bearer_returns_401() {
    let (app, _) = build_test_app().await;
    let req = json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "tools/call",
        "params": { "name": "gitzi_list_epics", "arguments": {} }
    });

    let (status, body) = post_mcp_authed(app, "this-is-not-a-jwt", req).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], -32001);
}

#[tokio::test]
async fn tools_call_with_wrong_key_returns_401() {
    let (app, _) = build_test_app().await;
    // Token from a different store (different signing key)
    let other_store = TokenStore::new();
    let foreign_token = other_store.issue("task-foreign").await;

    let req = json!({
        "jsonrpc": "2.0",
        "id": 12,
        "method": "tools/call",
        "params": { "name": "gitzi_list_epics", "arguments": {} }
    });

    let (status, body) = post_mcp_authed(app, &foreign_token, req).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], -32001);
}

#[tokio::test]
async fn tools_call_with_revoked_token_returns_401() {
    let (app, store) = build_test_app().await;
    let token = store.issue("task-revoked").await;
    store.revoke(&token).await;

    let req = json!({
        "jsonrpc": "2.0",
        "id": 13,
        "method": "tools/call",
        "params": { "name": "gitzi_list_epics", "arguments": {} }
    });

    let (status, body) = post_mcp_authed(app, &token, req).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["code"], -32001);
}

// ── Tool dispatch tests ───────────────────────────────────────────────────────

#[tokio::test]
async fn tools_call_missing_name_returns_invalid_params() {
    let (app, store) = build_test_app().await;
    let token = store.issue("task-a").await;

    let req = json!({
        "jsonrpc": "2.0",
        "id": 20,
        "method": "tools/call",
        "params": {
            // "name" intentionally absent
            "arguments": {}
        }
    });

    let (status, body) = post_mcp_authed(app, &token, req).await;

    assert_eq!(status, StatusCode::OK, "HTTP status should be 200 for protocol errors");
    assert_eq!(
        body["error"]["code"], -32602,
        "missing 'name' should return invalid params (-32602)"
    );
}

#[tokio::test]
async fn tools_call_unknown_tool_returns_internal_error() {
    let (app, store) = build_test_app().await;
    let token = store.issue("task-a").await;

    let req = json!({
        "jsonrpc": "2.0",
        "id": 21,
        "method": "tools/call",
        "params": {
            "name": "nonexistent_gitzi_tool",
            "arguments": {}
        }
    });

    let (status, body) = post_mcp_authed(app, &token, req).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32603, "unknown tool should return -32603");
    let msg = body["error"]["message"].as_str().unwrap_or("");
    assert!(msg.contains("unknown tool"), "error should mention 'unknown tool', got: {msg}");
}

#[tokio::test]
async fn tools_call_scope_mismatch_on_create_review_item_is_rejected() {
    let (app, store) = build_test_app().await;
    // Token is scoped to task-a but the request targets task-b
    let token = store.issue("task-a").await;

    let req = json!({
        "jsonrpc": "2.0",
        "id": 22,
        "method": "tools/call",
        "params": {
            "name": "gitzi_create_review_item",
            "arguments": {
                "task_id": "task-b",
                "question": "Should we use Redis or Postgres for the queue?"
            }
        }
    });

    let (status, body) = post_mcp_authed(app, &token, req).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32603, "scope mismatch should return -32603");
    let msg = body["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("task-a") || msg.contains("scoped to task"),
        "error should mention scope mismatch, got: {msg}"
    );
}

#[tokio::test]
async fn tools_call_scope_mismatch_on_update_task_is_rejected() {
    let (app, store) = build_test_app().await;
    let token = store.issue("task-a").await;

    let req = json!({
        "jsonrpc": "2.0",
        "id": 23,
        "method": "tools/call",
        "params": {
            "name": "gitzi_update_task",
            "arguments": {
                "task_id": "task-c",
                "title": "New title"
            }
        }
    });

    let (status, body) = post_mcp_authed(app, &token, req).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32603);
    let msg = body["error"]["message"].as_str().unwrap_or("");
    assert!(msg.contains("task-a") || msg.contains("scoped to task"), "{msg}");
}

#[tokio::test]
async fn tools_call_scope_mismatch_on_park_task_is_rejected() {
    let (app, store) = build_test_app().await;
    let token = store.issue("task-a").await;

    let req = json!({
        "jsonrpc": "2.0",
        "id": 24,
        "method": "tools/call",
        "params": {
            "name": "gitzi_park_task",
            "arguments": {
                "task_id": "task-d",
                "reason": "blocked by upstream"
            }
        }
    });

    let (status, body) = post_mcp_authed(app, &token, req).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32603);
}

#[tokio::test]
async fn create_review_item_missing_question_arg_returns_error() {
    let (app, store) = build_test_app().await;
    let token = store.issue("task-a").await;

    let req = json!({
        "jsonrpc": "2.0",
        "id": 30,
        "method": "tools/call",
        "params": {
            "name": "gitzi_create_review_item",
            "arguments": {
                "task_id": "task-a"
                // "question" missing
            }
        }
    });

    let (status, body) = post_mcp_authed(app, &token, req).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32603);
    let msg = body["error"]["message"].as_str().unwrap_or("");
    assert!(msg.contains("question"), "error should mention missing 'question', got: {msg}");
}

#[tokio::test]
async fn create_task_missing_title_returns_error() {
    let (app, store) = build_test_app().await;
    let token = store.issue("task-any").await;

    let req = json!({
        "jsonrpc": "2.0",
        "id": 31,
        "method": "tools/call",
        "params": {
            "name": "gitzi_create_task",
            "arguments": {
                "epic_id": "epic-1"
                // "title" missing
            }
        }
    });

    let (status, body) = post_mcp_authed(app, &token, req).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32603);
    let msg = body["error"]["message"].as_str().unwrap_or("");
    assert!(msg.contains("title"), "error should mention missing 'title', got: {msg}");
}

#[tokio::test]
async fn valid_token_bypasses_auth_and_reaches_dispatcher() {
    // A valid token reaches the dispatcher layer. The list_epics call may fail
    // due to missing disk state (no gitzi session), but the HTTP response is
    // 200 rather than 401 — confirming auth succeeded.
    let (app, store) = build_test_app().await;
    let token = store.issue("task-any").await;

    let req = json!({
        "jsonrpc": "2.0",
        "id": 40,
        "method": "tools/call",
        "params": {
            "name": "gitzi_list_epics",
            "arguments": {}
        }
    });

    let (status, _body) = post_mcp_authed(app, &token, req).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "valid token must not result in 401; dispatcher may return data or an error"
    );
}

// ── JSON-RPC id round-trip ────────────────────────────────────────────────────

#[tokio::test]
async fn response_id_matches_request_id() {
    let (app, _) = build_test_app().await;

    for id in [json!(42), json!("my-req-id"), json!(null)] {
        let req = json!({
            "jsonrpc": "2.0",
            "id": id.clone(),
            "method": "tools/list",
            "params": {}
        });
        let (_, body) = post_mcp(app.clone(), req).await;
        assert_eq!(
            body["id"], id,
            "response id should echo the request id (got {})",
            body
        );
    }
}
