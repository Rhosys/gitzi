use std::collections::HashMap;
use std::path::Path;
use crate::config::atomic_write;
use crate::error::Result;
use crate::model::{Epic, Task, WipSnapshot};
use crate::state::reader::task_dir;

// .gitzi/ is gitignored — state is local filesystem only, never committed to git.

pub fn write_task(repo_root: &Path, task: &Task) -> Result<()> {
    let dir = task_dir(repo_root, &task.id);
    std::fs::create_dir_all(&dir)?;
    atomic_write(&dir.join("task.toml"), &toml::to_string_pretty(task)?)
}

pub fn write_epic(repo_root: &Path, epic: &Epic) -> Result<()> {
    let dir = repo_root.join(".gitzi").join("epics");
    std::fs::create_dir_all(&dir)?;
    atomic_write(&dir.join(format!("{}.toml", epic.id)), &toml::to_string_pretty(epic)?)
}

pub fn write_wip(repo_root: &Path, tasks: &[Task]) -> Result<()> {
    let path = repo_root.join(".gitzi").join("wip.toml");
    atomic_write(&path, &toml::to_string_pretty(&wip_snapshot(tasks))?)
}

pub fn rebuild_wip(repo_root: &Path) -> Result<()> {
    let tasks = crate::state::reader::load_all_tasks(repo_root)?;
    write_wip(repo_root, &tasks)
}

pub fn append_agent_log(repo_root: &Path, task_id: &str, output: &str) -> Result<()> {
    use std::io::Write;
    let path = crate::state::reader::task_log_path(repo_root, task_id);
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{output}")?;
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
