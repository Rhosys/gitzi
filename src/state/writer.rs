use std::collections::HashMap;
use crate::config::atomic_write;
use crate::error::Result;
use crate::model::{Epic, Task, WipSnapshot};
use crate::state::reader::{epic_file, plan_dir, task_file, task_log_path, task_wip_dir, wip_file};

pub fn write_task(task: &Task) -> Result<()> {
    std::fs::create_dir_all(plan_dir().join("tasks"))?;
    atomic_write(&task_file(&task.id), &toml::to_string_pretty(task)?)
}

pub fn write_epic(epic: &Epic) -> Result<()> {
    std::fs::create_dir_all(plan_dir().join("epics"))?;
    atomic_write(&epic_file(&epic.id), &toml::to_string_pretty(epic)?)
}

pub fn write_wip(tasks: &[Task]) -> Result<()> {
    let path = wip_file();
    std::fs::create_dir_all(path.parent().unwrap())?;
    atomic_write(&path, &toml::to_string_pretty(&wip_snapshot(tasks))?)
}

pub fn rebuild_wip() -> Result<()> {
    write_wip(&crate::state::reader::load_all_tasks()?)
}

pub fn append_agent_log(task_id: &str, output: &str) -> Result<()> {
    use std::io::Write;
    let dir = task_wip_dir(task_id);
    std::fs::create_dir_all(&dir)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(task_log_path(task_id))?;
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
