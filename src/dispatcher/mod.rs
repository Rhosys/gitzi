pub mod agent_pool;
pub mod board;
pub mod event_bus;
pub mod review_queue;

use std::collections::HashMap;
use std::sync::Arc;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Mutex, RwLock, mpsc};
use tracing::{info, warn};

use crate::agent::{ChatTurn, MainAgent, MainChatBackend, OaiMessage, build_main_agent};
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
        if idx == 0 {
            None
        } else {
            ALL_COLUMNS.get(idx - 1).copied()
        }
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

    /// Hardcoded default system prompt for this role, prefixed with shared
    /// gitzi/kanban context so every agent understands the pipeline.
    pub fn default_system_prompt(&self) -> String {
        const GITZI_PREAMBLE: &str = "\
You are an AI agent in gitzi, an agile SDLC harness. The pipeline has a Kanban board: \
Prioritized → Designing → CodingBuffer → Coding → ReviewBuffer → Reviewing → \
SecurityAuditBuffer → Auditing → DeploymentBuffer → Deploying → Done. \
Buffer columns require human approval. Work columns have dedicated AI agents. \
Use your tools to look up any state you need — never guess.\n\n";

        let role_prompt = match self {
            AgentRole::Prioritizer => {
                "You break epics into minimal, independently-shippable tasks ordered by dependency and value."
            }
            AgentRole::Designer => {
                "\
You produce concise technical designs. No code — architecture, data models, interfaces only.\n\n\
BEFORE proposing anything new for a task:\n\
1. Review the existing codebase for code, components, icons, buttons, and UI patterns that \
already do something similar to what the task needs.\n\
2. Check whether a public, popular library already solves the problem.\n\
3. Never design something new when something — in this codebase or as a public library — \
already does the job. Reuse it directly with no further discussion.\n\
4. If nothing existing matches exactly but something similar exists, do not silently choose \
to extend, duplicate, or build from scratch yourself. List every similar existing piece you \
found, state how likely each one is to be a viable fit (and why), and ask the user to decide: \
extend the closest match, duplicate it, or build new.\n\
5. Only design from scratch outright when nothing similar exists anywhere in the codebase or \
in available public libraries.\n\
6. Explicitly list every existing test file that will need to change as a result of this design, \
and describe what each change will be. If no existing tests need to change, state that explicitly."
            }
            AgentRole::Coder => {
                "\
You are a disciplined coding agent. Make the smallest possible change. No refactoring, no extras.\n\n\
CODE PATTERNS:\n\
- Prefer early returns with `if` over `if`/`else`. Handle the exceptional or \
terminating case first and return, instead of nesting the main logic inside an `else`.\n\
- Avoid `else if` chains — they are almost always an anti-pattern. Use early returns, \
or a `match`/`switch` over the discriminant, instead.\n\
- For value lookups from a map, prefer a default/fallback expression \
(e.g. `dict[key] || default` or `map.get(key).unwrap_or(default)`) over an `if`/`else` \
that assigns the value in each branch.\n\n\
REFACTOR FIRST:\n\
- Before implementing, check whether refactoring existing code first would make this task \
trivial and clean to implement.\n\
- If so, do NOT refactor inline and do NOT implement the original task yet. Call gitzi_create_task \
to create a separate refactor ticket under the same epic describing exactly what needs to change.\n\
- Then call gitzi_create_review_item on your current task asking the human to confirm the \
refactor-first plan, and stop.\n\
- Once a later run shows the decision was accepted, call gitzi_block_task on your current \
task_id with blocked_by set to the new refactor ticket's ID, then stop. Your task moves back \
to the top of Prioritized and waits until the refactor ticket reaches Done.\n\
- If the decision was declined, implement the original task directly with no refactor.\n\
- Never implement the refactor ticket in the same run as the original task — it goes through \
the normal pipeline and may be picked up later in its own run.\n\n\
EXISTING TESTS:\n\
- Do not modify any existing test unless the task description explicitly names that test \
and describes the required change.\n\
- Adding new tests is always allowed.\n\
- If you find that an existing test must change but the task does not mention it, stop: \
call gitzi_create_review_item to ask the human whether to update the test and how."
            }
            AgentRole::Reviewer => {
                "\
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
7. Check: were any existing tests modified? For each modified existing test, verify it is \
explicitly listed in the task description with a documented reason. Any modified test not \
named in the task is an automatic rejection — new tests are always allowed, changes to \
existing tests are not.\n\
8. Report ALL findings as a structured list of problems.\n\
9. If problems found: reject the task with specific, actionable feedback.\n\
10. If clean: approve (respond with text, no tool calls).\n\n\
NEVER approve work that has problems. Be ruthless. The coder can handle it."
            }
            AgentRole::Auditor => {
                "You perform security audits. Check for vulnerabilities, leaked secrets, unsafe patterns."
            }
            AgentRole::Infrarian => {
                "You manage deployment infrastructure. Minimal, reproducible, observable."
            }
        };
        format!("{GITZI_PREAMBLE}{role_prompt}")
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
    pub main_agent: Box<dyn MainChatBackend>,
    /// The distinguished fallback "control-plane" brain (ADR-002). Built only
    /// when a fallback provider is configured *and* distinct from main's own
    /// provider, so it can run the recovery conversation when main is down.
    /// `None` when main already is the fallback (recovery couldn't help).
    pub fallback_agent: Option<Box<dyn MainChatBackend>>,
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
            let current_col = board
                .column_of(task_id)
                .ok_or_else(|| anyhow::anyhow!("task '{task_id}' not found on board"))?;
            if !current_col.is_buffer() {
                anyhow::bail!("task '{task_id}' is in {current_col}, not a buffer column");
            }
            current_col
                .next()
                .ok_or_else(|| anyhow::anyhow!("buffer column {current_col} has no next column"))?
        };

        // Write to board: advance task and record history
        {
            let mut board = self.board.write().await;
            board.advance(task_id, next_col)?;
            let task = board
                .task_mut(task_id)
                .ok_or_else(|| anyhow::anyhow!("task '{task_id}' disappeared after advance"))?;
            task.history
                .push(crate::model::task::HistoryEntry::Approval {
                    at: chrono::Utc::now(),
                    target_stage: next_col.into(),
                });
        }

        // Persist task to disk
        {
            let board = self.board.read().await;
            if let Some(task) = board.task(task_id)
                && let Err(e) = self.store.write_task(task)
            {
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
            let current_col = board
                .column_of(task_id)
                .ok_or_else(|| anyhow::anyhow!("task '{task_id}' not found on board"))?;
            if !current_col.is_buffer() {
                anyhow::bail!("task '{task_id}' is in {current_col}, not a buffer column");
            }
            current_col.prev().ok_or_else(|| {
                anyhow::anyhow!("buffer column {current_col} has no previous column")
            })?
        };
        self.reject_to(task_id, target_col, feedback).await
    }

    /// Send a task back to a specific target work column with feedback.
    /// Used when the human decides to skip intermediate columns (e.g. ReviewBuffer → Coding
    /// instead of ReviewBuffer → Reviewing, for "review valid, send back to coder").
    pub async fn reject_to(
        &self,
        task_id: &str,
        target_col: Column,
        feedback: String,
    ) -> anyhow::Result<()> {
        // Validate: task must be in a buffer column
        {
            let board = self.board.read().await;
            let current_col = board
                .column_of(task_id)
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
            let task = board
                .task_mut(task_id)
                .ok_or_else(|| anyhow::anyhow!("task '{task_id}' disappeared after advance"))?;
            task.agent_feedback = Some(feedback.clone());
            task.history
                .push(crate::model::task::HistoryEntry::Rejection {
                    at: chrono::Utc::now(),
                    feedback: feedback.clone(),
                    returned_to: target_col.into(),
                });
        }

        // Persist task to disk
        {
            let board = self.board.read().await;
            if let Some(task) = board.task(task_id)
                && let Err(e) = self.store.write_task(task)
            {
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
        use crate::git::ops::{MergeOutcome, merge_task_branch};

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
                    repo_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("default"),
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
            queue
                .dequeue(item_id)
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
            let task = board
                .task(&item.task_id)
                .ok_or_else(|| anyhow::anyhow!("task '{}' not found on board", item.task_id))?;
            let col = task.stage.to_column();
            col.agent_role().ok_or_else(|| {
                anyhow::anyhow!(
                    "task '{}' is in column {col} which has no agent",
                    item.task_id
                )
            })?
        };

        // Unblock the agent with the answer
        self.agent_pool.unblock(role, answer);

        Ok(())
    }

    // ── gitzi_ management tools ───────────────────────────────────────────────

    pub async fn gitzi_list_epics(&self) -> anyhow::Result<Vec<crate::model::Epic>> {
        Ok(self.store.load_all_epics()?)
    }

    pub async fn gitzi_list_tasks(
        &self,
        epic_id: Option<&str>,
    ) -> anyhow::Result<Vec<crate::model::Task>> {
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
        self.event_bus.emit(DispatchEvent::TaskCreated {
            task_id: id.clone(),
        });

        Ok(task)
    }

    pub async fn gitzi_update_task(
        &self,
        task_id: &str,
        title: Option<String>,
        description: Option<String>,
    ) -> anyhow::Result<crate::model::Task> {
        let mut task = self.store.load_task(task_id)?;
        if let Some(t) = title {
            task.title = t;
        }
        if let Some(d) = description {
            task.description = Some(d);
        }
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

    pub async fn gitzi_prioritize_task(
        &self,
        task_id: &str,
        priority: u32,
    ) -> anyhow::Result<crate::model::Task> {
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

    /// Block the task on one or more other tasks (e.g. a refactor ticket the
    /// Coder decided must land first). Moves the task back to the top of
    /// Prioritized; it becomes eligible for pick-up again once every task in
    /// `blocked_by` reaches `Stage::Done` (see `KanbanBoard::next_unblocked`).
    /// Unlike `gitzi_park_task` (a free-text note only), this creates a
    /// structural link the pipeline actually enforces.
    pub async fn gitzi_block_task(
        &self,
        task_id: &str,
        blocked_by: Vec<String>,
    ) -> anyhow::Result<()> {
        let from_col = {
            let mut board = self.board.write().await;
            let from_col = board
                .column_of(task_id)
                .ok_or_else(|| anyhow::anyhow!("task '{task_id}' not found on board"))?;
            board.advance(task_id, Column::Prioritized)?;
            board.set_priority(task_id, 0);
            let task = board
                .task_mut(task_id)
                .ok_or_else(|| anyhow::anyhow!("task '{task_id}' disappeared after advance"))?;
            task.blocked_by = blocked_by;
            task.updated_at = chrono::Utc::now();
            from_col
        };

        // Persist the now-fully-updated board copy, not a separately reloaded
        // one, so the on-disk `stage` never diverges from the in-memory board.
        {
            let board = self.board.read().await;
            if let Some(task) = board.task(task_id)
                && let Err(e) = self.store.write_task(task)
            {
                warn!(%task_id, error = %e, "failed to persist task after block");
            }
        }

        self.event_bus.emit(DispatchEvent::TaskStageChanged {
            task_id: task_id.to_string(),
            from: from_col,
            to: Column::Prioritized,
        });
        self.agent_pool.signal(AgentRole::Prioritizer);

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
            kind: PersistedReviewKind::AgentQuestion {
                question: full_question.clone(),
            },
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
                kind: review_queue::ReviewItemKind::AgentQuestion {
                    question: full_question,
                },
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

        // Shared scan + merge logic lives in `crate::setup` so the agent tool
        // and the daemon's bootstrap setup phase stay in lockstep (ADR-002).
        let discovered = crate::bootstrap::discover_providers();
        let added = crate::setup::merge_discovered(&mut config, &discovered);

        if !added.is_empty() {
            config.write(&gitzi_home)?;
            info!(added = ?added, "rediscovered new providers — merged into config.toml as disabled");
        }

        if discovered.is_empty() {
            return Ok(
                "No LLM providers or AWS Bedrock access discovered on this machine.".to_string(),
            );
        }

        let lines: Vec<String> = discovered
            .iter()
            .map(|p| {
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
                format!("- {} [{kind}] {status}", p.name)
            })
            .collect();

        let added_note = if added.is_empty() {
            String::new()
        } else {
            format!("\n\nNewly discovered: {}", added.join(", "))
        };

        Ok(format!(
            "Discovered providers:\n{}{added_note}",
            lines.join("\n")
        ))
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

        // Activation logic is shared with the daemon's bootstrap setup phase —
        // both drive `crate::setup::activate` so there is one source of truth
        // for "enable a provider and wire it into main" (ADR-002).
        match crate::setup::activate(&mut config, name, account_id, role_name).await? {
            crate::setup::ActivationOutcome::Activated { message } => {
                config.write(&gitzi_home)?;
                Ok(format!(
                    "ok: {message} Restart gitzi for this to take effect."
                ))
            }
            crate::setup::ActivationOutcome::NeedsMoreInput { message } => Ok(message),
        }
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
                        PersistedReviewKind::BufferApproval {
                            buffer_column,
                            task_priority,
                        } => review_queue::ReviewItemKind::BufferApproval {
                            buffer_column,
                            task_priority,
                        },
                    };
                    let item = review_queue::HumanReviewItem {
                        id: p.id,
                        task_id: p.task_id,
                        kind,
                        created_at: p.created_at,
                    };
                    queue.enqueue(item);
                }
                info!(
                    count = queue.len(),
                    "loaded unresolved review items from disk"
                );
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

        // 11. Build main agent from config, plus the fallback control-plane
        // brain used to drive recovery when main's own provider is down.
        let main_agent_def = config.resolve_agent("main");
        let main_agent = build_main_agent(&config, &main_agent_def);
        let fallback_agent = build_fallback_agent(&config);

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
            fallback_agent,
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
                Ok(new_config) => match WipLimits::from_config(&new_config.wip_limits.overrides) {
                    Ok(new_limits) => {
                        *self.wip_limits.write().await = new_limits;
                        info!("config.toml changed — WIP limits reloaded");
                    }
                    Err(e) => warn!(
                        error = %e,
                        "config.toml changed but WIP limits are invalid — keeping previous limits"
                    ),
                },
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
        let persist_path = ctx
            .as_ref()
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
            && let Ok(Some(mut review_item)) = self.store.find_unresolved_for_task(&task_id)
        {
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
    async fn run_main_agent_turn(
        &self,
        message: &str,
        history: &[ChatMessage],
    ) -> anyhow::Result<String> {
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
                board
                    .task(&item.task_id)
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
                let listing: String = repos
                    .iter()
                    .take(20)
                    .map(|r| format!("  {} [{}]", r.path, r.labels.join(", ")))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("[Repos]\n{listing}\n\n")
            }
        };
        // Inject board state so the agent knows what tasks are in flight
        let board_context = {
            let board = self.board.read().await;
            let mut lines = Vec::new();
            for col in crate::dispatcher::Column::all() {
                let task_ids = board.tasks_in(*col);
                if !task_ids.is_empty() {
                    let task_list: String = task_ids
                        .iter()
                        .take(5)
                        .filter_map(|id| board.task(id))
                        .map(|t| format!("  - {} ({})", t.title, t.id))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !task_list.is_empty() {
                        lines.push(format!("[{col}]\n{task_list}"));
                    }
                }
            }
            if lines.is_empty() {
                String::new()
            } else {
                format!("[Board]\n{}\n\n", lines.join("\n"))
            }
        };
        let llm_message = format!("{repo_context}{board_context}{llm_message}");

        // 4. Build messages from history + (possibly augmented) message
        let mut messages = MainAgent::history_to_messages(history, &llm_message);

        // 5. Determine tools based on fork context
        let in_fork = {
            let guard = self.chat_stack.lock().await;
            guard.last().is_some_and(|e| e.id != "main")
        };
        let tools = crate::agent::main_agent::tools_for_context(in_fork, true);

        // 6. Tool-calling loop. If main's provider is down, fall back *once* to
        // the control-plane brain to run a recovery conversation (ADR-002) —
        // never a silent swap for the user's real request.
        let mut active_agent: &dyn MainChatBackend = &*self.main_agent;
        let mut recovered = false;
        let final_response = loop {
            let (raw_assistant, turn) = match active_agent
                .turn_streaming(&messages, &tools, &self.event_bus)
                .await
            {
                Ok(t) => t,
                Err(e) => {
                    if !recovered && let Some(fallback) = self.fallback_agent.as_ref() {
                        warn!(error = %e, "main provider failed — handing off to the fallback brain for recovery");
                        recovered = true;
                        active_agent = &**fallback;
                        messages.push(OaiMessage {
                            role: "system".to_string(),
                            content: Some(format!(
                                "The main model provider is not responding ({e}). You are the \
                                 fallback assistant. Tell the user plainly that their main model \
                                 is unavailable, then ask what they want to do — retry, switch to \
                                 a different provider, or keep going with you. Do not attempt their \
                                 original request as if nothing happened."
                            )),
                            tool_calls: vec![],
                            tool_call_id: None,
                        });
                        continue;
                    }
                    return Err(anyhow::anyhow!("{e}"));
                }
            };

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
    async fn execute_main_agent_tool(&self, name: &str, args: &serde_json::Value) -> String {
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
                match self
                    .gitzi_create_task(epic_id, title, description, priority, repo)
                    .await
                {
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
                let epic_id = args.get("epic_id").and_then(serde_json::Value::as_str);
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
                    repos
                        .iter()
                        .map(|r| format!("- {} [{}]\n  {}", r.path, r.labels.join(", "), r.summary))
                        .collect::<Vec<_>>()
                        .join("\n")
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
                self.event_bus.emit(event_bus::DispatchEvent::ForkClosed {
                    id: fork_id,
                    summary: summary.clone(),
                });
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
                match self
                    .gitzi_activate_provider(&name, account_id, role_name)
                    .await
                {
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
                            b.task(&task_id).map(|t| t.priority).unwrap_or(u32::MAX)
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

                DispatchEvent::AgentCompleted {
                    task_id,
                    agent_role,
                } => {
                    // The agent loop already handles advancing the task to the next buffer.
                    // This event is for downstream consumers (TUI, logging).
                    info!(%task_id, %agent_role, "agent completed");
                }

                DispatchEvent::AgentBlocked {
                    task_id,
                    agent_role,
                    ref question,
                } => {
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

                DispatchEvent::HumanApprovalReceived {
                    task_id,
                    target_column,
                } => {
                    // NO-OP: approve() already signalled the agent.
                    // This event exists for TUI/logging consumers only.
                    info!(%task_id, %target_column, "approval event received (no-op in run loop)");
                }

                DispatchEvent::HumanRejectionReceived {
                    task_id,
                    returned_to,
                    feedback: _,
                } => {
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

                DispatchEvent::LogEntry { .. } => {
                    // TUI-only — no-op in dispatcher run loop.
                }

                DispatchEvent::StreamingTokens { .. }
                | DispatchEvent::StreamingToolCall { .. }
                | DispatchEvent::StreamingDone => {
                    // TUI-only — progress tracking, no-op in dispatcher run loop.
                }
            }
        }
    }
}

/// Point the `main` role's `[[agents]]` entry at `provider_name`, creating
/// the entry if one doesn't exist yet. Other roles keep falling back to
/// `main` (or the local `claude` CLI) per the agent-resolution rules in
/// `config.rs` — only `main` is rewired here.
/// Build the fallback control-plane brain (ADR-002): a chat agent bound to the
/// distinguished `fallback_provider`, used to run the recovery conversation
/// when main's own provider is down. Returns `None` unless the fallback is set,
/// OpenAI-compatible, and *distinct* from main's provider — if main
/// already is the fallback, falling back couldn't help.
fn build_fallback_agent(config: &Config) -> Option<Box<dyn MainChatBackend>> {
    let fallback = config.fallback_provider.as_ref()?;
    let provider = config.providers.get(fallback)?;
    let main_def = config.resolve_agent("main");
    if main_def.provider.as_deref() == Some(fallback.as_str()) {
        return None;
    }
    let def = crate::config::AgentDef {
        role: "main".to_string(),
        provider: Some(fallback.clone()),
        api_url: None,
        model: provider
            .model_id
            .clone()
            .unwrap_or_else(|| main_def.model.clone()),
    };
    Some(build_main_agent(config, &def))
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
            assert!(
                col.agent_role().is_some(),
                "{col} should have an agent role"
            );
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

    // ── Fallback control-plane brain (ADR-002) ──────────────────────────────

    fn openai_provider(url: &str) -> crate::config::ProviderDef {
        crate::config::ProviderDef {
            api_url: url.to_string(),
            ..crate::config::ProviderDef::default()
        }
    }

    #[test]
    fn fallback_agent_none_when_fallback_is_also_main() {
        // Main is bound to the same provider as the fallback — recovery via the
        // fallback couldn't help, so there's no separate brain.
        let config = Config {
            providers: std::collections::HashMap::from([(
                "local".to_string(),
                openai_provider("http://localhost:1234/v1"),
            )]),
            agents: vec![crate::config::AgentDef {
                role: "main".to_string(),
                provider: Some("local".to_string()),
                ..crate::config::AgentDef::default()
            }],
            fallback_provider: Some("local".to_string()),
            ..Config::default()
        };
        assert!(build_fallback_agent(&config).is_none());
    }

    #[test]
    fn fallback_agent_built_when_distinct_from_main() {
        // Main points at one provider, the fallback brain at another — the
        // fallback is built and aimed at the fallback provider's endpoint.
        let config = Config {
            providers: std::collections::HashMap::from([
                (
                    "remote".to_string(),
                    openai_provider("http://remote:8080/v1"),
                ),
                (
                    "local".to_string(),
                    openai_provider("http://localhost:11434/v1"),
                ),
            ]),
            agents: vec![crate::config::AgentDef {
                role: "main".to_string(),
                provider: Some("remote".to_string()),
                ..crate::config::AgentDef::default()
            }],
            fallback_provider: Some("local".to_string()),
            ..Config::default()
        };
        let fallback = build_fallback_agent(&config).expect("distinct fallback should build");
        assert_eq!(
            fallback.base_url(),
            Some("http://localhost:11434/v1".to_string())
        );
    }

    #[test]
    fn fallback_agent_none_when_provider_missing() {
        // Fallback references a provider not in config → None.
        let config = Config {
            providers: std::collections::HashMap::new(),
            agents: Vec::new(),
            fallback_provider: Some("nonexistent".to_string()),
            ..Config::default()
        };
        assert!(build_fallback_agent(&config).is_none());
    }
}
