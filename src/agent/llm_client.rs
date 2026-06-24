//! Shared LLM HTTP client with retry and auto-start.
//! All LLM calls (main agent, classifier, pipeline agents) go through this module.

use reqwest::Client;
use serde::de::DeserializeOwned;
use std::time::Duration;
use tracing::{info, warn};

/// Maximum wall-clock time for a single `post_with_retry` call (including
/// auto-start attempts). Prevents unbounded blocking when the LLM server
/// is unreachable.
const MAX_TOTAL_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes

/// Attempt an LLM API call. On connection failure in production:
/// 1. Try to start LM Studio via `lms server start`
/// 2. Wait for it to come up (up to 15s with polling)
/// 3. Retry the request once
///
/// The auto-start behavior is skipped during tests (`cfg(test)`) — connection
/// failures return immediately.
///
/// Returns the parsed response body on success.
pub async fn post_with_retry<T: DeserializeOwned>(
    client: &Client,
    url: &str,
    body: &impl serde::Serialize,
) -> Result<T, LlmError> {
    let deadline = tokio::time::Instant::now() + MAX_TOTAL_TIMEOUT;

    // First attempt
    let first = tokio::time::timeout_at(deadline, try_post::<T>(client, url, body)).await;
    match first {
        Ok(Ok(resp)) => return Ok(resp),
        Ok(Err(LlmError::ConnectionFailed(e))) => {
            if cfg!(test) {
                return Err(LlmError::ConnectionFailed(e));
            }
            warn!("LLM connection failed: {e} — attempting auto-start");
        }
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(LlmError::ConnectionFailed(
            "total timeout exceeded waiting for LLM".to_string(),
        )),
    }

    // Try starting LM Studio (production only — cfg(test) returns above)
    attempt_start_lms();

    // Wait for the server to become reachable (poll every 2s, up to 15s)
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    let wait_limit = remaining.min(Duration::from_secs(15));
    let started = wait_for_server(url, wait_limit).await;
    if !started {
        return Err(LlmError::ConnectionFailed(
            "LM Studio failed to start within timeout".to_string(),
        ));
    }

    // Retry (bounded by overall deadline)
    match tokio::time::timeout_at(deadline, try_post::<T>(client, url, body)).await {
        Ok(result) => result,
        Err(_) => Err(LlmError::ConnectionFailed(
            "total timeout exceeded on retry".to_string(),
        )),
    }
}

async fn try_post<T: DeserializeOwned>(
    client: &Client,
    url: &str,
    body: &impl serde::Serialize,
) -> Result<T, LlmError> {
    let resp = client
        .post(url)
        .json(body)
        .timeout(Duration::from_secs(120))
        .send()
        .await
        .map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                LlmError::ConnectionFailed(e.to_string())
            } else {
                LlmError::RequestFailed(e.to_string())
            }
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(LlmError::ApiError {
            status: status.as_u16(),
            body: text,
        });
    }

    resp.json::<T>()
        .await
        .map_err(|e| LlmError::ParseFailed(e.to_string()))
}

fn attempt_start_lms() {
    let lms_path = dirs::home_dir()
        .map(|h| h.join(".lmstudio/bin/lms"))
        .filter(|p| p.exists());

    if let Some(lms) = lms_path {
        info!("attempting `lms server start`");
        let _ = std::process::Command::new(lms)
            .args(["server", "start"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    } else {
        warn!("lms binary not found at ~/.lmstudio/bin/lms");
    }
}

async fn wait_for_server(url: &str, timeout: Duration) -> bool {
    // Extract base URL from the full endpoint URL for the health check
    let base = url.rsplitn(2, "/v1").last().unwrap_or(url);
    let health_url = format!("{}/v1/models", base.trim_end_matches('/'));

    let client = Client::new();
    let start = std::time::Instant::now();

    while start.elapsed() < timeout {
        if client
            .get(&health_url)
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .is_ok()
        {
            info!("LLM server is now reachable");
            return true;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    false
}

/// Errors from LLM API calls.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
    #[error("request failed: {0}")]
    RequestFailed(String),
    #[error("API returned {status}: {body}")]
    ApiError { status: u16, body: String },
    #[error("failed to parse response: {0}")]
    ParseFailed(String),
}
