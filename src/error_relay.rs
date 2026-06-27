//! Error relay: a tracing layer that captures ERROR-level log events and
//! sends them through a broadcast channel for the TUI to display.

use std::sync::OnceLock;
use tokio::sync::broadcast;
use tracing::Level;
use tracing_subscriber::Layer;

static ERROR_TX: OnceLock<broadcast::Sender<String>> = OnceLock::new();

/// Initialize the error relay channel. Returns the receiver side.
/// Must be called before tracing is initialized.
pub fn init() -> broadcast::Receiver<String> {
    let (tx, rx) = broadcast::channel(100);
    ERROR_TX.set(tx).expect("error relay already initialized");
    rx
}

/// Get a new receiver for the error relay (for additional subscribers).
pub fn subscribe() -> Option<broadcast::Receiver<String>> {
    ERROR_TX.get().map(|tx| tx.subscribe())
}

/// The tracing layer that captures errors.
pub struct ErrorRelayLayer;

impl<S> Layer<S> for ErrorRelayLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if *event.metadata().level() != Level::ERROR {
            return;
        }

        let Some(tx) = ERROR_TX.get() else { return };

        // Format the error message
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        let message = if visitor.0.is_empty() {
            format!("{}", event.metadata().target())
        } else {
            visitor.0
        };

        let _ = tx.send(message);
    }
}

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        } else if self.0.is_empty() {
            self.0 = format!("{}: {value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_string();
        } else if self.0.is_empty() {
            self.0 = format!("{}: {value}", field.name());
        }
    }
}
