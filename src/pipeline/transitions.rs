use crate::error::{GitziError, Result};
use crate::model::Stage;

pub fn validate_transition(from: &Stage, to: &Stage) -> Result<()> {
    if is_allowed(from, to) {
        Ok(())
    } else {
        Err(GitziError::InvalidTransition {
            from: from.to_string(),
            to: to.to_string(),
        })
    }
}

fn is_allowed(from: &Stage, to: &Stage) -> bool {
    matches!(
        (from, to),
        (Stage::Backlog, Stage::Prioritized)
            | (Stage::Prioritized, Stage::InProgress)
            | (Stage::InProgress, Stage::WaitingForReview)
            | (Stage::WaitingForReview, Stage::InTesting)
            | (Stage::WaitingForReview, Stage::InProgress) // rejection
            | (Stage::InTesting, Stage::Done)
            | (Stage::InTesting, Stage::InProgress) // test failure → back to agent
    )
}

pub fn next_stage(stage: &Stage) -> Option<Stage> {
    match stage {
        Stage::Backlog => Some(Stage::Prioritized),
        Stage::Prioritized => Some(Stage::InProgress),
        Stage::InProgress => Some(Stage::WaitingForReview),
        Stage::WaitingForReview => Some(Stage::InTesting),
        Stage::InTesting => Some(Stage::Done),
        Stage::Done => None,
        // New column-aligned stages use Column::next() for progression
        Stage::Designing => Some(Stage::CodingBuffer),
        Stage::CodingBuffer => Some(Stage::Coding),
        Stage::Coding => Some(Stage::ReviewBuffer),
        Stage::ReviewBuffer => Some(Stage::Reviewing),
        Stage::Reviewing => Some(Stage::TestBuffer),
        Stage::TestBuffer => Some(Stage::Testing),
        Stage::Testing => Some(Stage::SecurityAuditBuffer),
        Stage::SecurityAuditBuffer => Some(Stage::Auditing),
        Stage::Auditing => Some(Stage::DeploymentBuffer),
        Stage::DeploymentBuffer => Some(Stage::Deploying),
        Stage::Deploying => Some(Stage::Done),
    }
}
