// Feature: event-driven-dispatcher, Property 14: History serialization round-trip
// **Validates: Requirements 8.3**
//
// For any arbitrary HistoryEntry (generated via proptest), serializing to TOML
// and deserializing back produces an equal value. Tests all 3 variants with
// arbitrary field values.

use chrono::{DateTime, TimeZone, Utc};
use gitzi::model::{HistoryEntry, Stage};
use proptest::prelude::*;
use serde::{Deserialize, Serialize};

/// Wrapper so we can serialize a single HistoryEntry as a TOML table with an
/// array field (TOML requires top-level to be a table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Wrapper {
    entries: Vec<HistoryEntry>,
}

/// Generate an arbitrary Stage.
fn arb_stage() -> impl Strategy<Value = Stage> {
    prop_oneof![
        Just(Stage::Backlog),
        Just(Stage::InProgress),
        Just(Stage::WaitingForReview),
        Just(Stage::Prioritized),
        Just(Stage::Designing),
        Just(Stage::CodingBuffer),
        Just(Stage::Coding),
        Just(Stage::ReviewBuffer),
        Just(Stage::Reviewing),
        Just(Stage::SecurityAuditBuffer),
        Just(Stage::Auditing),
        Just(Stage::DeploymentBuffer),
        Just(Stage::Deploying),
        Just(Stage::Done),
    ]
}

/// Generate a DateTime<Utc> within a reasonable range.
/// TOML datetime has second precision, so we truncate sub-seconds to ensure
/// round-trip equality.
fn arb_datetime() -> impl Strategy<Value = DateTime<Utc>> {
    // Range: 2020-01-01 to 2030-01-01 (seconds)
    (1_577_836_800i64..1_893_456_000i64).prop_map(|secs| Utc.timestamp_opt(secs, 0).unwrap())
}

/// Generate an arbitrary note (Option<String>). Avoid TOML-breaking characters
/// by using printable ASCII subset that doesn't include problematic chars for
/// inline TOML strings.
fn arb_note() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(None),
        "[a-zA-Z0-9 _.,!?:;/#@&()-]{0,80}".prop_map(Some),
    ]
}

/// Generate arbitrary feedback text.
fn arb_feedback() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 _.,!?:;/#@&()-]{1,120}"
}

/// Generate an arbitrary HistoryEntry across all 3 variants.
fn arb_history_entry() -> impl Strategy<Value = HistoryEntry> {
    prop_oneof![
        // StageChange
        (arb_stage(), arb_stage(), arb_datetime(), arb_note())
            .prop_map(|(from, to, at, note)| HistoryEntry::StageChange { from, to, at, note }),
        // Approval
        (arb_datetime(), arb_stage())
            .prop_map(|(at, target_stage)| { HistoryEntry::Approval { at, target_stage } }),
        // Rejection
        (arb_datetime(), arb_feedback(), arb_stage()).prop_map(|(at, feedback, returned_to)| {
            HistoryEntry::Rejection {
                at,
                feedback,
                returned_to,
            }
        }),
    ]
}

proptest! {
    #[test]
    fn history_entry_toml_roundtrip(entry in arb_history_entry()) {
        let wrapper = Wrapper { entries: vec![entry.clone()] };
        let serialized = toml::to_string_pretty(&wrapper)
            .expect("serialization should succeed");
        let deserialized: Wrapper = toml::from_str(&serialized)
            .expect("deserialization should succeed");
        prop_assert_eq!(&deserialized.entries[0], &entry);
    }

    #[test]
    fn history_multiple_entries_roundtrip(
        entries in prop::collection::vec(arb_history_entry(), 1..10)
    ) {
        let wrapper = Wrapper { entries: entries.clone() };
        let serialized = toml::to_string_pretty(&wrapper)
            .expect("serialization should succeed");
        let deserialized: Wrapper = toml::from_str(&serialized)
            .expect("deserialization should succeed");
        prop_assert_eq!(&deserialized.entries, &entries);
    }
}
