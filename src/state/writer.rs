use std::collections::HashMap;
use std::path::Path;
use crate::config::atomic_write;
use crate::error::Result;
use crate::model::{Epic, Task, WipSnapshot};

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
