use std::collections::HashMap;
use std::path::Path;
use crate::config::atomic_write;
use crate::error::Result;
use crate::model::{Epic, Task, WipSnapshot};

// ── Filesystem writes (for the harness to read back immediately) ──────────────

pub fn write_task(repo_root: &Path, task: &Task) -> Result<()> {
    let path = repo_root
        .join(".gitzi")
        .join("tasks")
        .join(format!("{}.toml", task.id));
    atomic_write(&path, &toml::to_string_pretty(task)?)
}

pub fn write_epic(repo_root: &Path, epic: &Epic) -> Result<()> {
    let path = repo_root
        .join(".gitzi")
        .join("epics")
        .join(format!("{}.toml", epic.id));
    atomic_write(&path, &toml::to_string_pretty(epic)?)
}

pub fn write_wip(repo_root: &Path, tasks: &[Task]) -> Result<()> {
    let mut stages: HashMap<String, Vec<String>> = HashMap::new();
    for task in tasks {
        stages
            .entry(task.stage.to_string())
            .or_default()
            .push(task.id.clone());
    }
    let snapshot = WipSnapshot { stages };
    let path = repo_root.join(".gitzi").join("wip.toml");
    atomic_write(&path, &toml::to_string_pretty(&snapshot)?)
}

pub fn rebuild_wip(repo_root: &Path) -> Result<()> {
    let tasks = crate::state::reader::load_all_tasks(repo_root)?;
    write_wip(repo_root, &tasks)
}

// ── Git object commits (no working-tree staging) ──────────────────────────────
//
// These functions commit state files directly as git objects to the current
// HEAD branch, without touching the index or working tree. Safe to call while
// agents are running in worktrees.

/// Commit a single task file as a git object to HEAD.
pub fn commit_task(repo_root: &Path, task: &Task) -> Result<()> {
    let content = toml::to_string_pretty(task)?;
    let git_path = format!(".gitzi/tasks/{}.toml", task.id);
    commit_to_head(repo_root, &[(&git_path, content.as_bytes())], &format!("gitzi: update task {}", task.id))
}

/// Commit the wip snapshot as a git object to HEAD.
pub fn commit_wip(repo_root: &Path, tasks: &[Task]) -> Result<()> {
    let mut stages: HashMap<String, Vec<String>> = HashMap::new();
    for task in tasks {
        stages.entry(task.stage.to_string()).or_default().push(task.id.clone());
    }
    let snapshot = WipSnapshot { stages };
    let content = toml::to_string_pretty(&snapshot)?;
    commit_to_head(repo_root, &[(".gitzi/wip.toml", content.as_bytes())], "gitzi: update wip")
}

fn commit_to_head(repo_root: &Path, files: &[(&str, &[u8])], message: &str) -> Result<()> {
    let repo = crate::git::ops::open_repo(repo_root)?;
    let head_branch = match repo.head() {
        Ok(h) => h.shorthand().unwrap_or("main").to_string(),
        Err(_) => "main".to_string(),
    };
    crate::git::ops::commit_files_to_branch(&repo, &head_branch, files, message)?;
    Ok(())
}
