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
    }
}
