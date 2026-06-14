pub mod auth;
pub mod protocol;
pub mod tools;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde_json::{json, Value};
use tracing::info;

use auth::TokenStore;
use protocol::{JsonRpcRequest, JsonRpcResponse, sub_agent_tools};
use crate::dispatcher::Dispatcher;

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct McpState {
    dispatcher: Arc<Dispatcher>,
    token_store: Arc<TokenStore>,
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Build the Axum router for the MCP HTTP API.
///
/// Exposed for integration testing so tests can call `router.oneshot(...)` without
/// binding a real Unix socket.
pub fn build_router(dispatcher: Arc<Dispatcher>, token_store: Arc<TokenStore>) -> Router {
    let state = McpState { dispatcher, token_store };
    Router::new()
        .route("/mcp", post(handle_mcp))
        .with_state(state)
}

/// Bind a Unix-domain socket at `~/.gitzi/mcp.sock` and serve the MCP HTTP
/// API on it.  Stale socket files are removed before binding so that a crashed
/// process does not prevent restart.
pub async fn serve(
    dispatcher: Arc<Dispatcher>,
    token_store: Arc<TokenStore>,
) -> anyhow::Result<()> {
    let socket_path = crate::state::home::mcp_socket_path();

    // Remove a stale socket from a previous run.
    let _ = std::fs::remove_file(&socket_path);

    // Ensure the parent directory exists.
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let app = build_router(dispatcher, token_store);
    let listener = tokio::net::UnixListener::bind(&socket_path)?;

    info!(socket = %socket_path.display(), "MCP server listening");

    axum::serve(listener, app).await?;

    Ok(())
}

// ── Handler ───────────────────────────────────────────────────────────────────

/// Handle a single JSON-RPC 2.0 request posted to `/mcp`.
async fn handle_mcp(
    State(state): State<McpState>,
    headers: HeaderMap,
    Json(req): Json<JsonRpcRequest>,
) -> (StatusCode, Json<JsonRpcResponse>) {
    let id = req.id.clone();

    let response = match req.method.as_str() {
        // ── MCP lifecycle ─────────────────────────────────────────────────────
        "initialize" => {
            JsonRpcResponse::ok(
                id,
                json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "gitzi",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            )
        }

        "notifications/initialized" => {
            // Fire-and-forget notification — no meaningful response body.
            JsonRpcResponse::ok(id, json!({}))
        }

        // ── Tool discovery ────────────────────────────────────────────────────
        "tools/list" => {
            JsonRpcResponse::ok(id, json!({ "tools": sub_agent_tools() }))
        }

        // ── Tool invocation ───────────────────────────────────────────────────
        "tools/call" => {
            // Require a valid Bearer token for all write operations.
            let token = match extract_bearer(&headers) {
                Some(t) => t,
                None => {
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(JsonRpcResponse::err(
                            id,
                            -32001,
                            "missing Authorization: Bearer <token> header",
                        )),
                    );
                }
            };

            let entry = match state.token_store.validate(&token).await {
                Some(e) => e,
                None => {
                    return (
                        StatusCode::UNAUTHORIZED,
                        Json(JsonRpcResponse::err(id, -32001, "invalid or expired token")),
                    );
                }
            };

            let tool_name = match req.params.get("name").and_then(Value::as_str) {
                Some(n) => n.to_string(),
                None => {
                    return (
                        StatusCode::OK,
                        Json(JsonRpcResponse::err(
                            id,
                            -32602,
                            "missing required param: name",
                        )),
                    );
                }
            };

            let arguments = req
                .params
                .get("arguments")
                .cloned()
                .unwrap_or(json!({}));

            match tools::dispatch(&tool_name, &arguments, &entry, &state.dispatcher).await {
                Ok(result) => {
                    JsonRpcResponse::ok(
                        id,
                        json!({
                            "content": [
                                { "type": "text", "text": result.to_string() }
                            ]
                        }),
                    )
                }
                Err(msg) => JsonRpcResponse::err(id, -32603, msg),
            }
        }

        // ── Unknown method ────────────────────────────────────────────────────
        other => {
            JsonRpcResponse::err(id, -32601, format!("method not found: {other}"))
        }
    };

    (StatusCode::OK, Json(response))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract the token from an `Authorization: Bearer <token>` header.
/// Returns `None` if the header is absent or not in the expected format.
fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?;
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}
