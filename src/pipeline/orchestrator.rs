use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;
use crate::config::Config;
use crate::error::{GitziError, Result};
use crate::model::{Stage, Task};
use crate::pipeline::transitions::validate_transition;
use crate::state::{reader, writer};
use crate::state::watcher::StateEvent;

pub struct Orchestrator {
    pub repo_root: PathBuf,
    pub config: Arc<Config>,
    pub tx: broadcast::Sender<StateEvent>,
}

impl Orchestrator {
    pub fn new(repo_root: PathBuf, config: Arc<Config>, tx: broadcast::Sender<StateEvent>) -> Self {
        Self { repo_root, config, tx }
    }

    pub fn pick_next_task<'a>(&self, tasks: &'a [Task]) -> Option<&'a Task> {
        let in_progress = tasks.iter().filter(|t| t.stage == Stage::InProgress).count();
        if in_progress >= self.config.wip_limits.in_progress as usize {
            return None;
        }
        tasks
            .iter()
            .filter(|t| t.stage == Stage::Prioritized && !t.wip_limit_blocked)
            .min_by_key(|t| t.priority)
    }

    pub fn advance_task(&self, task_id: &str, to: Stage, note: Option<String>) -> Result<()> {
        let mut task = reader::load_task(&self.repo_root, task_id)?;
        validate_transition(&task.stage, &to)?;

        let stage_str = to.to_string();
        let limit = match to {
            Stage::InProgress => Some(self.config.wip_limits.in_progress),
            Stage::WaitingForReview => Some(self.config.wip_limits.waiting_for_review),
            Stage::InTesting => Some(self.config.wip_limits.in_testing),
            _ => None,
        };

        if let Some(limit) = limit {
            let tasks = reader::load_all_tasks(&self.repo_root)?;
            let count = tasks.iter().filter(|t| t.stage == to && t.id != task_id).count();
            if count >= limit as usize {
                return Err(GitziError::WipLimitExceeded { stage: stage_str, limit });
            }
        }

        task.transition_to(to, note);
        writer::write_task(&self.repo_root, &task)?;
        writer::rebuild_wip(&self.repo_root)?;
        let _ = self.tx.send(StateEvent::TaskChanged(task_id.to_string()));
        Ok(())
    }

    pub fn set_task_branch(&self, task_id: &str, branch: &str) -> Result<()> {
        let mut task = reader::load_task(&self.repo_root, task_id)?;
        task.branch = Some(branch.to_string());
        task.updated_at = chrono::Utc::now();
        writer::write_task(&self.repo_root, &task)?;
        Ok(())
    }

    pub fn reject_task(&self, task_id: &str, feedback: &str) -> Result<()> {
        let mut task = reader::load_task(&self.repo_root, task_id)?;
        task.agent_feedback = Some(feedback.to_string());
        writer::write_task(&self.repo_root, &task)?;
        self.advance_task(task_id, Stage::InProgress, Some(format!("rejected: {feedback}")))?;
        Ok(())
    }
}
