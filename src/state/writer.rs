use std::collections::HashMap;
use std::path::Path;
use crate::config::atomic_write;
use crate::error::Result;
use crate::model::{Epic, Task, WipSnapshot};
use crate::state::reader::{epic_file, plan_dir, task_file, task_log_path, task_wip_dir, wip_file};

// plan/ → committable project state (epics, tasks)
// wip/  → gitignored runtime state (wip snapshot, worktrees, logs)

pub fn write_task(repo_root: &Path, task: &Task) -> Result<()> {
    let dir = plan_dir(repo_root).join("tasks");
    std::fs::create_dir_all(&dir)?;
    atomic_write(&task_file(repo_root, &task.id), &toml::to_string_pretty(task)?)
}

pub fn write_epic(repo_root: &Path, epic: &Epic) -> Result<()> {
    let dir = plan_dir(repo_root).join("epics");
    std::fs::create_dir_all(&dir)?;
    atomic_write(&epic_file(repo_root, &epic.id), &toml::to_string_pretty(epic)?)
}

pub fn write_wip(repo_root: &Path, tasks: &[Task]) -> Result<()> {
    let path = wip_file(repo_root);
    std::fs::create_dir_all(path.parent().unwrap())?;
    atomic_write(&path, &toml::to_string_pretty(&wip_snapshot(tasks))?)
}

pub fn rebuild_wip(repo_root: &Path) -> Result<()> {
    let tasks = crate::state::reader::load_all_tasks(repo_root)?;
    write_wip(repo_root, &tasks)
}

pub fn append_agent_log(repo_root: &Path, task_id: &str, output: &str) -> Result<()> {
    use std::io::Write;
    let dir = task_wip_dir(repo_root, task_id);
    std::fs::create_dir_all(&dir)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(task_log_path(repo_root, task_id))?;
    writeln!(file, "{output}")?;
    Ok(())
}

fn wip_snapshot(tasks: &[Task]) -> WipSnapshot {
    let mut stages: HashMap<String, Vec<String>> = HashMap::new();
    for task in tasks {
        stages.entry(task.stage.to_string()).or_default().push(task.id.clone());
    }
    WipSnapshot { stages }
}
