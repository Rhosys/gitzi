pub mod agent_pool;
pub mod board;
pub mod event_bus;
pub mod review_queue;

use std::collections::HashMap;
use std::sync::Arc;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{info, warn};

use crate::agent::{build_main_agent, ChatTurn, MainAgent, OaiMessage};
use crate::config::Config;
use crate::mcp::auth::TokenStore;
use crate::state::chat::{self as chat_store, ChatMessage, Role};
use crate::state::home;
use crate::state::review::{PersistedReviewItem, PersistedReviewKind, ReviewAction};
use crate::state::store::{FileStore, StateStore};

use self::agent_pool::AgentPool;
use self::board::{KanbanBoard, WipLimits};
use self::event_bus::{DispatchEvent, EventBus};
use self::review_queue::HumanReviewQueue;

/// A named stage on the Kanban board. Columns are ordered from intake (Prioritized)
/// through delivery (Done), with buffer columns between every work stage requiring
/// human approval before advancement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Column {
    Prioritized,
    Designing,
    CodingBuffer,
    Coding,
    ReviewBuffer,
    Reviewing,
    SecurityAuditBuffer,
    Auditing,
    DeploymentBuffer,
    Deploying,
    Done,
}

static ALL_COLUMNS: &[Column] = &[
    Column::Prioritized,
    Column::Designing,
    Column::CodingBuffer,
    Column::Coding,
    Column::ReviewBuffer,
    Column::Reviewing,
    Column::SecurityAuditBuffer,
    Column::Auditing,
    Column::DeploymentBuffer,
    Column::Deploying,
    Column::Done,
];

impl Column {
    /// All columns in pipeline order.
    pub fn all() -> &'static [Column] {
        ALL_COLUMNS
    }

    /// The next column after this one, or None if Done.
    pub fn next(&self) -> Option<Column> {
        let idx = ALL_COLUMNS.iter().position(|c| c == self)?;
        ALL_COLUMNS.get(idx + 1).copied()
    }

    /// The previous column before this one, or None if Prioritized.
    pub fn prev(&self) -> Option<Column> {
        let idx = ALL_COLUMNS.iter().position(|c| c == self)?;
        if idx == 0 { None } else { ALL_COLUMNS.get(idx - 1).copied() }
    }

    /// True for buffer columns (require human approval to advance).
    pub fn is_buffer(&self) -> bool {
        matches!(
            self,
            Column::CodingBuffer
                | Column::ReviewBuffer
                | Column::SecurityAuditBuffer
                | Column::DeploymentBuffer
        )
    }

    /// The agent role that works this column, if it's a work column.
    pub fn agent_role(&self) -> Option<AgentRole> {
        match self {
            Column::Prioritized => Some(AgentRole::Prioritizer),
            Column::Designing => Some(AgentRole::Designer),
            Column::Coding => Some(AgentRole::Coder),
            Column::Reviewing => Some(AgentRole::Reviewer),
            Column::Auditing => Some(AgentRole::Auditor),
            Column::Deploying => Some(AgentRole::Infrarian),
            _ => None,
        }
    }
}

impl std::fmt::Display for Column {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Column::Prioritized => "prioritized",
            Column::Designing => "designing",
            Column::CodingBuffer => "coding-buffer",
            Column::Coding => "coding",
            Column::ReviewBuffer => "review-buffer",
            Column::Reviewing => "reviewing",
            Column::SecurityAuditBuffer => "security-audit-buffer",
            Column::Auditing => "auditing",
            Column::DeploymentBuffer => "deployment-buffer",
            Column::Deploying => "deploying",
            Column::Done => "done",
        };
        write!(f, "{s}")
    }
}

/// One of the six LLM agent roles. Each role has exactly one agent instance
/// and maps to a specific work column on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentRole {
    Prioritizer,
    Designer,
    Coder,
    Reviewer,
    Auditor,
    Infrarian,
}

static ALL_ROLES: &[AgentRole] = &[
    AgentRole::Prioritizer,
    AgentRole::Designer,
    AgentRole::Coder,
    AgentRole::Reviewer,
    AgentRole::Auditor,
    AgentRole::Infrarian,
];

impl AgentRole {
    /// All agent roles.
    pub fn all() -> &'static [AgentRole] {
        ALL_ROLES
    }

    /// The work column this role processes.
    pub fn column(&self) -> Column {
        match self {
            AgentRole::Prioritizer => Column::Prioritized,
            AgentRole::Designer => Column::Designing,
            AgentRole::Coder => Column::Coding,
            AgentRole::Reviewer => Column::Reviewing,
            AgentRole::Auditor => Column::Auditing,
            AgentRole::Infrarian => Column::Deploying,
        }
    }
}

impl AgentRole {
    /// Hardcoded default AgentDef for this role.
    pub fn default_agent_def(&self) -> crate::config::AgentDef {
        crate::config::AgentDef {
            role: self.to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            api_url: None,
            provider: None,
        }
    }

    /// Hardcoded default system prompt for this role.
    pub fn default_system_prompt(&self) -> &'static str {
        match self {
            AgentRole::Prioritizer => "You break epics into minimal, independently-shippable tasks ordered by dependency and value.",
            AgentRole::Designer => "You produce concise technical designs. No code — architecture, data models, interfaces only.",
            AgentRole::Coder => "You are a disciplined coding agent. Make the smallest possible change. No refactoring, no extras.",
            AgentRole::Reviewer => "\
You are an adversarial code auditor. Your job is to find everything wrong with the \
coder's work. Assume the coder attempted to:\n\
- Build the most wrong implementation possible for the task\n\
- Build too much code (scope creep, unnecessary abstractions)\n\
- Duplicate logic instead of reusing existing code\n\
- Change existing code in ways incompatible with its original contract\n\
- Use wrong architecture for the problem\n\
- Leave subtle bugs that will explode in production\n\
- Leave dead code, unused imports, commented-out blocks\n\
- Add defensive \"just in case\" handling for scenarios no one cares about\n\n\
YOUR PROCESS:\n\
1. Read the task description and epic context — understand WHAT was asked.\n\
2. Read the git diff — understand WHAT was changed.\n\
3. Read the full files touched — check compatibility with existing code.\n\
4. Run tests and lint — confirm they pass (if not, that's finding #1).\n\
5. Check: do the tests actually validate the task's requirements? Or do they test \
the wrong thing?\n\
6. Check: does the change fit the epic's goal? Or did the coder go off-track?\n\
7. Report ALL findings as a structured list of problems.\n\
8. If problems found: reject the task with specific, actionable feedback.\n\
9. If clean: approve (respond with text, no tool calls).\n\n\
NEVER approve work that has problems. Be ruthless. The coder can handle it.",
            AgentRole::Auditor => "You perform security audits. Check for vulnerabilities, leaked secrets, unsafe patterns.",
            AgentRole::Infrarian => "You manage deployment infrastructure. Minimal, reproducible, observable.",
        }
    }
}

impl std::fmt::Display for AgentRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            AgentRole::Prioritizer => "prioritizer",
            AgentRole::Designer => "designer",
            AgentRole::Coder => "coder",
            AgentRole::Reviewer => "reviewer",
            AgentRole::Auditor => "auditor",
            AgentRole::Infrarian => "infrarian",
        };
        write!(f, "{s}")
    }
}

// ─── Dispatcher ───────────────────────────────────────────────────────────────

/// Central orchestration struct. Owns the event bus, board projection,
/// review queue, agent pool, WIP limits, and the main chat harness.
pub struct Dispatcher {
    pub event_bus: Arc<EventBus>,
    pub board: Arc<RwLock<KanbanBoard>>,
    pub review_queue: Arc<Mutex<HumanReviewQueue>>,
    pub agent_pool: AgentPool,
    pub config: Arc<Config>,
    pub wip_limits: Arc<RwLock<WipLimits>>,
    /// Agents waiting to advance into a full column.
    pub wip_waiting: Arc<Mutex<HashMap<Column, AgentRole>>>,
    /// Persistent chat history (in-memory mirror of chat.jsonl).
    pub chat_history: Arc<Mutex<Vec<ChatMessage>>>,
    /// The main coordination agent that drives the chat interface.
    pub main_agent: MainAgent,
    /// Token store for MCP sub-agent authorization.
    pub token_store: Arc<TokenStore>,
    /// Persistence layer for tasks, epics, and review items.
    pub store: Arc<dyn StateStore>,
    /// Tracks the currently in-flight chat turn for interrupt classification.
    /// A stack — each fork pushes a new entry. Classifier operates against the top.
    pub chat_stack: Mutex<Vec<ForkEntry>>,
}

/// A single entry on the fork stack — represents an active conversation branch.
pub struct ForkEntry {
    /// Unique ID for this fork.
    pub id: String,
    /// Short topic name (2-4 words, generated by LLM).
    pub name: String,
    /// The user message that created this fork (used for interrupt classification).
    pub pending_message: String,
    /// Abort handle to cancel the in-flight LLM request.
    pub abort_handle: tokio::task::AbortHandle,
    /// Whether this fork has a pending LLM turn (vs waiting for user input).
    pub turn_active: bool,
    /// Isolated chat history for this fork (seeded from the last 50 main entries).
    pub fork_history: Vec<crate::state::chat::ChatMessage>,
}

/// Context for a chat turn — determines which history and persistence path to use.
pub struct ChatContext {
    /// Which chat history to use for this turn.
    pub history: Vec<ChatMessage>,
    /// Where to persist messages from this turn.
    pub persist_path: std::path::PathBuf,
    /// Fork ID (or "main" for the main session).
    pub fork_id: String,
}

impl Dispatcher {
    /// Approve a task in a buffer column: advance to the next work column,
    /// record history, emit event, and signal the agent for the target column.
    pub async fn approve(&self, task_id: &str) -> anyhow::Result<()> {
        let next_col = {
            let board = self.board.read().await;
            let current_col = board.column_of(task_id)
                .ok_or_else(|| anyhow::anyhow!("task '{task_id}' not found on board"))?;
            if !current_col.is_buffer() {
                anyhow::bail!("task '{task_id}' is in {current_col}, not a buffer column");
            }
            current_col.next()
                .ok_or_else(|| anyhow::anyhow!("buffer column {current_col} has no next column"))?
        };

        // Write to board: advance task and record history
        {
            let mut board = self.board.write().await;
            board.advance(task_id, next_col)?;
            let task = board.task_mut(task_id)
                .ok_or_else(|| anyhow::anyhow!("task '{task_id}' disappeared after advance"))?;
            task.history.push(crate::model::task::HistoryEntry::Approval {
                at: chrono::Utc::now(),
                target_stage: next_col.into(),
            });
        }

        // Persist task to disk
        {
            let board = self.board.read().await;
            if let Some(task) = board.task(task_id)
                && let Err(e) = self.store.write_task(task) {
                warn!(%task_id, error = %e, "failed to persist task after approval");
            }
        }

        // Persist review item with approval action appended
        if let Ok(Some(mut review_item)) = self.store.find_unresolved_for_task(task_id) {
            review_item.actions.push(ReviewAction::Approval {
                at: chrono::Utc::now(),
            });
            if let Err(e) = self.store.write_review_item(&review_item) {
                warn!(%task_id, error = %e, "failed to persist review item approval");
            }
        }

        // Remove the resolved item from the in-memory queue so it stops being
        // surfaced to the TUI and main agent.
        self.review_queue.lock().await.dequeue_by_task_id(task_id);

        // Emit event
        self.event_bus.emit(DispatchEvent::HumanApprovalReceived {
            task_id: task_id.to_string(),
            target_column: next_col,
        });

        // Signal the agent for the target work column
        if let Some(role) = next_col.agent_role() {
            self.agent_pool.signal(role);
        }

        Ok(())
    }

    /// Send a task in a buffer column back for rework: move to previous work column
    /// with priority 0, store feedback, record history, emit event, and signal the
    /// agent. Invoked via the main agent's `gitzi_request_rework` tool once the
    /// human and main agent have converged on what feedback to send.
    pub async fn reject(&self, task_id: &str, feedback: String) -> anyhow::Result<()> {
        // Default: send back to the previous column (one step back)
        let target_col = {
            let board = self.board.read().await;
            let current_col = board.column_of(task_id)
                .ok_or_else(|| anyhow::anyhow!("task '{task_id}' not found on board"))?;
            if !current_col.is_buffer() {
                anyhow::bail!("task '{task_id}' is in {current_col}, not a buffer column");
            }
            current_col.prev()
                .ok_or_else(|| anyhow::anyhow!("buffer column {current_col} has no previous column"))?
        };
        self.reject_to(task_id, target_col, feedback).await
    }

    /// Send a task back to a specific target work column with feedback.
    /// Used when the human decides to skip intermediate columns (e.g. ReviewBuffer → Coding
    /// instead of ReviewBuffer → Reviewing, for "review valid, send back to coder").
    pub async fn reject_to(&self, task_id: &str, target_col: Column, feedback: String) -> anyhow::Result<()> {
        // Validate: task must be in a buffer column
        {
            let board = self.board.read().await;
            let current_col = board.column_of(task_id)
                .ok_or_else(|| anyhow::anyhow!("task '{task_id}' not found on board"))?;
            if !current_col.is_buffer() {
                anyhow::bail!("task '{task_id}' is in {current_col}, not a buffer column");
            }
        }

        // Validate: target must be a work column (has an agent role)
        if target_col.agent_role().is_none() {
            anyhow::bail!("target column {target_col} is not a work column");
        }

        // Write to board: advance to target, set priority, store feedback, record history
        {
            let mut board = self.board.write().await;
            board.advance(task_id, target_col)?;
            board.set_priority(task_id, 0);
            let task = board.task_mut(task_id)
                .ok_or_else(|| anyhow::anyhow!("task '{task_id}' disappeared after advance"))?;
            task.agent_feedback = Some(feedback.clone());
            task.history.push(crate::model::task::HistoryEntry::Rejection {
                at: chrono::Utc::now(),
                feedback: feedback.clone(),
                returned_to: target_col.into(),
            });
        }

        // Persist task to disk
        {
            let board = self.board.read().await;
            if let Some(task) = board.task(task_id)
                && let Err(e) = self.store.write_task(task) {
                warn!(%task_id, error = %e, "failed to persist task after rejection");
            }
        }

        // Persist review item with rejection action appended
        if let Ok(Some(mut review_item)) = self.store.find_unresolved_for_task(task_id) {
            review_item.actions.push(ReviewAction::Rejection {
                at: chrono::Utc::now(),
                feedback: feedback.clone(),
            });
            if let Err(e) = self.store.write_review_item(&review_item) {
                warn!(%task_id, error = %e, "failed to persist review item rejection");
            }
        }

        // Remove the resolved item from the in-memory queue so it stops being
        // surfaced to the TUI and main agent.
        self.review_queue.lock().await.dequeue_by_task_id(task_id);

        // Emit event
        self.event_bus.emit(DispatchEvent::HumanRejectionReceived {
            task_id: task_id.to_string(),
            returned_to: target_col,
            feedback,
        });

        // Signal the agent for the target work column
        if let Some(role) = target_col.agent_role() {
            self.agent_pool.signal(role);
        }

        Ok(())
    }

    /// Attempt to merge a completed task's branch into its target branch.
    async fn attempt_merge(&self, task_id: &str) {
        use crate::git::ops::{merge_task_branch, MergeOutcome};

        let task = match self.store.load_task(task_id) {
            Ok(t) => t,
            Err(e) => {
                warn!(%task_id, error = %e, "cannot load task for merge");
                return;
            }
        };

        let branch = match &task.branch {
            Some(b) => b.clone(),
            None => {
                info!(%task_id, "task has no branch — skipping merge");
                return;
            }
        };

        let repo_path = home::repo_path();
        let repo_path_str = repo_path.to_string_lossy().to_string();

        let repo_config = self.config.repo_config(&repo_path_str);

        match merge_task_branch(
            &repo_path,
            &branch,
            &repo_config.main_branch,
            &repo_config.merge_strategy,
        ) {
            Ok(MergeOutcome::Merged) => {
                info!(
                    %task_id,
                    branch = %branch,
                    "merged into {}",
                    repo_config.main_branch
                );
                crate::state::repo_cache::increment_commits(
                    repo_path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("default")
                );
            }
            Ok(MergeOutcome::Skipped(reason)) => {
                info!(%task_id, %reason, "merge skipped");
            }
            Ok(MergeOutcome::FfFailed(reason)) => {
                warn!(%task_id, %reason, "ff-merge failed — creating review item");
                let item = review_queue::HumanReviewItem::new(
                    task_id,
                    review_queue::ReviewItemKind::AgentQuestion {
                        question: format!(
                            "Task '{}' is done but cannot be fast-forward \
                             merged: {}. What should I do? (rebase the \
                             branch, force merge, or leave it)",
                            task.title, reason
                        ),
                    },
                );
                let mut q = self.review_queue.lock().await;
                q.enqueue(item);
            }
            Err(e) => {
                warn!(%task_id, error = %e, "merge attempt failed");
            }
        }
    }

    /// Answer an agent's question: dequeue the review item, persist the answer on the
    /// review item, and unblock the waiting agent.
    pub async fn answer_question(&self, item_id: &str, answer: String) -> anyhow::Result<()> {
        // Dequeue the item from the in-memory review queue
        let item = {
            let mut queue = self.review_queue.lock().await;
            queue.dequeue(item_id)
                .ok_or_else(|| anyhow::anyhow!("review item '{item_id}' not found in queue"))?
        };

        // Validate it's a question, not a buffer approval
        match &item.kind {
            review_queue::ReviewItemKind::AgentQuestion { .. } => {}
            review_queue::ReviewItemKind::BufferApproval { .. } => {
                anyhow::bail!("review item '{item_id}' is a buffer approval, not a question");
            }
        }

        // Persist the answer on the review item (not the task)
        match self.store.load_review_item(item_id) {
            Ok(mut persisted) => {
                persisted.actions.push(ReviewAction::Answer {
                    at: chrono::Utc::now(),
                    content: answer.clone(),
                });
                if let Err(e) = self.store.write_review_item(&persisted) {
                    warn!(%item_id, error = %e, "failed to persist answer to review item");
                }
            }
            Err(e) => {
                warn!(%item_id, error = %e, "failed to load review item for answer persistence");
            }
        }

        // Determine which agent role is blocked (read-only board access)
        let role = {
            let board = self.board.read().await;
            let task = board.task(&item.task_id)
                .ok_or_else(|| anyhow::anyhow!("task '{}' not found on board", item.task_id))?;
            let col = task.stage.to_column();
            col.agent_role()
                .ok_or_else(|| anyhow::anyhow!("task '{}' is in column {col} which has no agent", item.task_id))?
        };

        // Unblock the agent with the answer
        self.agent_pool.unblock(role, answer);

        Ok(())
    }

    // ── gitzi_ management tools ───────────────────────────────────────────────

    pub async fn gitzi_list_epics(&self) -> anyhow::Result<Vec<crate::model::Epic>> {
        Ok(self.store.load_all_epics()?)
    }

    pub async fn gitzi_list_tasks(&self, epic_id: Option<&str>) -> anyhow::Result<Vec<crate::model::Task>> {
        let all = self.store.load_all_tasks()?;
        if let Some(id) = epic_id {
            Ok(all.into_iter().filter(|t| t.epic == id).collect())
        } else {
            Ok(all)
        }
    }

    pub async fn gitzi_get_review_item(&self, id: &str) -> anyhow::Result<PersistedReviewItem> {
        Ok(self.store.load_review_item(id)?)
    }

    pub async fn gitzi_create_epic(
        &self,
        title: String,
        description: Option<String>,
    ) -> anyhow::Result<crate::model::Epic> {
        let id = crate::id::new_id(&title);
        let mut epic = crate::model::Epic::new(&id, &title);
        epic.description = description;
        self.store.write_epic(&epic)?;
        Ok(epic)
    }

    pub async fn gitzi_create_task(
        &self,
        epic_id: String,
        title: String,
        description: Option<String>,
        priority: Option<u32>,
        repo: Option<String>,
    ) -> anyhow::Result<crate::model::Task> {
        let id = crate::id::new_id(&title);
        let mut task = crate::model::Task::new(&id, &epic_id, &title);
        task.description = description;
        task.priority = priority.unwrap_or(100);
        task.repo = repo;
        self.store.write_task(&task)?;

        // Update parent epic's task list on disk
        if let Ok(mut epic) = self.store.load_epic(&epic_id) {
            epic.tasks.push(id.clone());
            if let Err(e) = self.store.write_epic(&epic) {
                warn!(%epic_id, error = %e, "failed to update epic task list after create_task");
            }
        }

        // Add to in-memory board and signal the prioritizer
        {
            let mut board = self.board.write().await;
            board.add_task(task.clone());
        }
        self.event_bus.emit(DispatchEvent::TaskCreated { task_id: id.clone() });

        Ok(task)
    }

    pub async fn gitzi_update_task(
        &self,
        task_id: &str,
        title: Option<String>,
        description: Option<String>,
    ) -> anyhow::Result<crate::model::Task> {
        let mut task = self.store.load_task(task_id)?;
        if let Some(t) = title { task.title = t; }
        if let Some(d) = description { task.description = Some(d); }
        task.updated_at = chrono::Utc::now();
        self.store.write_task(&task)?;

        // Sync in-memory board entry
        {
            let mut board = self.board.write().await;
            if let Some(t) = board.task_mut(task_id) {
                *t = task.clone();
            }
        }

        Ok(task)
    }

    pub async fn gitzi_prioritize_task(&self, task_id: &str, priority: u32) -> anyhow::Result<crate::model::Task> {
        let mut task = self.store.load_task(task_id)?;
        task.priority = priority;
        task.updated_at = chrono::Utc::now();
        self.store.write_task(&task)?;

        {
            let mut board = self.board.write().await;
            board.set_priority(task_id, priority);
        }

        Ok(task)
    }

    pub async fn gitzi_park_task(&self, task_id: &str, reason: String) -> anyhow::Result<()> {
        let mut task = self.store.load_task(task_id)?;
        task.resume_summary = Some(reason);
        task.updated_at = chrono::Utc::now();
        self.store.write_task(&task)?;

        // Sync in-memory board entry
        {
            let mut board = self.board.write().await;
            if let Some(t) = board.task_mut(task_id) {
                t.resume_summary = task.resume_summary.clone();
                t.updated_at = task.updated_at;
            }
        }

        Ok(())
    }

    pub async fn gitzi_switch_panel(&self, view: String) {
        self.event_bus.emit(DispatchEvent::PanelSwitch { view });
    }

    pub async fn gitzi_create_review_item(
        &self,
        task_id: String,
        question: String,
        context: Option<String>,
    ) -> anyhow::Result<PersistedReviewItem> {
        let full_question = if let Some(ctx) = context {
            format!("{question}\n\nContext:\n{ctx}")
        } else {
            question
        };

        // Use the same ID for both the persisted file and the in-memory queue item
        let item_id = crate::id::new_id(&task_id);
        let now = chrono::Utc::now();

        let persisted = PersistedReviewItem {
            id: item_id.clone(),
            task_id: task_id.clone(),
            kind: PersistedReviewKind::AgentQuestion { question: full_question.clone() },
            created_at: now,
            actions: Vec::new(),
        };
        self.store.write_review_item(&persisted)?;

        // Enqueue in-memory with the same ID
        {
            let mut q = self.review_queue.lock().await;
            q.enqueue(review_queue::HumanReviewItem {
                id: item_id,
                task_id: task_id.clone(),
                kind: review_queue::ReviewItemKind::AgentQuestion { question: full_question },
                created_at: now,
            });
        }

        Ok(persisted)
    }

    /// Re-scan this machine for LLM providers and AWS Bedrock/SSO access,
    /// merging any newly-found ones into `~/.gitzi/config.toml` as disabled
    /// candidates (never auto-activated — see `bootstrap` module docs).
    /// Reads and writes the config file directly rather than `self.config`,
    /// since the latter is an immutable snapshot for the life of the daemon.
    /// Returns a human-readable summary for the main agent to relay.
    pub async fn gitzi_rediscover_providers(&self) -> anyhow::Result<String> {
        let gitzi_home = crate::state::home::gitzi_home();
        let mut config = Config::load(&gitzi_home)?;

        let discovered = crate::bootstrap::discover_providers();
        let mut added = Vec::new();
        for provider in &discovered {
            if config.providers.contains_key(&provider.name) {
                continue;
            }
            let mut def = crate::config::ProviderDef {
                kind: provider.kind,
                enabled: false,
                ..crate::config::ProviderDef::default()
            };
            match provider.kind {
                crate::config::ProviderKind::OpenaiCompatible => {
                    def.api_url = provider.api_url.clone();
                }
                crate::config::ProviderKind::Bedrock => {
                    def.region = provider.region.clone();
                    def.sso_start_url = provider.sso_start_url.clone();
                }
            }
            config.providers.insert(provider.name.clone(), def);
            added.push(provider.name.clone());
        }

        if !added.is_empty() {
            config.write(&gitzi_home)?;
            info!(added = ?added, "rediscovered new providers — merged into config.toml as disabled");
        }

        if discovered.is_empty() {
            return Ok("No LLM providers or AWS Bedrock access discovered on this machine.".to_string());
        }

        let lines: Vec<String> = discovered.iter().map(|p| {
            let enabled = config.providers.get(&p.name).map(|d| d.enabled).unwrap_or(false);
            let status = if p.model_loaded {
                "running, model loaded"
            } else if p.running {
                "running, no model loaded"
            } else if p.installed {
                "installed, not running"
            } else {
                "not installed"
            };
            let kind = match p.kind {
                crate::config::ProviderKind::OpenaiCompatible => "openai-compatible",
                crate::config::ProviderKind::Bedrock => "bedrock",
            };
            format!("- {} [{kind}] {status}, enabled={enabled}", p.name)
        }).collect();

        let added_note = if added.is_empty() {
            String::new()
        } else {
            format!("\n\nNewly discovered: {}", added.join(", "))
        };

        Ok(format!("Discovered providers:\n{}{added_note}", lines.join("\n")))
    }

    /// Activate a discovered provider so agents actually use it. For
    /// OpenAI-compatible providers this is immediate. For Bedrock, this may
    /// span multiple calls — see the `gitzi_activate_provider` tool
    /// description. Operates on `~/.gitzi/config.toml` directly; the caller
    /// must tell the user a gitzi restart is needed for the rewired agent to
    /// take effect (the running daemon's `self.config` is immutable).
    pub async fn gitzi_activate_provider(
        &self,
        name: &str,
        account_id: Option<String>,
        role_name: Option<String>,
    ) -> anyhow::Result<String> {
        let gitzi_home = crate::state::home::gitzi_home();
        let mut config = Config::load(&gitzi_home)?;

        let mut provider = config.providers.get(name).cloned().ok_or_else(|| {
            anyhow::anyhow!("no provider named '{name}' — run gitzi_rediscover_providers first")
        })?;

        if provider.kind == crate::config::ProviderKind::OpenaiCompatible {
            provider.enabled = true;
            config.providers.insert(name.to_string(), provider);
            wire_main_agent(&mut config, name);
            config.write(&gitzi_home)?;
            return Ok(format!(
                "ok: activated '{name}' and wired it into the main agent. Restart gitzi for this to take effect."
            ));
        }

        // Bedrock: AWS SSO login, then account/role selection, then validate.
        let region = provider.region.clone().ok_or_else(|| {
            anyhow::anyhow!("provider '{name}' has no AWS region configured — set one in config.toml under [providers.{name}] first")
        })?;
        let start_url = provider.sso_start_url.clone().ok_or_else(|| {
            anyhow::anyhow!("provider '{name}' has no sso_start_url configured — set one in config.toml under [providers.{name}] first")
        })?;

        let token = match crate::aws_sso::load_token(&start_url) {
            Some(t) => t,
            None => {
                let pending = crate::aws_sso::start_device_login(&region, &start_url).await?;
                info!(provider = %name, "AWS SSO device login started — waiting for user approval in browser");
                crate::aws_sso::poll_for_token(&pending, &start_url).await?
            }
        };

        let Some(account_id) = account_id else {
            let accounts = crate::aws_sso::list_accounts(&region, &token.access_token).await?;
            if accounts.is_empty() {
                anyhow::bail!("AWS SSO login succeeded but no accounts are assigned to this user");
            }
            let listing = accounts
                .iter()
                .map(|a| format!("- {} ({}) <{}>", a.account_id, a.account_name, a.email_address))
                .collect::<Vec<_>>()
                .join("\n");
            return Ok(format!(
                "AWS SSO login confirmed. Choose an account and call gitzi_activate_provider again with name='{name}' and account_id set:\n{listing}"
            ));
        };

        let Some(role_name) = role_name else {
            let roles = crate::aws_sso::list_account_roles(&region, &token.access_token, &account_id).await?;
            if roles.is_empty() {
                anyhow::bail!("no SSO roles assigned to account '{account_id}'");
            }
            let listing = roles
                .iter()
                .map(|r| format!("- {}", r.role_name))
                .collect::<Vec<_>>()
                .join("\n");
            return Ok(format!(
                "Choose a role and call gitzi_activate_provider again with name='{name}', account_id='{account_id}', and role_name set:\n{listing}"
            ));
        };

        // Validate the chosen account/role actually exchange for credentials.
        crate::aws_sso::get_role_credentials(&region, &token.access_token, &account_id, &role_name).await?;

        crate::aws_sso::write_credential_process_profile(name, name, &region)?;

        provider.enabled = true;
        provider.sso_account_id = Some(account_id.clone());
        provider.sso_role_name = Some(role_name.clone());
        provider.profile = Some(name.to_string());
        if provider.model_id.is_none() {
            provider.model_id = Some("anthropic.claude-sonnet-4-6-v1:0".to_string());
        }
        config.providers.insert(name.to_string(), provider);
        wire_main_agent(&mut config, name);
        config.write(&gitzi_home)?;

        Ok(format!(
            "ok: activated Bedrock provider '{name}' (account {account_id}, role {role_name}) and wired it into the main agent. Restart gitzi for this to take effect."
        ))
    }

    /// Boot the dispatcher: load tasks, build board, spawn agents, emit BootComplete,
    /// and signal agents whose columns contain work.
    pub async fn start(config: Config) -> anyhow::Result<Self> {
        let config = Arc::new(config);

        // 1. Load tasks from disk
        let store: Arc<dyn StateStore> = Arc::new(FileStore);
        let tasks = store.load_all_tasks()?;
        info!(task_count = tasks.len(), "loaded tasks from disk");

        // 2. Build KanbanBoard from tasks
        let board = Arc::new(RwLock::new(KanbanBoard::from_tasks(tasks)));

        // 3. Create EventBus (capacity 256)
        let event_bus = Arc::new(EventBus::new(256));

        // 4. Create empty HumanReviewQueue and load persisted unresolved items
        let mut queue = HumanReviewQueue::new();
        match store.load_all_unresolved() {
            Ok(persisted_items) => {
                for p in persisted_items {
                    let kind = match p.kind {
                        PersistedReviewKind::AgentQuestion { question } => {
                            review_queue::ReviewItemKind::AgentQuestion { question }
                        }
                        PersistedReviewKind::BufferApproval { buffer_column, task_priority } => {
                            review_queue::ReviewItemKind::BufferApproval {
                                buffer_column,
                                task_priority,
                            }
                        }
                    };
                    let item = review_queue::HumanReviewItem {
                        id: p.id,
                        task_id: p.task_id,
                        kind,
                        created_at: p.created_at,
                    };
                    queue.enqueue(item);
                }
                info!(count = queue.len(), "loaded unresolved review items from disk");
            }
            Err(e) => {
                warn!(error = %e, "failed to load persisted review items — starting with empty queue");
            }
        }
        let review_queue = Arc::new(Mutex::new(queue));

        // 5. Build WipLimits from config overrides layered onto built-in defaults.
        // `Config::load` already validated the overrides, so this can't fail in
        // practice — fall back to defaults defensively rather than panic on boot.
        let wip_limits = WipLimits::from_config(&config.wip_limits.overrides).unwrap_or_else(|e| {
            warn!(error = %e, "invalid WIP limit overrides in config — using defaults");
            WipLimits::default()
        });
        let wip_limits = Arc::new(RwLock::new(wip_limits));

        // 6. Create WIP waiting map (runtime-only, rebuilt on boot)
        let wip_waiting = Arc::new(Mutex::new(HashMap::new()));

        // 7. Spawn AgentPool
        let token_store = Arc::new(TokenStore::new());
        let agent_pool = AgentPool::spawn(
            Arc::clone(&event_bus),
            Arc::clone(&board),
            Arc::clone(&config),
            Arc::clone(&wip_limits),
            Arc::clone(&wip_waiting),
            Arc::clone(&review_queue),
            Arc::clone(&token_store),
            Arc::clone(&store),
        );

        // 8. Emit BootComplete
        event_bus.emit(DispatchEvent::BootComplete);
        info!("boot complete event emitted");

        // 9. Signal all agents that have work in their column
        {
            let b = board.read().await;
            for role in AgentRole::all() {
                let col = role.column();
                if !b.tasks_in(col).is_empty() {
                    agent_pool.signal(*role);
                    info!(%role, "signalled agent — work available");
                }
            }
        }

        // 10. Load chat history from disk
        let chat_history = {
            let path = home::current_chat_file();
            let messages = chat_store::load(&path).unwrap_or_default();
            info!(messages = messages.len(), "loaded chat history from disk");
            Arc::new(Mutex::new(messages))
        };

        // 11. Build main agent from config
        let main_agent_def = config.resolve_agent("main");
        let main_agent = build_main_agent(&main_agent_def);

        let dispatcher = Self {
            event_bus,
            board,
            review_queue,
            agent_pool,
            config,
            wip_limits,
            wip_waiting,
            chat_history,
            main_agent,
            token_store,
            store,
            chat_stack: Mutex::new(Vec::new()),
        };

        // 12. Proactively surface opening status — once per daemon boot.
        if let Err(e) = dispatcher.opening_status().await {
            warn!(error = %e, "failed to generate opening status");
        }

        Ok(dispatcher)
    }

    /// Watch `config.toml` for changes and hot-reload WIP limits without
    /// restarting the daemon — changes take effect on the next tick.
    ///
    /// Scope is intentionally narrow: only the per-column WIP overrides are
    /// swapped in. Other config fields (agent definitions, test command,
    /// etc.) keep whatever was loaded at boot, since they're captured by
    /// value at points that aren't safe to hot-swap without a larger
    /// refactor (e.g. in-flight agent backends).
    pub async fn watch_config(&self) -> anyhow::Result<()> {
        let path = home::global_config_file();
        let Some(dir) = path.parent().map(std::path::Path::to_path_buf) else {
            warn!("config file has no parent directory — config hot-reload disabled");
            return Ok(());
        };
        if !dir.exists() {
            warn!(dir = %dir.display(), "config directory does not exist — config hot-reload disabled");
            return Ok(());
        }

        let (tx, mut rx) = mpsc::unbounded_channel::<()>();
        let watch_path = path.clone();
        let mut watcher: RecommendedWatcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res
                    && event.paths.iter().any(|p| p == &watch_path)
                {
                    let _ = tx.send(());
                }
            })?;
        watcher.watch(&dir, RecursiveMode::NonRecursive)?;
        info!(path = %path.display(), "watching config.toml for changes");

        while rx.recv().await.is_some() {
            // Debounce: editors commonly emit several events (write + rename)
            // for a single save. Drain anything else that arrived meanwhile.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            while rx.try_recv().is_ok() {}

            match Config::load(std::path::Path::new(".")) {
                Ok(new_config) => {
                    match WipLimits::from_config(&new_config.wip_limits.overrides) {
                        Ok(new_limits) => {
                            *self.wip_limits.write().await = new_limits;
                            info!("config.toml changed — WIP limits reloaded");
                        }
                        Err(e) => warn!(
                            error = %e,
                            "config.toml changed but WIP limits are invalid — keeping previous limits"
                        ),
                    }
                }
                Err(e) => warn!(
                    error = %e,
                    "config.toml changed but failed to reload — keeping previous config"
                ),
            }
        }

        Ok(())
    }

    /// Process a user chat message through the main agent tool calling loop.
    ///
    /// On every turn we first peek the review queue. If an item is pending we
    /// switch the side panel to the review view and inject queue context into
    /// the LLM message so the agent surfaces it. Only the original user message
    /// (without injected context) is persisted to chat history.
    pub async fn chat(&self, message: &str, ctx: Option<ChatContext>) -> anyhow::Result<String> {
        // Capture which buffer-approval item (if any) is under discussion before
        // the turn runs, so we record this exchange against that item's structured
        // history even if the turn itself resolves and dequeues it.
        let discussed_task_id = {
            let queue = self.review_queue.lock().await;
            queue.peek().and_then(|item| match item.kind {
                review_queue::ReviewItemKind::BufferApproval { .. } => Some(item.task_id.clone()),
                review_queue::ReviewItemKind::AgentQuestion { .. } => None,
            })
        };

        let history = if let Some(ref ctx) = ctx {
            ctx.history.clone()
        } else {
            self.chat_history.lock().await.clone()
        };
        let final_response = self.run_main_agent_turn(message, &history).await?;

        // Persist the original user message (not augmented) and the agent response
        let persist_path = ctx.as_ref()
            .map(|c| c.persist_path.clone())
            .unwrap_or_else(home::current_chat_file);

        if ctx.is_none() || ctx.as_ref().is_some_and(|c| c.fork_id == "main") {
            let user_msg = ChatMessage::user(message);
            let agent_msg = ChatMessage::agent(&final_response);
            chat_store::append(&persist_path, &user_msg).ok();
            chat_store::append(&persist_path, &agent_msg).ok();

            let mut hist = self.chat_history.lock().await;
            hist.push(user_msg);
            hist.push(agent_msg);
        } else {
            let user_msg = ChatMessage::user(message);
            let agent_msg = ChatMessage::agent(&final_response);
            chat_store::append(&persist_path, &user_msg).ok();
            chat_store::append(&persist_path, &agent_msg).ok();

            if let Some(ref ctx) = ctx {
                let mut guard = self.chat_stack.lock().await;
                if let Some(entry) = guard.iter_mut().find(|e| e.id == ctx.fork_id) {
                    entry.fork_history.push(user_msg);
                    entry.fork_history.push(agent_msg);
                }
            }
        }

        // Record this exchange on the review item's own structured history.
        if let Some(task_id) = discussed_task_id
            && let Ok(Some(mut review_item)) = self.store.find_unresolved_for_task(&task_id) {
            let now = chrono::Utc::now();
            review_item.actions.push(ReviewAction::Comment {
                at: now,
                role: Role::User,
                content: message.to_string(),
            });
            review_item.actions.push(ReviewAction::Comment {
                at: now,
                role: Role::Agent,
                content: final_response.clone(),
            });
            if let Err(e) = self.store.write_review_item(&review_item) {
                warn!(%task_id, error = %e, "failed to persist rework discussion comment");
            }
        }

        Ok(final_response)
    }

    /// Proactively surface project status at session start, without a user message.
    /// Runs the same agent turn as `chat()` (review queue peek, panel switch, tool
    /// loop) but persists only the agent's response — there is no user turn to record.
    pub async fn opening_status(&self) -> anyhow::Result<String> {
        let prompt = "Session just started. Proactively surface what needs my attention \
                      right now: the current epic and its progress, tasks in progress, \
                      anything waiting for my review or approval, and any open questions. \
                      Be concise.";
        let history = self.chat_history.lock().await.clone();
        let final_response = self.run_main_agent_turn(prompt, &history).await?;

        {
            let path = home::current_chat_file();
            let agent_msg = ChatMessage::agent(&final_response);
            chat_store::append(&path, &agent_msg).ok();

            let mut hist = self.chat_history.lock().await;
            hist.push(agent_msg);
        }

        Ok(final_response)
    }

    /// Shared agent turn: peek the review queue, switch the panel and inject queue
    /// context if something is pending, then run the tool-calling loop to completion.
    /// Does not persist anything to chat history — callers decide what to record.
    async fn run_main_agent_turn(&self, message: &str, history: &[ChatMessage]) -> anyhow::Result<String> {
        // 1. Peek review queue (lock released immediately after clone)
        let pending_review = {
            let queue = self.review_queue.lock().await;
            queue.peek().cloned()
        };

        // 2. Switch side panel when something needs human attention
        if pending_review.is_some() {
            self.gitzi_switch_panel("task".to_string()).await;
        }

        // 3. Build context-augmented message for the LLM
        let llm_message = if let Some(ref item) = pending_review {
            let task_title = {
                let board = self.board.read().await;
                board.task(&item.task_id)
                    .map(|t| t.title.clone())
                    .unwrap_or_else(|| item.task_id.clone())
            };
            let kind_summary = match &item.kind {
                review_queue::ReviewItemKind::AgentQuestion { question } => {
                    format!("agent question: {question}")
                }
                review_queue::ReviewItemKind::BufferApproval { buffer_column, .. } => {
                    format!(
                        "is parked at {buffer_column} awaiting a decision. The user can advance it \
                        directly from the board without talking to you, so if they're discussing it \
                        here they have a concern. Do not call gitzi_request_rework until you've \
                        restated their concern in your own words and they've confirmed you understood \
                        both WHAT they want changed and HOW strongly they feel about it. Ask one \
                        clarifying question at a time if anything is unclear."
                    )
                }
            };
            format!(
                "[Review queue] Task \"{task_title}\" needs attention: {kind_summary}\n\n{message}"
            )
        } else {
            message.to_string()
        };

        // Inject repo context so the agent knows what repos are available
        let repo_context = {
            let repos = crate::state::repo_cache::populate(&self.config.repo_paths);
            if repos.is_empty() {
                String::new()
            } else {
                let listing: String = repos.iter().take(20).map(|r| {
                    format!("  {} [{}]", r.path, r.labels.join(", "))
                }).collect::<Vec<_>>().join("\n");
                format!("[Repos]\n{listing}\n\n")
            }
        };
        let llm_message = format!("{repo_context}{llm_message}");

        // 4. Build messages from history + (possibly augmented) message
        let mut messages = MainAgent::history_to_messages(history, &llm_message);

        // 5. Determine tools based on fork context
        let in_fork = {
            let guard = self.chat_stack.lock().await;
            guard.last().is_some_and(|e| e.id != "main")
        };
        let tools = crate::agent::main_agent::tools_for_context(
            in_fork,
            true,
        );

        // 6. Tool-calling loop
        let final_response = loop {
            let (raw_assistant, turn) = self
                .main_agent
                .turn(&messages, &tools)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;

            match turn {
                ChatTurn::Text(text) => break text,
                ChatTurn::ToolCalls(calls) => {
                    // Add assistant message (with tool_calls) to conversation
                    messages.push(raw_assistant);

                    // Execute each tool and add the result
                    for call in &calls {
                        let result = self
                            .execute_main_agent_tool(&call.name, &call.arguments)
                            .await;
                        messages.push(OaiMessage {
                            role: "tool".to_string(),
                            content: Some(result),
                            tool_calls: vec![],
                            tool_call_id: Some(call.id.clone()),
                        });
                    }
                }
            }
        };

        Ok(final_response)
    }

    /// Dispatch a tool call from the main agent to the appropriate Dispatcher method.
    async fn execute_main_agent_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> String {
        match name {
            "gitzi_create_epic" => {
                let title = args
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if title.is_empty() {
                    return "error: missing required argument: title".to_string();
                }
                let description = args
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                match self.gitzi_create_epic(title, description).await {
                    Ok(epic) => serde_json::to_string(&epic).unwrap_or_else(|_| "{}".to_string()),
                    Err(e) => format!("error: {e}"),
                }
            }

            "gitzi_prioritize_task" => {
                let task_id = args
                    .get("task_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let priority = args
                    .get("priority")
                    .and_then(serde_json::Value::as_u64)
                    .map(|v| v as u32);
                if task_id.is_empty() {
                    return "error: missing required argument: task_id".to_string();
                }
                let priority = match priority {
                    Some(p) => p,
                    None => return "error: missing required argument: priority".to_string(),
                };
                match self.gitzi_prioritize_task(&task_id, priority).await {
                    Ok(task) => serde_json::to_string(&task).unwrap_or_else(|_| "{}".to_string()),
                    Err(e) => format!("error: {e}"),
                }
            }

            "gitzi_switch_panel" => {
                let view = args
                    .get("view")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if view.is_empty() {
                    return "error: missing required argument: view".to_string();
                }
                self.gitzi_switch_panel(view).await;
                r#"{"ok":true}"#.to_string()
            }

            "gitzi_create_task" => {
                let epic_id = args
                    .get("epic_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let title = args
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if epic_id.is_empty() {
                    return "error: missing required argument: epic_id".to_string();
                }
                if title.is_empty() {
                    return "error: missing required argument: title".to_string();
                }
                let description = args
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let priority = args
                    .get("priority")
                    .and_then(serde_json::Value::as_u64)
                    .map(|v| v as u32);
                let repo = args
                    .get("repo")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                match self.gitzi_create_task(epic_id, title, description, priority, repo).await {
                    Ok(task) => serde_json::to_string(&task).unwrap_or_else(|_| "{}".to_string()),
                    Err(e) => format!("error: {e}"),
                }
            }

            "gitzi_update_task" => {
                let task_id = args
                    .get("task_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if task_id.is_empty() {
                    return "error: missing required argument: task_id".to_string();
                }
                let title = args
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let description = args
                    .get("description")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                match self.gitzi_update_task(&task_id, title, description).await {
                    Ok(task) => serde_json::to_string(&task).unwrap_or_else(|_| "{}".to_string()),
                    Err(e) => format!("error: {e}"),
                }
            }

            "gitzi_park_task" => {
                let task_id = args
                    .get("task_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let reason = args
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if task_id.is_empty() {
                    return "error: missing required argument: task_id".to_string();
                }
                if reason.is_empty() {
                    return "error: missing required argument: reason".to_string();
                }
                match self.gitzi_park_task(&task_id, reason).await {
                    Ok(()) => r#"{"ok":true}"#.to_string(),
                    Err(e) => format!("error: {e}"),
                }
            }

            "gitzi_list_epics" => match self.gitzi_list_epics().await {
                Ok(epics) => serde_json::to_string(&epics).unwrap_or_else(|_| "[]".to_string()),
                Err(e) => format!("error: {e}"),
            },

            "gitzi_list_tasks" => {
                let epic_id = args
                    .get("epic_id")
                    .and_then(serde_json::Value::as_str);
                match self.gitzi_list_tasks(epic_id).await {
                    Ok(tasks) => serde_json::to_string(&tasks).unwrap_or_else(|_| "[]".to_string()),
                    Err(e) => format!("error: {e}"),
                }
            }

            "gitzi_get_review_item" => {
                let id = args
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if id.is_empty() {
                    return "error: missing required argument: id".to_string();
                }
                match self.gitzi_get_review_item(&id).await {
                    Ok(item) => serde_json::to_string(&item).unwrap_or_else(|_| "{}".to_string()),
                    Err(e) => format!("error: {e}"),
                }
            }

            "gitzi_request_rework" => {
                let task_id = args
                    .get("task_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let feedback = args
                    .get("feedback")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let target_column = args
                    .get("target_column")
                    .and_then(serde_json::Value::as_str);
                if task_id.is_empty() || feedback.is_empty() {
                    return "error: missing required argument: task_id or feedback".to_string();
                }
                let result = if let Some(col_name) = target_column {
                    use serde::de::value::{Error as DeError, StrDeserializer};
                    let deser: StrDeserializer<DeError> =
                        serde::de::value::StrDeserializer::new(col_name);
                    match Column::deserialize(deser) {
                        Ok(col) => self.reject_to(&task_id, col, feedback).await,
                        Err(_) => Err(anyhow::anyhow!("unknown target_column: {col_name}")),
                    }
                } else {
                    self.reject(&task_id, feedback).await
                };
                match result {
                    Ok(()) => "ok: task sent back for rework".to_string(),
                    Err(e) => format!("error: {e}"),
                }
            }

            "gitzi_list_repos" => {
                let repos = crate::state::repo_cache::populate(&self.config.repo_paths);
                if repos.is_empty() {
                    "No repos discovered. Configure repo_paths in config.toml.".to_string()
                } else {
                    repos.iter().map(|r| {
                        format!("- {} [{}]\n  {}", r.path, r.labels.join(", "), r.summary)
                    }).collect::<Vec<_>>().join("\n")
                }
            }

            "gitzi_close_fork" => {
                let summary = args
                    .get("summary")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if summary.is_empty() {
                    return "error: missing required argument: summary".to_string();
                }
                // Check if we're actually in a fork
                let in_fork = {
                    let guard = self.chat_stack.lock().await;
                    guard.last().is_some_and(|e| e.id != "main")
                };
                if !in_fork {
                    return "error: not inside a fork — cannot close".to_string();
                }
                // Pop the fork from the stack
                let fork_id = {
                    let mut guard = self.chat_stack.lock().await;
                    guard.pop().map(|e| e.id).unwrap_or_default()
                };
                // Emit close event
                self.event_bus.emit(
                    event_bus::DispatchEvent::ForkClosed { id: fork_id, summary: summary.clone() }
                );
                format!("ok: fork closed — {summary}")
            }

            "gitzi_rediscover_providers" => match self.gitzi_rediscover_providers().await {
                Ok(summary) => summary,
                Err(e) => format!("error: {e}"),
            },

            "gitzi_activate_provider" => {
                let name = args
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    return "error: missing required argument: name".to_string();
                }
                let account_id = args
                    .get("account_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                let role_name = args
                    .get("role_name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
                match self.gitzi_activate_provider(&name, account_id, role_name).await {
                    Ok(summary) => summary,
                    Err(e) => format!("error: {e}"),
                }
            }

            "gitzi_search_kb" => {
                let query = args
                    .get("query")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                if query.is_empty() {
                    return "error: missing required argument: query".to_string();
                }
                let results = crate::kb::search(query);
                if results.is_empty() {
                    "No matching KB articles found.".to_string()
                } else {
                    results
                        .iter()
                        .map(|(name, content)| format!("## {name}\n\n{content}"))
                        .collect::<Vec<_>>()
                        .join("\n\n---\n\n")
                }
            }

            other => format!("error: unknown tool: {other}"),
        }
    }

    /// Main event loop — subscribes to the bus and reacts to each event variant.
    pub async fn run(&self) -> anyhow::Result<()> {
        let mut rx = self.event_bus.subscribe();

        loop {
            let event = match rx.recv().await {
                Ok(event) => event,
                Err(RecvError::Lagged(n)) => {
                    warn!(skipped = n, "event bus receiver lagged, some events missed");
                    continue;
                }
                Err(RecvError::Closed) => {
                    info!("event bus closed, dispatcher shutting down");
                    return Ok(());
                }
            };

            match event {
                DispatchEvent::TaskCreated { task_id } => {
                    info!(%task_id, "task created — signalling prioritizer");
                    self.agent_pool.signal(AgentRole::Prioritizer);
                }

                DispatchEvent::TaskStageChanged { task_id, from, to } => {
                    // Check if a waiting agent can now advance (WIP slot freed)
                    if let Some(role) = self.wip_waiting.lock().await.remove(&from) {
                        info!(%role, column = %from, "WIP slot freed — re-signalling waiting agent");
                        self.agent_pool.signal(role);
                    }

                    if let Some(role) = to.agent_role() {
                        info!(%task_id, %role, "task entered work column — signalling agent");
                        self.agent_pool.signal(role);
                    } else if to.is_buffer() {
                        // Create a BufferApproval review item
                        let priority = {
                            let b = self.board.read().await;
                            b.task(&task_id)
                                .map(|t| t.priority)
                                .unwrap_or(u32::MAX)
                        };

                        // Persist review item to disk
                        let persisted = PersistedReviewItem {
                            id: crate::id::new_id(&task_id),
                            task_id: task_id.clone(),
                            kind: PersistedReviewKind::BufferApproval {
                                buffer_column: to,
                                task_priority: priority,
                            },
                            created_at: chrono::Utc::now(),
                            actions: Vec::new(),
                        };
                        if let Err(e) = self.store.write_review_item(&persisted) {
                            warn!(%task_id, error = %e, "failed to persist review item");
                        }

                        // Enqueue in-memory review item
                        let item = review_queue::HumanReviewItem::new(
                            &task_id,
                            review_queue::ReviewItemKind::BufferApproval {
                                buffer_column: to,
                                task_priority: priority,
                            },
                        );
                        let mut q = self.review_queue.lock().await;
                        q.enqueue(item);
                        info!(%task_id, column = %to, "buffer entry — review item created and persisted");
                    }

                    // If task reached Done, attempt merge
                    if to == Column::Done {
                        self.attempt_merge(&task_id).await;
                    }
                }

                DispatchEvent::AgentCompleted { task_id, agent_role } => {
                    // The agent loop already handles advancing the task to the next buffer.
                    // This event is for downstream consumers (TUI, logging).
                    info!(%task_id, %agent_role, "agent completed");
                }

                DispatchEvent::AgentBlocked { task_id, agent_role, ref question } => {
                    info!(%task_id, %agent_role, "agent blocked — creating review item");
                    let item = review_queue::HumanReviewItem::new(
                        &task_id,
                        review_queue::ReviewItemKind::AgentQuestion {
                            question: question.clone(),
                        },
                    );
                    let mut q = self.review_queue.lock().await;
                    q.enqueue(item);
                }

                DispatchEvent::HumanApprovalReceived { task_id, target_column } => {
                    // NO-OP: approve() already signalled the agent.
                    // This event exists for TUI/logging consumers only.
                    info!(%task_id, %target_column, "approval event received (no-op in run loop)");
                }

                DispatchEvent::HumanRejectionReceived { task_id, returned_to, feedback: _ } => {
                    // NO-OP: reject() already signalled the agent.
                    // This event exists for TUI/logging consumers only.
                    info!(%task_id, %returned_to, "rejection event received (no-op in run loop)");
                }

                DispatchEvent::BootComplete => {
                    // Already handled in start() — no-op here.
                    info!("boot complete event received in run loop");
                }

                DispatchEvent::PanelSwitch { .. } => {
                    // TUI-only event — no-op in dispatcher run loop.
                }

                DispatchEvent::ChatResponse { .. } => {
                    // TUI-only event — delivered to subscribers, no-op in run loop.
                }

                DispatchEvent::ForkCreated { .. } | DispatchEvent::ForkClosed { .. } => {
                    // TUI-only events — no-op in dispatcher run loop.
                }
            }
        }
    }
}

/// Point the `main` role's `[[agents]]` entry at `provider_name`, creating
/// the entry if one doesn't exist yet. Other roles keep falling back to
/// `main` (or the local `claude` CLI) per the agent-resolution rules in
/// `config.rs` — only `main` is rewired here.
fn wire_main_agent(config: &mut Config, provider_name: &str) {
    if let Some(agent) = config.agents.iter_mut().find(|a| a.role == "main") {
        agent.provider = Some(provider_name.to_string());
        agent.api_url = None;
    } else {
        config.agents.push(crate::config::AgentDef {
            role: "main".to_string(),
            provider: Some(provider_name.to_string()),
            ..crate::config::AgentDef::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_all_has_13_variants() {
        assert_eq!(Column::all().len(), 11);
    }

    #[test]
    fn column_order_starts_with_prioritized_ends_with_done() {
        let all = Column::all();
        assert_eq!(all[0], Column::Prioritized);
        assert_eq!(all[10], Column::Done);
    }

    #[test]
    fn column_next_prev_are_inverse() {
        for col in Column::all() {
            if let Some(next) = col.next() {
                assert_eq!(next.prev(), Some(*col));
            }
        }
    }

    #[test]
    fn done_has_no_next() {
        assert_eq!(Column::Done.next(), None);
    }

    #[test]
    fn prioritized_has_no_prev() {
        assert_eq!(Column::Prioritized.prev(), None);
    }

    #[test]
    fn buffer_columns_are_correct() {
        let buffers: Vec<Column> = Column::all()
            .iter()
            .filter(|c| c.is_buffer())
            .copied()
            .collect();
        assert_eq!(
            buffers,
            vec![
                Column::CodingBuffer,
                Column::ReviewBuffer,
                Column::SecurityAuditBuffer,
                Column::DeploymentBuffer,
            ]
        );
    }

    #[test]
    fn work_columns_have_agent_roles() {
        let work_columns = [
            Column::Prioritized,
            Column::Designing,
            Column::Coding,
            Column::Reviewing,
            Column::Auditing,
            Column::Deploying,
        ];
        for col in &work_columns {
            assert!(col.agent_role().is_some(), "{col} should have an agent role");
        }
    }

    #[test]
    fn buffer_and_done_have_no_agent_role() {
        for col in Column::all() {
            if col.is_buffer() || *col == Column::Done {
                assert_eq!(col.agent_role(), None, "{col} should have no agent role");
            }
        }
    }

    #[test]
    fn agent_role_all_has_7_variants() {
        assert_eq!(AgentRole::all().len(), 6);
    }

    #[test]
    fn agent_role_column_roundtrip() {
        for role in AgentRole::all() {
            let col = role.column();
            assert_eq!(col.agent_role(), Some(*role));
        }
    }
}
