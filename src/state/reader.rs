use std::path::PathBuf;
use crate::error::{GitziError, Result};
use crate::model::{Epic, Task, WipSnapshot};
use crate::state::home;

// ── Path helpers ──────────────────────────────────────────────────────────────

pub fn plan_dir() -> Result<PathBuf> { Ok(home::session_dir()?.join("plan")) }
pub fn wip_dir()  -> Result<PathBuf> { Ok(home::session_dir()?.join("wip")) }

pub fn task_file(task_id: &str) -> Result<PathBuf> {
    Ok(plan_dir()?.join("tasks").join(format!("{task_id}.toml")))
}

pub fn epic_file(epic_id: &str) -> Result<PathBuf> {
    Ok(plan_dir()?.join("epics").join(format!("{epic_id}.toml")))
}

pub fn wip_file() -> Result<PathBuf> {
    Ok(wip_dir()?.join("wip.toml"))
}

pub fn task_wip_dir(task_id: &str) -> Result<PathBuf> {
    Ok(wip_dir()?.join("tasks").join(task_id))
}

pub fn task_worktree_path(task_id: &str) -> Result<PathBuf> {
    Ok(task_wip_dir(task_id)?.join("worktree"))
}

pub fn task_log_path(task_id: &str) -> Result<PathBuf> {
    Ok(task_wip_dir(task_id)?.join("agent.log"))
}

// ── Readers ───────────────────────────────────────────────────────────────────

pub fn load_task(id: &str) -> Result<Task> {
    let path = task_file(id)?;
    let text = std::fs::read_to_string(&path)
        .map_err(|_| GitziError::TaskNotFound(id.to_string()))?;
    Ok(toml::from_str(&text)?)
}

pub fn load_all_tasks() -> Result<Vec<Task>> {
    let dir = plan_dir()?.join("tasks");
    if !dir.exists() { return Ok(Vec::new()); }
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
    let path = epic_file(id)?;
    let text = std::fs::read_to_string(&path)
        .map_err(|_| GitziError::EpicNotFound(id.to_string()))?;
    Ok(toml::from_str(&text)?)
}

pub fn load_all_epics() -> Result<Vec<Epic>> {
    let dir = plan_dir()?.join("epics");
    if !dir.exists() { return Ok(Vec::new()); }
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
    let path = wip_file()?;
    if !path.exists() { return Ok(WipSnapshot::default()); }
    Ok(toml::from_str(&std::fs::read_to_string(&path)?)?)
}
