use std::sync::Arc;
use std::time::Duration;
use tokio::time;
use tracing::{info, warn, error};
use crate::agent::{AgentBackend, ClaudeCodeCli, RunContext};
use crate::config::Config;
use crate::error::Result;
use crate::git::ops as git;
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
    pub fn new(orchestrator: Arc<Orchestrator>, config: Arc<Config>, repo_root: std::path::PathBuf) -> Self {
        Self {
            orchestrator,
            config,
            repo_root,
            interval: Duration::from_secs(10),
        }
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

        // Dispatch next prioritized task if WIP allows
        if let Some(task) = self.orchestrator.pick_next_task(&tasks) {
            let task = task.clone();
            info!("Dispatching task {} to agent", task.id);

            let branch = task.branch_name();
            let repo = git::open_repo(&self.repo_root)?;
            if let Err(e) = git::create_branch(&repo, &branch) {
                warn!("Branch {} may already exist: {e}", branch);
            }

            self.orchestrator.set_task_branch(&task.id, &branch)?;
            self.orchestrator.advance_task(&task.id, Stage::InProgress, None)?;

            let agent = ClaudeCodeCli;
            let ctx = RunContext {
                repo_root: self.repo_root.clone(),
                branch: branch.clone(),
            };

            match agent.run(&task, &ctx).await {
                Ok(result) => {
                    info!("Agent completed task {}: success={}", task.id, result.success);
                    if result.success {
                        self.orchestrator.advance_task(
                            &task.id,
                            Stage::WaitingForReview,
                            Some("agent completed".to_string()),
                        )?;
                    } else {
                        warn!("Agent failed on task {}: {}", task.id, result.output);
                    }
                }
                Err(e) => {
                    error!("Agent error on task {}: {e}", task.id);
                }
            }
        }

        // Check tasks in testing
        let tasks = reader::load_all_tasks(&self.repo_root)?;
        for task in tasks.iter().filter(|t| t.stage == Stage::InTesting) {
            info!("Running tests for task {}", task.id);
            match run_tests(&self.config, &self.repo_root).await {
                Ok(result) if result.success => {
                    info!("Tests passed for task {}", task.id);
                    self.orchestrator.advance_task(&task.id, Stage::Done, Some("tests passed".to_string()))?;
                }
                Ok(result) => {
                    warn!("Tests failed for task {}: {}", task.id, result.output);
                    self.orchestrator.advance_task(
                        &task.id,
                        Stage::InProgress,
                        Some(format!("tests failed: {}", result.output)),
                    )?;
                }
                Err(e) => {
                    error!("Test runner error for task {}: {e}", task.id);
                }
            }
        }

        Ok(())
    }
}
