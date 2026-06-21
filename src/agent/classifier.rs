//! Lightweight interrupt classifier — determines what to do when the user sends
//! a message while the main agent is already processing one.

use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tracing::warn;

/// The three possible interrupt dispositions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptAction {
    /// Cancel the in-flight request, merge messages, retry as one turn.
    Amend,
    /// Hold the new message until the current response arrives, then deliver.
    Queue,
    /// Fork a new chat session, process the new message independently.
    Fork,
}

const SYSTEM_PROMPT: &str = "\
You are a message router. Given a conversation context, a pending message \
currently being processed, and a new message from the user, classify the new message. \
Respond with exactly one word: amend, queue, or fork.\n\n\
- amend: the new message updates, corrects, or supersedes the pending message\n\
- queue: the new message is related and can wait until the current response finishes\n\
- fork: the new message is a completely different thought unrelated to the pending topic\n\n\
Respond with one word only.";

/// Classify an interrupt message against the currently in-flight message.
/// Falls back to `Queue` on any error.
pub async fn classify(
    base_url: &str,
    model: &str,
    context_turns: &[String],
    pending_message: &str,
    new_message: &str,
) -> InterruptAction {
    let user_content = format!(
        "Recent context:\n{}\n\nPending message (currently being processed):\n{}\n\nNew message:\n{}",
        context_turns.join("\n"),
        pending_message,
        new_message,
    );

    let body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": SYSTEM_PROMPT },
            { "role": "user", "content": user_content },
        ],
        "max_tokens": 5,
        "temperature": 0.0,
    });

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    let parsed: ClassifierResponse =
        match crate::agent::llm_client::post_with_retry(&Client::new(), &url, &body).await {
            Ok(r) => r,
            Err(e) => {
                warn!("interrupt classifier failed: {e}");
                return InterruptAction::Queue;
            }
        };

    let word = parsed
        .choices
        .first()
        .and_then(|c| c.message.content.as_deref())
        .unwrap_or("")
        .trim()
        .to_lowercase();

    match word.as_str() {
        "amend" => InterruptAction::Amend,
        "fork" => InterruptAction::Fork,
        _ => InterruptAction::Queue,
    }
}

#[derive(Deserialize)]
struct ClassifierResponse {
    choices: Vec<ClassifierChoice>,
}

#[derive(Deserialize)]
struct ClassifierChoice {
    message: ClassifierMessage,
}

#[derive(Deserialize)]
struct ClassifierMessage {
    content: Option<String>,
}


/// Generate a short 2-4 word topic name for a fork from the user's message.
/// Falls back to the first few words of the message on failure.
pub async fn name_fork(base_url: &str, model: &str, message: &str) -> String {
    let body = json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": "Generate a 2-4 word topic label for this message. \
                            Respond with only the label, no punctuation, no quotes."
            },
            { "role": "user", "content": message },
        ],
        "max_tokens": 10,
        "temperature": 0.3,
    });

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    let parsed: ClassifierResponse =
        match crate::agent::llm_client::post_with_retry(&Client::new(), &url, &body).await {
            Ok(r) => r,
            Err(_) => return fallback_name(message),
        };

    parsed
        .choices
        .first()
        .and_then(|c| c.message.content.as_deref())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() < 50)
        .unwrap_or_else(|| fallback_name(message))
}

fn fallback_name(message: &str) -> String {
    message
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join(" ")
}
