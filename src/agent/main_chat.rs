//! Unified trait for the main chat agent backend, allowing both OpenAI-compatible
//! HTTP and AWS Bedrock (via rig-bedrock) to drive the interactive tool loop.
//!
//! The dispatcher owns tool execution; the backend is responsible only for:
//! 1. Sending messages + tool definitions to the model
//! 2. Returning either text or tool-call requests
//!
//! Both backends implement [`MainChatBackend`], and the dispatcher drives them
//! identically via the same tool-calling loop.

use std::future::Future;
use std::pin::Pin;

use crate::error::Result;
use super::main_agent::{OaiMessage, OaiTool, ChatTurn};

/// Boxed future returned by [`MainChatBackend::turn`].
pub type TurnFuture<'a> = Pin<Box<dyn Future<Output = Result<(OaiMessage, ChatTurn)>> + Send + 'a>>;

/// Trait abstracting one "turn" of the main chat agent. The dispatcher calls
/// `turn()` in a loop, feeding tool results back as messages, until the model
/// emits a text response.
///
/// Returns `(raw_assistant_message, parsed_turn)` — the raw message is needed
/// to append to the conversation for multi-turn threading.
pub trait MainChatBackend: Send + Sync {
    /// Execute one model turn: send the conversation history + tool definitions
    /// and return either a text response or a list of tool-call requests.
    fn turn<'a>(
        &'a self,
        messages: &'a [OaiMessage],
        tools: &'a [OaiTool],
    ) -> TurnFuture<'a>;

    /// The model identifier (for diagnostics / logging).
    fn model(&self) -> String;

    /// The OpenAI-compatible base URL, if this backend speaks that protocol.
    /// Returns `None` for backends (like Bedrock) that don't expose an HTTP
    /// endpoint usable by the classifier.
    fn base_url(&self) -> Option<String> {
        None
    }
}
