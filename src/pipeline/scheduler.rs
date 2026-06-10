use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use tracing::{info, warn, error};
use crate::agent::{AgentBackend, RunContext, build_agent};
use crate::config::{AgentDef, Config};
use crate::error::Result;
use crate::git::ops::{self as git, TaskWorktree};
use crate::model::Stage;
use crate::pipeline::orchestrator::Orchestrator;
use crate::runner::run_tests;
use crate::state::{reader, writer};

pub struct Scheduler {
    pub orchestrator: Arc<Orchestrator>,
    pub config: Arc<Config>,
    pub repo_root: std::path::PathBuf,
    pub interval: Duration,
}

impl Scheduler {
    pub fn new(
        orchestrator: Arc<Orchestrator>,
        config: Arc<Config>,
        repo_root: std::path::PathBuf,
    ) -> Self {
        Self { orchestrator, config, repo_root, interval: Duration::from_secs(10) }
    }

    pub async fn run(self) -> Result<()> {
        let mut ticker = time::interval(self.interval);
        loop {
            ticker.tick().await;
            if let Err(e) = self.tick().await {
                error!("Scheduler tick error: {e}");
            }
        }
    }

    async fn tick(&self) -> Result<()> {
        let tasks = reader::load_all_tasks()?;

        if let Some(task) = self.orchestrator.pick_next_task(&tasks) {
            let task = task.clone();
            let branch = task.branch_name();
            let task_id = task.id.clone();
            info!("Dispatching task {task_id} (branch: {branch})");

            let (root, tid, br) = (self.repo_root.clone(), task_id.clone(), branch.clone());
            tokio::task::spawn_blocking(move || {
                let repo = git::open_repo(&root)?;
                TaskWorktree::create(&repo, &tid, &br)
            })
            .await
            .map_err(|e| crate::error::GitziError::AgentFailed(e.to_string()))??;

            let worktree = {
                let repo = git::open_repo(&self.repo_root)?;
                TaskWorktree::open(&repo, &task_id, &branch)?
            };

            self.orchestrator.set_task_branch(&task_id, &branch)?;
            self.orchestrator.advance_task(&task_id, Stage::InProgress, None)?;

            let agent_def = resolve_agent_for_task(&task, &self.config);
            let agent = build_agent(&agent_def)?;
            let ctx = RunContext { repo_root: worktree.path.clone(), branch: branch.clone() };

            match agent.run(&task, &ctx).await {
                Ok(result) if result.success => {
                    info!("Agent completed task {task_id}");
                    let _ = writer::append_agent_log(&task_id, &result.output);

                    let commit_msg = format!("gitzi: agent output for task {task_id}");
                    tokio::task::spawn_blocking(move || worktree.commit_all(&commit_msg))
                        .await
                        .map_err(|e| crate::error::GitziError::AgentFailed(e.to_string()))??;

                    self.orchestrator.advance_task(
                        &task_id, Stage::WaitingForReview, Some("agent completed".into()),
                    )?;
                }
                Ok(result) => {
                    warn!("Agent failed on task {task_id}: {}", result.output);
                    let _ = writer::append_agent_log(&task_id, &result.output);
                }
                Err(e) => error!("Agent error on task {task_id}: {e}"),
            }
        }

        // Run tests for tasks in testing
        let tasks = reader::load_all_tasks()?;
        for task in tasks.iter().filter(|t| t.stage == Stage::InTesting) {
            let test_root = task.branch.as_deref()
                .and_then(|_| {
                    git::open_repo(&self.repo_root).ok()
                        .and_then(|repo| TaskWorktree::open(&repo, &task.id, task.branch.as_deref().unwrap_or("")).ok())
                        .map(|wt| wt.path)
                })
                .unwrap_or_else(|| self.repo_root.clone());

            info!("Testing task {} in {}", task.id, test_root.display());
            match run_tests(&self.config, &test_root).await {
                Ok(r) if r.success => {
                    info!("Tests passed for task {}", task.id);
                    self.orchestrator.advance_task(&task.id, Stage::Done, Some("tests passed".into()))?;
                }
                Ok(r) => {
                    warn!("Tests failed for task {}: {}", task.id, &r.output[..r.output.len().min(500)]);
                    self.orchestrator.advance_task(
                        &task.id, Stage::InProgress,
                        Some(format!("tests failed: {}", &r.output[..r.output.len().min(500)])),
                    )?;
                }
                Err(e) => error!("Test runner error for task {}: {e}", task.id),
            }
        }

        Ok(())
    }
}

/// Pick the agent definition for a task: use `task.agent` if set, else the config default.
fn resolve_agent_for_task(task: &crate::model::Task, config: &Config) -> AgentDef {
    let name = task.agent.as_deref().unwrap_or(&config.default_agent);
    config.resolve_agent(name)
}
