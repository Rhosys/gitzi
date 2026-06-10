use std::path::{Path, PathBuf};
use crate::error::{GitziError, Result};
use crate::model::{Epic, Task, WipSnapshot};
use crate::state::home;

// ── Path helpers ──────────────────────────────────────────────────────────────
//
// All state lives in ~/.gitzi/projects/<id>/ — never inside the project repo.
// `repo_root` is used only to derive the project ID via a path hash.

pub fn plan_dir(repo_root: &Path) -> PathBuf {
    home::project_dir(repo_root).join("plan")
}

pub fn wip_dir(repo_root: &Path) -> PathBuf {
    home::project_dir(repo_root).join("wip")
}

pub fn task_file(repo_root: &Path, task_id: &str) -> PathBuf {
    plan_dir(repo_root).join("tasks").join(format!("{task_id}.toml"))
}

pub fn epic_file(repo_root: &Path, epic_id: &str) -> PathBuf {
    plan_dir(repo_root).join("epics").join(format!("{epic_id}.toml"))
}

pub fn wip_file(repo_root: &Path) -> PathBuf {
    wip_dir(repo_root).join("wip.toml")
}

pub fn task_wip_dir(repo_root: &Path, task_id: &str) -> PathBuf {
    wip_dir(repo_root).join("tasks").join(task_id)
}

pub fn task_worktree_path(repo_root: &Path, task_id: &str) -> PathBuf {
    task_wip_dir(repo_root, task_id).join("worktree")
}

pub fn task_log_path(repo_root: &Path, task_id: &str) -> PathBuf {
    task_wip_dir(repo_root, task_id).join("agent.log")
}

// ── Readers ───────────────────────────────────────────────────────────────────

pub fn load_task(repo_root: &Path, id: &str) -> Result<Task> {
    let path = task_file(repo_root, id);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| GitziError::TaskNotFound(id.to_string()))?;
    Ok(toml::from_str(&text)?)
}

pub fn load_all_tasks(repo_root: &Path) -> Result<Vec<Task>> {
    let dir = plan_dir(repo_root).join("tasks");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut tasks = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            tasks.push(toml::from_str(&std::fs::read_to_string(&path)?)?);
        }
    }
    Ok(tasks)
}

pub fn load_epic(repo_root: &Path, id: &str) -> Result<Epic> {
    let path = epic_file(repo_root, id);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| GitziError::EpicNotFound(id.to_string()))?;
    Ok(toml::from_str(&text)?)
}

pub fn load_all_epics(repo_root: &Path) -> Result<Vec<Epic>> {
    let dir = plan_dir(repo_root).join("epics");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut epics = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            epics.push(toml::from_str(&std::fs::read_to_string(&path)?)?);
        }
    }
    Ok(epics)
}

pub fn load_wip(repo_root: &Path) -> Result<WipSnapshot> {
    let path = wip_file(repo_root);
    if !path.exists() {
        return Ok(WipSnapshot::default());
    }
    Ok(toml::from_str(&std::fs::read_to_string(&path)?)?)
}
