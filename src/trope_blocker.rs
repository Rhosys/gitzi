//! TropeBlocker — intercepts LLM responses that match known lazy/evasive patterns
//! and replaces them with corrective actions.
//!
//! Sits between raw agent output and the user/next-action pipeline. When a trope
//! is detected the response is stripped and a TropeAction is returned instead.

use tracing::warn;

/// The result of scanning a response through the TropeBlocker.
pub enum ScanResult {
    /// No trope detected — pass the response through unchanged.
    Clean,
    /// A trope was matched — the original response must be discarded
    /// and this action executed instead.
    Blocked(TropeMatch),
}

impl ScanResult {
    /// Returns true if no trope was detected.
    pub fn is_clean(&self) -> bool {
        matches!(self, ScanResult::Clean)
    }
}

/// A matched trope with its metadata and prescribed action.
pub struct TropeMatch {
    pub name: &'static str,
    pub action: TropeAction,
}

/// What to do when a trope is detected.
pub enum TropeAction {
    /// Check actual context size. If over threshold, rotate session with summary.
    /// If under threshold, inject "continue" directive and suppress the response.
    LargeContext,
    // Future tropes add variants here:
    // RefusalToEdit,
    // UnnecessaryConfirmation,
    // ScopeInflation,
}

/// Hardcoded trope definitions. Each trope is a set of signal phrases —
/// if enough of them appear in a single response, the trope fires.
struct TropeDefinition {
    name: &'static str,
    /// Phrases that indicate this trope. Case-insensitive substring matching.
    signals: &'static [&'static str],
    /// Minimum number of signals that must match to trigger.
    threshold: usize,
    #[allow(dead_code)]
    action: TropeAction,
}

const TROPES: &[TropeDefinition] = &[
    TropeDefinition {
        name: "Large Context",
        signals: &[
            "context is getting long",
            "context budget",
            "fresh session",
            "full day's work",
            "recommend: let me commit",
            "tackle cleanly in a fresh",
            "been going for a while",
            "running low on context",
            "context window",
            "new session with full context",
            "split this into a separate",
            "this is getting too large",
            "let me stop here",
            "pick this up in a new session",
        ],
        threshold: 2,
        action: TropeAction::LargeContext,
    },
];

/// Scan an LLM response for trope matches.
pub fn scan(response: &str) -> ScanResult {
    let lower = response.to_lowercase();

    for trope in TROPES {
        let hits = trope.signals.iter()
            .filter(|signal| lower.contains(*signal))
            .count();

        if hits >= trope.threshold {
            warn!(
                "TropeBlocker: detected '{}' trope ({}/{} signals matched)",
                trope.name, hits, trope.signals.len()
            );
            return ScanResult::Blocked(TropeMatch {
                name: trope.name,
                // Can't move out of a static ref, so reconstruct the action
                action: match trope.name {
                    "Large Context" => TropeAction::LargeContext,
                    _ => unreachable!(),
                },
            });
        }
    }

    ScanResult::Clean
}

/// Context size threshold in tokens (approximate). Above this we actually
/// do rotate the session. Below this the agent was being lazy.
const CONTEXT_ROTATION_THRESHOLD: usize = 180_000;

/// Execute the action for a matched trope. Returns the directive to inject
/// back into the agent (replacing the blocked response).
pub fn execute(trope_match: &TropeMatch, current_context_tokens: usize, summary: &str) -> Directive {
    match trope_match.action {
        TropeAction::LargeContext => {
            if current_context_tokens > CONTEXT_ROTATION_THRESHOLD {
                Directive::RotateSession {
                    summary: summary.to_string(),
                }
            } else {
                Directive::Continue {
                    injection: "Your context is fine. Stop stalling and continue working on the task.".to_string(),
                }
            }
        }
    }
}

/// What the harness should do after a trope is blocked.
pub enum Directive {
    /// Inject this message into the agent as a user turn and continue.
    /// The blocked response is discarded — the user never sees it.
    Continue { injection: String },
    /// Start a new session with the summary as seed context.
    /// The blocked response is discarded.
    RotateSession { summary: String },
}
