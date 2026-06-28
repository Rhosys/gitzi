use gitzi::model::{HistoryEntry, Stage, Task};
use serde::{Deserialize, Serialize};
use chrono::Utc;

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

#[test]
fn history_entry_roundtrip_stage_change() {
    let mut task = Task::new("t-001", "epic-1", "Test task");
    task.stage = Stage::Coding;
    task.history.push(HistoryEntry::StageChange {
        from: Stage::Coding,
        to: Stage::CodingBuffer,
        at: Utc::now(),
        note: Some("agent completed".into()),
    });

    let toml_str = toml::to_string_pretty(&task).unwrap();
    assert!(toml_str.contains("kind = \"stage_change\""));
    assert!(toml_str.contains("from = \"coding\""));
    assert!(toml_str.contains("to = \"coding-buffer\""));

    let decoded: Task = toml::from_str(&toml_str).unwrap();
    assert_eq!(decoded.history.len(), 1);
    match &decoded.history[0] {
        HistoryEntry::StageChange { from, to, note, .. } => {
            assert_eq!(*from, Stage::Coding);
            assert_eq!(*to, Stage::CodingBuffer);
            assert_eq!(note.as_deref(), Some("agent completed"));
        }
        other => panic!("expected StageChange, got {:?}", other),
    }
}

#[test]
fn history_entry_roundtrip_approval() {
    let mut task = Task::new("t-002", "epic-1", "Approved task");
    task.stage = Stage::Coding;
    task.history.push(HistoryEntry::Approval {
        at: Utc::now(),
        target_stage: Stage::Coding,
    });

    let toml_str = toml::to_string_pretty(&task).unwrap();
    assert!(toml_str.contains("kind = \"approval\""));
    assert!(toml_str.contains("target_stage = \"coding\""));

    let decoded: Task = toml::from_str(&toml_str).unwrap();
    assert_eq!(decoded.history.len(), 1);
    match &decoded.history[0] {
        HistoryEntry::Approval { target_stage, .. } => {
            assert_eq!(*target_stage, Stage::Coding);
        }
        other => panic!("expected Approval, got {:?}", other),
    }
}

#[test]
fn history_entry_roundtrip_rejection() {
    let mut task = Task::new("t-003", "epic-1", "Rejected task");
    task.stage = Stage::Coding;
    task.history.push(HistoryEntry::Rejection {
        at: Utc::now(),
        feedback: "Missing error handling for network timeout".into(),
        returned_to: Stage::Coding,
    });

    let toml_str = toml::to_string_pretty(&task).unwrap();
    assert!(toml_str.contains("kind = \"rejection\""));
    assert!(toml_str.contains("feedback = \"Missing error handling for network timeout\""));
    assert!(toml_str.contains("returned_to = \"coding\""));

    let decoded: Task = toml::from_str(&toml_str).unwrap();
    assert_eq!(decoded.history.len(), 1);
    match &decoded.history[0] {
        HistoryEntry::Rejection { feedback, returned_to, .. } => {
            assert_eq!(feedback, "Missing error handling for network timeout");
            assert_eq!(*returned_to, Stage::Coding);
        }
        other => panic!("expected Rejection, got {:?}", other),
    }
}

#[test]
fn history_mixed_entries_roundtrip() {
    let mut task = Task::new("t-004", "epic-1", "Full lifecycle task");
    task.stage = Stage::CodingBuffer;
    task.history.push(HistoryEntry::StageChange {
        from: Stage::Coding,
        to: Stage::CodingBuffer,
        at: Utc::now(),
        note: Some("agent completed".into()),
    });
    task.history.push(HistoryEntry::Rejection {
        at: Utc::now(),
        feedback: "Needs tests".into(),
        returned_to: Stage::Coding,
    });
    task.history.push(HistoryEntry::StageChange {
        from: Stage::Coding,
        to: Stage::CodingBuffer,
        at: Utc::now(),
        note: None,
    });
    task.history.push(HistoryEntry::Approval {
        at: Utc::now(),
        target_stage: Stage::Coding,
    });

    let toml_str = toml::to_string_pretty(&task).unwrap();
    let decoded: Task = toml::from_str(&toml_str).unwrap();
    assert_eq!(decoded.history.len(), 4);
    assert!(matches!(&decoded.history[0], HistoryEntry::StageChange { .. }));
    assert!(matches!(&decoded.history[1], HistoryEntry::Rejection { .. }));
    assert!(matches!(&decoded.history[2], HistoryEntry::StageChange { .. }));
    assert!(matches!(&decoded.history[3], HistoryEntry::Approval { .. }));
}

#[test]
fn new_column_stages_kebab_case_roundtrip() {
    let cases = [
        (Stage::Designing, "designing"),
        (Stage::CodingBuffer, "coding-buffer"),
        (Stage::Coding, "coding"),
        (Stage::ReviewBuffer, "review-buffer"),
        (Stage::Reviewing, "reviewing"),
        (Stage::SecurityAuditBuffer, "security-audit-buffer"),
        (Stage::Auditing, "auditing"),
        (Stage::DeploymentBuffer, "deployment-buffer"),
        (Stage::Deploying, "deploying"),
    ];
    for (stage, expected) in cases {
        let wrapped = StageWrapper { stage: stage.clone() };
        let serialized = toml::to_string(&wrapped).unwrap();
        assert!(serialized.contains(expected), "expected '{expected}' in '{serialized}'");
        let decoded: StageWrapper = toml::from_str(&serialized).unwrap();
        assert_eq!(decoded.stage, stage);
    }
}
