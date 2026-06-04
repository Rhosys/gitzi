use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use tracing::{info, warn, error};
use crate::agent::{AgentBackend, ClaudeCodeCli, RunContext};
use crate::config::Config;
use crate::error::Result;
use crate::git::ops::{self as git, TaskWorktree};
use crate::model::Stage;
use crate::pipeline::orchestrator::Orchestrator;
use crate::runner::run_tests;
use crate::state::reader;

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
        let tasks = reader::load_all_tasks(&self.repo_root)?;

        if let Some(task) = self.orchestrator.pick_next_task(&tasks) {
            let task = task.clone();
            let branch = task.branch_name();
            info!("Dispatching task {} (branch: {branch})", task.id);

            // Create a worktree for the task branch. The main workspace stays
            // untouched — the agent works entirely inside the worktree directory.
            let repo = tokio::task::spawn_blocking({
                let root = self.repo_root.clone();
                let branch = branch.clone();
                move || -> Result<()> {
                    let repo = git::open_repo(&root)?;
                    TaskWorktree::create(&repo, &branch)?;
                    Ok(())
                }
            })
            .await
            .map_err(|e| crate::error::GitziError::AgentFailed(e.to_string()))??;

            // Retrieve the worktree path for the agent subprocess.
            let worktree = {
                let repo = git::open_repo(&self.repo_root)?;
                TaskWorktree::open(&repo, &branch)?
            };

            self.orchestrator.set_task_branch(&task.id, &branch)?;
            self.orchestrator.advance_task(&task.id, Stage::InProgress, None)?;

            let agent = ClaudeCodeCli;
            let ctx = RunContext {
                repo_root: worktree.path.clone(),
                branch: branch.clone(),
            };

            match agent.run(&task, &ctx).await {
                Ok(result) if result.success => {
                    info!("Agent completed task {}", task.id);
                    // Commit agent output inside the worktree (object write —
                    // main workspace index is never touched).
                    let worktree_path = worktree.path.clone();
                    let commit_msg = format!("gitzi: agent output for task {}", task.id);
                    tokio::task::spawn_blocking(move || worktree.commit_all(&commit_msg))
                        .await
                        .map_err(|e| crate::error::GitziError::AgentFailed(e.to_string()))??;

                    self.orchestrator.advance_task(
                        &task.id,
                        Stage::WaitingForReview,
                        Some("agent completed".into()),
                    )?;
                }
                Ok(result) => {
                    warn!("Agent failed on task {}: {}", task.id, result.output);
                }
                Err(e) => {
                    error!("Agent error on task {}: {e}", task.id);
                }
            }
        }

        // Check tasks in testing — run tests inside the task's worktree so the
        // main workspace is not polluted by intermediate build artifacts.
        let tasks = reader::load_all_tasks(&self.repo_root)?;
        for task in tasks.iter().filter(|t| t.stage == Stage::InTesting) {
            let test_root = task.branch.as_deref().map(|b| {
                let repo = git::open_repo(&self.repo_root).ok()?;
                let wt = TaskWorktree::open(&repo, b).ok()?;
                Some(wt.path)
            }).flatten().unwrap_or_else(|| self.repo_root.clone());

            info!("Running tests for task {} in {}", task.id, test_root.display());
            match run_tests(&self.config, &test_root).await {
                Ok(result) if result.success => {
                    info!("Tests passed for task {}", task.id);
                    self.orchestrator.advance_task(
                        &task.id, Stage::Done, Some("tests passed".into()),
                    )?;
                }
                Ok(result) => {
                    warn!("Tests failed for task {}: {}", task.id, result.output);
                    self.orchestrator.advance_task(
                        &task.id,
                        Stage::InProgress,
                        Some(format!("tests failed: {}", &result.output[..result.output.len().min(500)])),
                    )?;
                }
                Err(e) => error!("Test runner error for task {}: {e}", task.id),
            }
        }

        Ok(())
    }
}
