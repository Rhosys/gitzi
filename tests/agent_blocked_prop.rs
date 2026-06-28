// Feature: event-driven-dispatcher, Property 16: Blocked agent rejects new work
// **Validates: Requirements 10.2**

use gitzi::dispatcher::AgentRole;
use proptest::prelude::*;

/// Strategy that generates an arbitrary `AgentRole`.
fn arb_role() -> impl Strategy<Value = AgentRole> {
    prop_oneof![
        Just(AgentRole::Prioritizer),
        Just(AgentRole::Designer),
        Just(AgentRole::Coder),
        Just(AgentRole::Reviewer),
        Just(AgentRole::Auditor),
        Just(AgentRole::Infrarian),
    ]
}

proptest! {
    /// Property 16: Blocked agent rejects new work
    ///
    /// For any agent role, when the blocked flag is set to true, `is_blocked()`
    /// returns true — which is the guard condition the agent loop uses to skip
    /// work pickup. This validates the state-machine invariant:
    /// `blocked == true` implies the agent will not process work.
    #[test]
    fn blocked_agent_rejects_new_work(role in arb_role()) {
        // Construct a handle via the AgentPool to get a real AgentHandle
        // We test the invariant at the data-structure level: the AtomicBool
        // guard that the agent loop checks after every wake.
        use gitzi::dispatcher::agent_pool::AgentHandle;

        let handle = AgentHandle::new_for_test(role);

        // Initially not blocked — agent would pick up work
        prop_assert!(!handle.is_blocked(), "Handle should start unblocked");

        // Set blocked flag (simulates AgentBlocked state)
        handle.set_blocked(true);

        // The guard condition: is_blocked() == true means agent skips work
        prop_assert!(
            handle.is_blocked(),
            "After setting blocked=true, is_blocked() must return true for role {:?}",
            role
        );

        // Signal the agent — in the real loop, this would wake it but the
        // `if handle.is_blocked() { continue }` guard prevents task pickup.
        // We verify the flag remains true after signalling (signal doesn't clear it).
        handle.signal();
        prop_assert!(
            handle.is_blocked(),
            "Signal must NOT clear the blocked flag for role {:?}",
            role
        );

        // Unblock — simulates human answer delivered
        handle.set_blocked(false);
        prop_assert!(
            !handle.is_blocked(),
            "After clearing blocked flag, agent should be unblocked for role {:?}",
            role
        );
    }
}
