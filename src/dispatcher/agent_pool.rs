use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{Mutex, Notify, RwLock};
use tracing::{debug, info, warn};

use super::board::KanbanBoard;
use super::event_bus::EventBus;
use super::AgentRole;
use crate::agent::{self, AgentBackend, AgentResult, RunContext};
use crate::config::Config;
use crate::dispatcher::event_bus::DispatchEvent;
use crate::git::ops;
use crate::model::task::Task;
use crate::trope_blocker;

// ─── AgentHandle ──────────────────────────────────────────────────────────────

/// A handle to a spawned agent tokio task. Used by the Dispatcher to signal,
/// query blocked state, and deliver human answers.
#[derive(Debug, Clone)]
pub struct AgentHandle {
    pub role: AgentRole,
    notify: Arc<Notify>,
    blocked: Arc<AtomicBool>,
    answer: Arc<Mutex<Option<String>>>,
}

impl AgentHandle {
    /// Create a new handle for the given role.
    fn new(role: AgentRole) -> Self {
        Self {
            role,
            notify: Arc::new(Notify::new()),
            blocked: Arc::new(AtomicBool::new(false)),
            answer: Arc::new(Mutex::new(None)),
        }
    }

    /// Wake the agent's tokio task.
    pub fn signal(&self) {
        self.notify.notify_one();
    }

    /// True if the agent is blocked waiting for a human answer.
    pub fn is_blocked(&self) -> bool {
        self.blocked.load(Ordering::Acquire)
    }

    /// Set the blocked state. Used by the pool internals and tests.
    pub fn set_blocked(&self, blocked: bool) {
        self.blocked.store(blocked, Ordering::Release);
    }

    /// Create a handle for testing purposes (not tied to a spawned task).
    pub fn new_for_test(role: AgentRole) -> Self {
        Self::new(role)
    }
}

// ─── AgentPool ────────────────────────────────────────────────────────────────

/// Manages the lifecycle of all seven agent instances.
/// Each agent is a tokio task that sleeps on its Notify until signalled.
pub struct AgentPool {
    agents: HashMap<AgentRole, AgentHandle>,
}

impl AgentPool {
    /// Create the pool and spawn agent loops as tokio tasks.
    ///
    /// Each agent loop:
    /// 1. Sleep on Notify
    /// 2. Wake → pick highest-priority task from its column
    /// 3. Run agent backend with task context (including agent_feedback if present)
    /// 4. Scan response through TropeBlocker
    /// 5. On clean: advance task to next buffer, emit AgentCompleted
    /// 6. On blocked: set blocked flag, emit AgentBlocked, sleep until unblocked
    pub fn spawn(
        event_bus: Arc<EventBus>,
        board: Arc<RwLock<KanbanBoard>>,
        config: Arc<Config>,
    ) -> Self {
        let mut agents = HashMap::new();

        for role in AgentRole::all() {
            let handle = AgentHandle::new(*role);
            agents.insert(*role, handle.clone());

            let eb = Arc::clone(&event_bus);
            let b = Arc::clone(&board);
            let cfg = Arc::clone(&config);

            tokio::spawn(agent_loop(handle, eb, b, cfg));
        }

        Self { agents }
    }

    /// Signal an agent to wake and check for work.
    pub fn signal(&self, role: AgentRole) {
        if let Some(handle) = self.agents.get(&role) {
            debug!(%role, "signalling agent");
            handle.signal();
        } else {
            warn!(%role, "signal called for unknown role");
        }
    }

    /// Check whether an agent is currently blocked waiting for a human answer.
    pub fn is_blocked(&self, role: AgentRole) -> bool {
        self.agents
            .get(&role)
            .map(|h| h.is_blocked())
            .unwrap_or(false)
    }

    /// Deliver a human answer to a blocked agent, unblock it, and signal wake.
    pub fn unblock(&self, role: AgentRole, answer: String) {
        if let Some(handle) = self.agents.get(&role) {
            // Store the answer
            let answer_slot = Arc::clone(&handle.answer);
            let notify = Arc::clone(&handle.notify);
            let blocked = Arc::clone(&handle.blocked);

            tokio::spawn(async move {
                let mut slot = answer_slot.lock().await;
                *slot = Some(answer);
                blocked.store(false, Ordering::Release);
                notify.notify_one();
            });

            debug!(%role, "unblocked agent with answer");
        } else {
            warn!(%role, "unblock called for unknown role");
        }
    }

    /// Get a reference to an agent handle by role.
    pub fn handle(&self, role: AgentRole) -> Option<&AgentHandle> {
        self.agents.get(&role)
    }
}

// ─── Agent Loop ───────────────────────────────────────────────────────────────

/// The core loop for a single agent instance. Runs as a tokio task.
async fn agent_loop(
    handle: AgentHandle,
    event_bus: Arc<EventBus>,
    board: Arc<RwLock<KanbanBoard>>,
    config: Arc<Config>,
) {
    let role = handle.role;
    let column = role.column();

    info!(%role, "agent loop started, sleeping until signalled");

    loop {
        // Sleep until signalled
        handle.notify.notified().await;

        // If blocked, don't pick up work
        if handle.is_blocked() {
            debug!(%role, "woke while blocked, going back to sleep");
            continue;
        }

        debug!(%role, "agent woke, checking for work");

        // Pick highest-priority task from our column
        let task_snapshot = {
            let b = board.read().await;
            let task_ids = b.tasks_in(column);
            if task_ids.is_empty() {
                debug!(%role, "no tasks in column, sleeping");
                continue;
            }
            // First task is highest priority (sorted by board)
            let task_id = &task_ids[0];
            b.task(task_id).cloned()
        };

        let Some(task) = task_snapshot else {
            warn!(%role, "task disappeared between check and fetch");
            continue;
        };

        info!(%role, task_id = %task.id, "processing task");

        // Build prompt context — include agent_feedback if present
        let ctx = build_run_context(&task, &config);

        // Resolve agent backend for this role
        let agent_def = config.resolve_agent(&role.to_string());
        let backend = agent::build_agent(&agent_def);

        // Run the agent backend
        let result = backend.run(&task, &ctx).await;

        match result {
            Ok(agent_result) => {
                handle_agent_result(
                    &handle,
                    &event_bus,
                    &board,
                    &task,
                    agent_result,
                    &config,
                    &ctx,
                )
                .await;
            }
            Err(e) => {
                warn!(%role, task_id = %task.id, error = %e, "agent backend error, sleeping");
                // Leave task in current column, agent sleeps until re-signaled
            }
        }
    }
}

/// Handle the result from an agent run: scan for tropes, advance or block.
async fn handle_agent_result(
    handle: &AgentHandle,
    event_bus: &Arc<EventBus>,
    board: &Arc<RwLock<KanbanBoard>>,
    task: &Task,
    agent_result: AgentResult,
    config: &Arc<Config>,
    ctx: &RunContext,
) {
    let role = handle.role;

    let output = match agent_result {
        AgentResult::Blocked { question } => {
            info!(%role, task_id = %task.id, "agent blocked with question");
            // TODO: persist review item, emit AgentBlocked, set blocked (task 6.3)
            let _ = question;
            return;
        }
        AgentResult::Failure { output } => {
            warn!(%role, task_id = %task.id, "agent reported failure, leaving task in column");
            let _ = output;
            return;
        }
        AgentResult::Success { output } => output,
    };

    // TropeBlocker scan
    match trope_blocker::scan(&output) {
        trope_blocker::ScanResult::Clean => {
            // Advance task to next buffer (or Done for Deploying)
            let next_col = role.column().next();
            if let Some(target) = next_col {
                let mut b = board.write().await;
                if let Err(e) = b.advance(&task.id, target) {
                    warn!(%role, task_id = %task.id, error = %e, "failed to advance task");
                    return;
                }
                info!(%role, task_id = %task.id, to = %target, "task advanced");
            }

            event_bus.emit(DispatchEvent::AgentCompleted {
                task_id: task.id.clone(),
                agent_role: role,
            });
        }
        trope_blocker::ScanResult::Blocked(trope_match) => {
            // Estimate context tokens (rough: 4 chars per token)
            let token_estimate = output.len() / 4;
            let summary = format!("Task: {} — {}", task.id, task.title);

            let directive =
                trope_blocker::execute(&trope_match, token_estimate, &summary);

            match directive {
                trope_blocker::Directive::Continue { injection } => {
                    debug!(%role, task_id = %task.id, "trope blocked — injecting correction");
                    // Re-run agent with injection as context
                    // For now, log the injection. Full retry loop is a follow-up task.
                    let agent_def = config.resolve_agent(&role.to_string());
                    let backend = agent::build_agent(&agent_def);
                    // TODO: inject correction into context and retry (task 8.1)
                    let _ = injection;
                    let _ = backend;
                    let _ = ctx;
                }
                trope_blocker::Directive::RotateSession { summary } => {
                    debug!(%role, task_id = %task.id, "trope blocked — rotate session");
                    // TODO: session rotation (task 8.1)
                    let _ = summary;
                }
            }
        }
    }
}

/// Build the RunContext for an agent invocation.
/// Includes agent_feedback in the branch field as a signal (the actual prompt
/// injection happens in the agent backend using the task's agent_feedback field).
fn build_run_context(task: &Task, _config: &Config) -> RunContext {
    let branch = task
        .branch
        .clone()
        .unwrap_or_else(|| task.branch_name());

    let resume_summary = resume_context(task);

    RunContext {
        repo_root: std::path::PathBuf::from("."),
        branch,
        resume_summary,
    }
}

/// Inspect existing branch state for a task that may have been in-progress before
/// a restart. Returns `Some(summary)` with commit log and diff stats if the branch
/// exists and has commits, or `None` if no recoverable state is found.
///
/// Requirements: 13.1, 13.2, 13.3, 13.4
fn resume_context(task: &Task) -> Option<String> {
    let branch_name = task.branch.as_deref()?;

    let repo_root = Path::new(".");
    let repo = match ops::open_repo(repo_root) {
        Ok(r) => r,
        Err(e) => {
            warn!(task_id = %task.id, error = %e, "failed to open repo for resume inspection");
            return None;
        }
    };

    // Check if the branch exists
    let branch_ref = format!("refs/heads/{branch_name}");
    let reference = match repo.find_reference(&branch_ref) {
        Ok(r) => r,
        Err(_) => {
            warn!(
                task_id = %task.id,
                branch = %branch_name,
                "branch not found — no recoverable state, restarting task from beginning of stage"
            );
            return None;
        }
    };

    // Get commit log (last 5 commits on this branch)
    let branch_commit = match reference.peel_to_commit() {
        Ok(c) => c,
        Err(_) => {
            warn!(task_id = %task.id, branch = %branch_name, "branch ref does not point to a commit");
            return None;
        }
    };

    let mut revwalk = match repo.revwalk() {
        Ok(rw) => rw,
        Err(_) => return None,
    };
    if revwalk.push(branch_commit.id()).is_err() {
        return None;
    }

    let mut commits: Vec<String> = Vec::new();
    for oid in revwalk.take(5).flatten() {
        if let Ok(commit) = repo.find_commit(oid) {
            let short_id = &commit.id().to_string()[..7];
            let message = commit.summary().unwrap_or(None).unwrap_or("(no message)");
            commits.push(format!("  {short_id} {message}"));
        }
    }

    if commits.is_empty() {
        warn!(task_id = %task.id, branch = %branch_name, "branch exists but has no commits");
        return None;
    }

    // Get diff stats (files changed between merge-base and branch tip)
    let diff_stats = match get_diff_stats(&repo, branch_name) {
        Some(s) => s,
        None => "  (unable to compute diff stats)".to_string(),
    };

    let summary = format!(
        "Branch: {branch_name}\nRecent commits:\n{commits}\nDiff stats:\n{diff_stats}",
        commits = commits.join("\n"),
    );

    info!(task_id = %task.id, branch = %branch_name, "boot resume: found recoverable work state");
    Some(summary)
}

/// Get a short diff stat summary (files changed, insertions, deletions) between
/// the merge-base and the branch tip.
fn get_diff_stats(repo: &git2::Repository, branch_name: &str) -> Option<String> {
    let branch_ref = format!("refs/heads/{branch_name}");
    let branch_commit = repo.find_reference(&branch_ref).ok()?.peel_to_commit().ok()?;

    // Find merge base with HEAD (main branch)
    let head_oid = repo.head().ok()?.target()?;
    let merge_base = repo.merge_base(head_oid, branch_commit.id()).ok()?;

    let base_tree = repo.find_commit(merge_base).ok()?.tree().ok()?;
    let branch_tree = branch_commit.tree().ok()?;

    let diff = repo
        .diff_tree_to_tree(Some(&base_tree), Some(&branch_tree), None)
        .ok()?;
    let stats = diff.stats().ok()?;

    Some(format!(
        "  {} file(s) changed, {} insertions(+), {} deletions(-)",
        stats.files_changed(),
        stats.insertions(),
        stats.deletions(),
    ))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_handle_signal_does_not_panic() {
        let handle = AgentHandle::new(AgentRole::Coder);
        handle.signal(); // Should not panic
    }

    #[test]
    fn agent_handle_blocked_starts_false() {
        let handle = AgentHandle::new(AgentRole::Designer);
        assert!(!handle.is_blocked());
    }

    #[test]
    fn agent_handle_blocked_state_toggle() {
        let handle = AgentHandle::new(AgentRole::Tester);
        assert!(!handle.is_blocked());
        handle.blocked.store(true, Ordering::Release);
        assert!(handle.is_blocked());
        handle.blocked.store(false, Ordering::Release);
        assert!(!handle.is_blocked());
    }

    #[tokio::test]
    async fn agent_pool_spawn_creates_all_roles() {
        let event_bus = Arc::new(EventBus::new(16));
        let board = Arc::new(RwLock::new(KanbanBoard::from_tasks(vec![])));
        let config = Arc::new(Config::default());

        let pool = AgentPool::spawn(event_bus, board, config);

        for role in AgentRole::all() {
            assert!(pool.handle(*role).is_some(), "missing handle for {role}");
        }
    }

    #[tokio::test]
    async fn agent_pool_is_blocked_false_by_default() {
        let event_bus = Arc::new(EventBus::new(16));
        let board = Arc::new(RwLock::new(KanbanBoard::from_tasks(vec![])));
        let config = Arc::new(Config::default());

        let pool = AgentPool::spawn(event_bus, board, config);

        for role in AgentRole::all() {
            assert!(!pool.is_blocked(*role));
        }
    }

    #[tokio::test]
    async fn agent_pool_unblock_stores_answer() {
        let event_bus = Arc::new(EventBus::new(16));
        let board = Arc::new(RwLock::new(KanbanBoard::from_tasks(vec![])));
        let config = Arc::new(Config::default());

        let pool = AgentPool::spawn(event_bus, board, config);

        // Simulate blocked state
        let handle = pool.handle(AgentRole::Coder).unwrap();
        handle.blocked.store(true, Ordering::Release);
        assert!(pool.is_blocked(AgentRole::Coder));

        // Unblock with answer
        pool.unblock(AgentRole::Coder, "yes, proceed".to_string());

        // Give the spawned task time to execute
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        assert!(!pool.is_blocked(AgentRole::Coder));
        let answer = handle.answer.lock().await;
        assert_eq!(answer.as_deref(), Some("yes, proceed"));
    }

    #[tokio::test]
    async fn signal_unknown_role_does_not_panic() {
        let event_bus = Arc::new(EventBus::new(16));
        let board = Arc::new(RwLock::new(KanbanBoard::from_tasks(vec![])));
        let config = Arc::new(Config::default());

        let pool = AgentPool::spawn(event_bus, board, config);
        // All roles exist, so this just tests the method doesn't panic
        pool.signal(AgentRole::Infrarian);
    }
}
