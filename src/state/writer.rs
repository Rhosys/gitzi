use std::collections::HashMap;
use std::path::Path;
use crate::config::{atomic_write, Config};
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
    let path = repo_root.join(".gitzi").join("wip.toml");
    atomic_write(&path, &toml::to_string_pretty(&wip_snapshot(tasks))?)
}

pub fn rebuild_wip(repo_root: &Path) -> Result<()> {
    let tasks = crate::state::reader::load_all_tasks(repo_root)?;
    write_wip(repo_root, &tasks)
}

// ── Git object commits → state branch ────────────────────────────────────────
//
// All .gitzi/ state changes are committed as git objects onto `config.state_branch`
// (default: "gitzi/state") — never onto main. The user reviews and opens a PR
// to merge that branch into main when satisfied.

/// Commit a task file onto the state branch.
pub fn commit_task(repo_root: &Path, config: &Config, task: &Task) -> Result<()> {
    let content = toml::to_string_pretty(task)?;
    commit_to_state_branch(
        repo_root,
        config,
        &[(&format!(".gitzi/tasks/{}.toml", task.id), content.as_bytes())],
        &format!("gitzi: update task {}", task.id),
    )
}

/// Commit a wip snapshot onto the state branch.
pub fn commit_wip_to_branch(repo_root: &Path, config: &Config, tasks: &[Task]) -> Result<()> {
    let content = toml::to_string_pretty(&wip_snapshot(tasks))?;
    commit_to_state_branch(
        repo_root,
        config,
        &[(".gitzi/wip.toml", content.as_bytes())],
        "gitzi: update wip",
    )
}

/// Commit an epic file onto the state branch.
pub fn commit_epic(repo_root: &Path, config: &Config, epic: &Epic) -> Result<()> {
    let content = toml::to_string_pretty(epic)?;
    commit_to_state_branch(
        repo_root,
        config,
        &[(&format!(".gitzi/epics/{}.toml", epic.id), content.as_bytes())],
        &format!("gitzi: update epic {}", epic.id),
    )
}

fn commit_to_state_branch(
    repo_root: &Path,
    config: &Config,
    files: &[(&str, &[u8])],
    message: &str,
) -> Result<()> {
    let repo = crate::git::ops::open_repo(repo_root)?;
    crate::git::ops::commit_files_to_branch(&repo, &config.state_branch, files, message)?;
    Ok(())
}

fn wip_snapshot(tasks: &[Task]) -> WipSnapshot {
    let mut stages: HashMap<String, Vec<String>> = HashMap::new();
    for task in tasks {
        stages
            .entry(task.stage.to_string())
            .or_default()
            .push(task.id.clone());
    }
    WipSnapshot { stages }
}
