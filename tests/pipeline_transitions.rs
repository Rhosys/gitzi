use gitzi::pipeline::transitions::{next_stage, validate_transition};
use gitzi::model::Stage;

#[test]
fn valid_happy_path() {
    let path = [
        Stage::Backlog,
        Stage::Prioritized,
        Stage::InProgress,
        Stage::WaitingForReview,
        Stage::InTesting,
        Stage::Done,
    ];
    for window in path.windows(2) {
        validate_transition(&window[0], &window[1]).unwrap();
    }
}

#[test]
fn rejection_is_valid() {
    validate_transition(&Stage::WaitingForReview, &Stage::InProgress).unwrap();
}

#[test]
fn test_failure_is_valid() {
    validate_transition(&Stage::InTesting, &Stage::InProgress).unwrap();
}

#[test]
fn skipping_stages_is_invalid() {
    assert!(validate_transition(&Stage::Backlog, &Stage::InProgress).is_err());
    assert!(validate_transition(&Stage::Prioritized, &Stage::Done).is_err());
    assert!(validate_transition(&Stage::Backlog, &Stage::Done).is_err());
}

#[test]
fn backwards_is_invalid_except_rejection() {
    assert!(validate_transition(&Stage::Done, &Stage::InTesting).is_err());
    assert!(validate_transition(&Stage::InProgress, &Stage::Prioritized).is_err());
}

#[test]
fn next_stage_happy_path() {
    assert_eq!(next_stage(&Stage::Backlog), Some(Stage::Prioritized));
    assert_eq!(next_stage(&Stage::Done), None);
}
