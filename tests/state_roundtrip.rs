use gitzi::model::{Stage, Task};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct StageWrapper {
    stage: Stage,
}

#[test]
fn stage_kebab_case_roundtrip() {
    let cases = [
        (Stage::Backlog, "backlog"),
        (Stage::Prioritized, "prioritized"),
        (Stage::InProgress, "in-progress"),
        (Stage::WaitingForReview, "waiting-for-review"),
        (Stage::InTesting, "in-testing"),
        (Stage::Done, "done"),
    ];
    for (stage, expected) in cases {
        let wrapped = StageWrapper { stage: stage.clone() };
        let serialized = toml::to_string(&wrapped).unwrap();
        assert!(serialized.contains(expected), "expected '{expected}' in '{serialized}'");
        let decoded: StageWrapper = toml::from_str(&serialized).unwrap();
        assert_eq!(decoded.stage, stage);
    }
}

#[test]
fn task_roundtrip() {
    let original = Task::new("abc123", "epic-1", "Add login endpoint");
    let toml_str = toml::to_string_pretty(&original).unwrap();
    let decoded: Task = toml::from_str(&toml_str).unwrap();
    assert_eq!(decoded.id, original.id);
    assert_eq!(decoded.title, original.title);
    assert_eq!(decoded.stage, Stage::Backlog);
    assert_eq!(decoded.priority, 100);
}

#[test]
fn task_branch_name() {
    let task = Task::new("abc123", "epic-1", "Add Login Endpoint!");
    assert_eq!(task.branch_name(), "gitzi/abc123-add-login-endpoint");
}
