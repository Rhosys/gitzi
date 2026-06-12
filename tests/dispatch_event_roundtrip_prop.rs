// Feature: dispatcher-audit-fixes, Property 13: DispatchEvent JSON round-trip
// **Validates: Requirements 9.1, 9.2**
//
// For any valid DispatchEvent variant with random payloads, serializing to JSON
// then deserializing back produces an equivalent value.

use gitzi::dispatcher::event_bus::DispatchEvent;
use gitzi::dispatcher::{AgentRole, Column};
use proptest::prelude::*;

/// Strategy that generates an arbitrary `Column`.
fn arb_column() -> impl Strategy<Value = Column> {
    prop_oneof![
        Just(Column::Prioritized),
        Just(Column::Designing),
        Just(Column::CodingBuffer),
        Just(Column::Coding),
        Just(Column::ReviewBuffer),
        Just(Column::Reviewing),
        Just(Column::TestBuffer),
        Just(Column::Testing),
        Just(Column::SecurityAuditBuffer),
        Just(Column::Auditing),
        Just(Column::DeploymentBuffer),
        Just(Column::Deploying),
        Just(Column::Done),
    ]
}

/// Strategy that generates an arbitrary `AgentRole`.
fn arb_agent_role() -> impl Strategy<Value = AgentRole> {
    prop_oneof![
        Just(AgentRole::Prioritizer),
        Just(AgentRole::Designer),
        Just(AgentRole::Coder),
        Just(AgentRole::Reviewer),
        Just(AgentRole::Tester),
        Just(AgentRole::Auditor),
        Just(AgentRole::Infrarian),
    ]
}

/// Strategy that generates an arbitrary `DispatchEvent` variant.
fn arb_dispatch_event() -> impl Strategy<Value = DispatchEvent> {
    prop_oneof![
        ".{1,100}".prop_map(|task_id| DispatchEvent::TaskCreated { task_id }),
        (".{1,100}", arb_column(), arb_column())
            .prop_map(|(task_id, from, to)| DispatchEvent::TaskStageChanged {
                task_id,
                from,
                to,
            }),
        (".{1,100}", arb_column())
            .prop_map(|(task_id, target_column)| DispatchEvent::HumanApprovalReceived {
                task_id,
                target_column,
            }),
        (".{1,100}", arb_column(), ".{1,200}")
            .prop_map(|(task_id, returned_to, feedback)| {
                DispatchEvent::HumanRejectionReceived {
                    task_id,
                    returned_to,
                    feedback,
                }
            }),
        (".{1,100}", arb_agent_role())
            .prop_map(|(task_id, agent_role)| DispatchEvent::AgentCompleted {
                task_id,
                agent_role,
            }),
        (".{1,100}", arb_agent_role(), ".{1,300}")
            .prop_map(|(task_id, agent_role, question)| DispatchEvent::AgentBlocked {
                task_id,
                agent_role,
                question,
            }),
        Just(DispatchEvent::BootComplete),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 100, ..Default::default() })]

    /// Property 13: DispatchEvent JSON round-trip
    ///
    /// For any valid DispatchEvent variant, serializing to JSON and deserializing
    /// back produces an equivalent value.
    #[test]
    fn dispatch_event_json_roundtrip(event in arb_dispatch_event()) {
        let json = serde_json::to_string(&event).expect("serialize should succeed");
        let deserialized: DispatchEvent =
            serde_json::from_str(&json).expect("deserialize should succeed");
        prop_assert_eq!(event, deserialized);
    }
}
