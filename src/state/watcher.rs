use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub enum StateEvent {
    TaskChanged(String),
    WipChanged,
    AnyChange,
}

pub fn start_watcher(
    gitzi_dir: &Path,
    tx: broadcast::Sender<StateEvent>,
) -> notify::Result<RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            for path in &event.paths {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();

                let state_event = if path
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    == Some("tasks")
                {
                    StateEvent::TaskChanged(name)
                } else if name == "wip" {
                    StateEvent::WipChanged
                } else {
                    StateEvent::AnyChange
                };

                let _ = tx.send(state_event);
            }
        }
    })?;

    watcher.watch(gitzi_dir, RecursiveMode::Recursive)?;
    Ok(watcher)
}
