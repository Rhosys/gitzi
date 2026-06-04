use std::path::Path;
use crate::error::{GitziError, Result};
use crate::model::{Epic, Task, WipSnapshot};

pub fn load_task(repo_root: &Path, id: &str) -> Result<Task> {
    let path = task_dir(repo_root, id).join("task.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|_| GitziError::TaskNotFound(id.to_string()))?;
    Ok(toml::from_str(&text)?)
}

pub fn load_all_tasks(repo_root: &Path) -> Result<Vec<Task>> {
    let dir = repo_root.join(".gitzi").join("tasks");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut tasks = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let task_file = entry.path().join("task.toml");
        if task_file.exists() {
            let text = std::fs::read_to_string(&task_file)?;
            tasks.push(toml::from_str(&text)?);
        }
    }
    Ok(tasks)
}

pub fn load_epic(repo_root: &Path, id: &str) -> Result<Epic> {
    let path = repo_root.join(".gitzi").join("epics").join(format!("{id}.toml"));
    let text = std::fs::read_to_string(&path)
        .map_err(|_| GitziError::EpicNotFound(id.to_string()))?;
    Ok(toml::from_str(&text)?)
}

pub fn load_all_epics(repo_root: &Path) -> Result<Vec<Epic>> {
    let dir = repo_root.join(".gitzi").join("epics");
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut epics = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            let text = std::fs::read_to_string(&path)?;
            epics.push(toml::from_str(&text)?);
        }
    }
    Ok(epics)
}

pub fn load_wip(repo_root: &Path) -> Result<WipSnapshot> {
    let path = repo_root.join(".gitzi").join("wip.toml");
    if !path.exists() {
        return Ok(WipSnapshot::default());
    }
    let text = std::fs::read_to_string(&path)?;
    Ok(toml::from_str(&text)?)
}

pub fn task_dir(repo_root: &Path, task_id: &str) -> std::path::PathBuf {
    repo_root.join(".gitzi").join("tasks").join(task_id)
}

pub fn task_worktree_path(repo_root: &Path, task_id: &str) -> std::path::PathBuf {
    task_dir(repo_root, task_id).join("worktree")
}

pub fn task_log_path(repo_root: &Path, task_id: &str) -> std::path::PathBuf {
    task_dir(repo_root, task_id).join("agent.log")
}
