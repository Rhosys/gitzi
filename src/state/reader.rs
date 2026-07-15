use crate::error::{GitziError, Result};
use crate::model::{Epic, Task, WipSnapshot};
use crate::state::home;
use std::path::PathBuf;

// ── Path helpers ──────────────────────────────────────────────────────────────

pub fn plan_dir() -> PathBuf {
    home::plan_dir()
}

pub fn task_file(task_id: &str) -> PathBuf {
    plan_dir().join("tasks").join(format!("{task_id}.toml"))
}

pub fn epic_file(epic_id: &str) -> PathBuf {
    plan_dir().join("epics").join(format!("{epic_id}.toml"))
}

pub fn wip_file() -> PathBuf {
    home::board_file()
}

pub fn task_wip_dir(task_id: &str) -> PathBuf {
    home::task_tmp_dir(task_id)
}

pub fn task_worktree_path(task_id: &str, repo_slug: &str) -> PathBuf {
    home::task_worktree_path(task_id, repo_slug)
}

pub fn task_log_path(task_id: &str) -> PathBuf {
    home::task_log_path(task_id)
}

// ── Readers ───────────────────────────────────────────────────────────────────

pub fn load_task(id: &str) -> Result<Task> {
    let path = task_file(id);
    let text =
        std::fs::read_to_string(&path).map_err(|_| GitziError::TaskNotFound(id.to_string()))?;
    Ok(toml::from_str(&text)?)
}

pub fn load_all_tasks() -> Result<Vec<Task>> {
    let dir = plan_dir().join("tasks");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut tasks = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            tasks.push(toml::from_str(&std::fs::read_to_string(&path)?)?);
        }
    }
    Ok(tasks)
}

pub fn load_epic(id: &str) -> Result<Epic> {
    let path = epic_file(id);
    let text =
        std::fs::read_to_string(&path).map_err(|_| GitziError::EpicNotFound(id.to_string()))?;
    Ok(toml::from_str(&text)?)
}

pub fn load_all_epics() -> Result<Vec<Epic>> {
    let dir = plan_dir().join("epics");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut epics = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            epics.push(toml::from_str(&std::fs::read_to_string(&path)?)?);
        }
    }
    Ok(epics)
}

pub fn load_wip() -> Result<WipSnapshot> {
    let path = wip_file();
    if !path.exists() {
        return Ok(WipSnapshot::default());
    }
    Ok(toml::from_str(&std::fs::read_to_string(&path)?)?)
}
