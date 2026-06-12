use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;
use crate::config::Config;
use crate::error::Result;
use crate::model::Stage;
use crate::pipeline::transitions::validate_transition;
use crate::state::{reader, writer};
use crate::state::watcher::StateEvent;

/// Thin compatibility layer retained for the CLI `advance` command and the
/// legacy dashboard. All WIP enforcement and dispatch logic now lives in
/// `src/dispatcher/mod.rs`.
pub struct Orchestrator {
    pub repo_root: PathBuf,
    pub config: Arc<Config>,
    pub tx: broadcast::Sender<StateEvent>,
}

impl Orchestrator {
    pub fn new(repo_root: PathBuf, config: Arc<Config>, tx: broadcast::Sender<StateEvent>) -> Self {
        Self { repo_root, config, tx }
    }

    /// Advance a task to a new stage. Validates the transition and persists the
    /// change. WIP limits are NOT enforced here — that responsibility belongs to
    /// the Dispatcher for event-driven flow.
    pub fn advance_task(&self, task_id: &str, to: Stage, note: Option<String>) -> Result<()> {
        let mut task = reader::load_task(task_id)?;
        validate_transition(&task.stage, &to)?;

        task.transition_to(to, note);
        writer::write_task(&task)?;
        writer::rebuild_wip()?;
        let _ = self.tx.send(StateEvent::TaskChanged(task_id.to_string()));
        Ok(())
    }

    pub fn set_task_branch(&self, task_id: &str, branch: &str) -> Result<()> {
        let mut task = reader::load_task(task_id)?;
        task.branch = Some(branch.to_string());
        task.updated_at = chrono::Utc::now();
        writer::write_task(&task)?;
        Ok(())
    }

    pub fn reject_task(&self, task_id: &str, feedback: &str) -> Result<()> {
        let mut task = reader::load_task(task_id)?;
        task.agent_feedback = Some(feedback.to_string());
        writer::write_task(&task)?;
        self.advance_task(task_id, Stage::InProgress, Some(format!("rejected: {feedback}")))?;
        Ok(())
    }
}
