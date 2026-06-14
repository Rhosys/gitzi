pub mod agent_pool;
pub mod board;
pub mod event_bus;
pub mod review_queue;

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};

use crate::agent::{build_main_agent, ChatTurn, MainAgent, OaiMessage};
use crate::config::Config;
use crate::mcp::auth::TokenStore;
use crate::state::chat::{self as chat_store, ChatMessage};
use crate::state::home;
use crate::state::reader;
use crate::state::review::{self, PersistedReviewItem, PersistedReviewKind, ReviewAction};
use crate::state::writer;

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
    TestBuffer,
    Testing,
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
    Column::TestBuffer,
    Column::Testing,
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
                | Column::TestBuffer
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
            Column::Testing => Some(AgentRole::Tester),
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
            Column::TestBuffer => "test-buffer",
            Column::Testing => "testing",
            Column::SecurityAuditBuffer => "security-audit-buffer",
            Column::Auditing => "auditing",
            Column::DeploymentBuffer => "deployment-buffer",
            Column::Deploying => "deploying",
            Column::Done => "done",
        };
        write!(f, "{s}")
    }
}

/// One of the seven LLM agent roles. Each role has exactly one agent instance
/// and maps to a specific work column on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentRole {
    Prioritizer,
    Designer,
    Coder,
    Reviewer,
    Tester,
    Auditor,
    Infrarian,
}

static ALL_ROLES: &[AgentRole] = &[
    AgentRole::Prioritizer,
    AgentRole::Designer,
    AgentRole::Coder,
    AgentRole::Reviewer,
    AgentRole::Tester,
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
            AgentRole::Tester => Column::Testing,
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
            base_url: None,
            system_prompt: Some(self.default_system_prompt().to_string()),
        }
    }

    /// Hardcoded default system prompt for this role.
    pub fn default_system_prompt(&self) -> &'static str {
        match self {
            AgentRole::Prioritizer => "You break epics into minimal, independently-shippable tasks ordered by dependency and value.",
            AgentRole::Designer => "You produce concise technical designs. No code — architecture, data models, interfaces only.",
            AgentRole::Coder => "You are a disciplined coding agent. Make the smallest possible change. No refactoring, no extras.",
            AgentRole::Reviewer => "You review code for correctness, security, and adherence to the design. Flag issues, never rewrite.",
            AgentRole::Tester => "You write and run tests. Property-based where applicable, example-based otherwise.",
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
            AgentRole::Tester => "tester",
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
    pub wip_limits: Arc<WipLimits>,
    /// Agents waiting to advance into a full column.
    pub wip_waiting: Arc<Mutex<HashMap<Column, AgentRole>>>,
    /// Persistent chat history (in-memory mirror of chat.jsonl).
    pub chat_history: Arc<Mutex<Vec<ChatMessage>>>,
    /// The main coordination agent that drives the chat interface.
    pub main_agent: MainAgent,
    /// Token store for MCP sub-agent authorization.
    pub token_store: Arc<TokenStore>,
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
                && let Err(e) = writer::write_task(task) {
                warn!(%task_id, error = %e, "failed to persist task after approval");
            }
        }

        // Persist review item with approval action appended
        if let Ok(Some(mut review_item)) = review::find_unresolved_for_task(task_id) {
            review_item.actions.push(ReviewAction::Approval {
                at: chrono::Utc::now(),
            });
            if let Err(e) = review::write_review_item(&review_item) {
                warn!(%task_id, error = %e, "failed to persist review item approval");
            }
        }

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

    /// Reject a task in a buffer column: move to previous work column with priority 0,
    /// store feedback, record history, emit event, and signal the agent.
    pub async fn reject(&self, task_id: &str, feedback: String) -> anyhow::Result<()> {
        let prev_col = {
            let board = self.board.read().await;
            let current_col = board.column_of(task_id)
                .ok_or_else(|| anyhow::anyhow!("task '{task_id}' not found on board"))?;
            if !current_col.is_buffer() {
                anyhow::bail!("task '{task_id}' is in {current_col}, not a buffer column");
            }
            current_col.prev()
                .ok_or_else(|| anyhow::anyhow!("buffer column {current_col} has no previous column"))?
        };

        // Write to board: advance to prev, set priority, store feedback, record history
        {
            let mut board = self.board.write().await;
            board.advance(task_id, prev_col)?;
            board.set_priority(task_id, 0);
            let task = board.task_mut(task_id)
                .ok_or_else(|| anyhow::anyhow!("task '{task_id}' disappeared after advance"))?;
            task.agent_feedback = Some(feedback.clone());
            task.history.push(crate::model::task::HistoryEntry::Rejection {
                at: chrono::Utc::now(),
                feedback: feedback.clone(),
                returned_to: prev_col.into(),
            });
        }

        // Persist task to disk
        {
            let board = self.board.read().await;
            if let Some(task) = board.task(task_id)
                && let Err(e) = writer::write_task(task) {
                warn!(%task_id, error = %e, "failed to persist task after rejection");
            }
        }

        // Persist review item with rejection action appended
        if let Ok(Some(mut review_item)) = review::find_unresolved_for_task(task_id) {
            review_item.actions.push(ReviewAction::Rejection {
                at: chrono::Utc::now(),
                feedback: feedback.clone(),
            });
            if let Err(e) = review::write_review_item(&review_item) {
                warn!(%task_id, error = %e, "failed to persist review item rejection");
            }
        }

        // Emit event
        self.event_bus.emit(DispatchEvent::HumanRejectionReceived {
            task_id: task_id.to_string(),
            returned_to: prev_col,
            feedback,
        });

        // Signal the agent for the previous work column
        if let Some(role) = prev_col.agent_role() {
            self.agent_pool.signal(role);
        }

        Ok(())
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
        match review::load_review_item(item_id) {
            Ok(mut persisted) => {
                persisted.actions.push(ReviewAction::Answer {
                    at: chrono::Utc::now(),
                    content: answer.clone(),
                });
                if let Err(e) = review::write_review_item(&persisted) {
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
        Ok(reader::load_all_epics()?)
    }

    pub async fn gitzi_list_tasks(&self, epic_id: Option<&str>) -> anyhow::Result<Vec<crate::model::Task>> {
        let all = reader::load_all_tasks()?;
        if let Some(id) = epic_id {
            Ok(all.into_iter().filter(|t| t.epic == id).collect())
        } else {
            Ok(all)
        }
    }

    pub async fn gitzi_get_review_item(&self, id: &str) -> anyhow::Result<PersistedReviewItem> {
        Ok(review::load_review_item(id)?)
    }

    pub async fn gitzi_create_epic(
        &self,
        title: String,
        description: Option<String>,
    ) -> anyhow::Result<crate::model::Epic> {
        let id = crate::id::new_id(&title);
        let mut epic = crate::model::Epic::new(&id, &title);
        epic.description = description;
        writer::write_epic(&epic)?;
        Ok(epic)
    }

    pub async fn gitzi_create_task(
        &self,
        epic_id: String,
        title: String,
        description: Option<String>,
        priority: Option<u32>,
    ) -> anyhow::Result<crate::model::Task> {
        let id = crate::id::new_id(&title);
        let mut task = crate::model::Task::new(&id, &epic_id, &title);
        task.description = description;
        task.priority = priority.unwrap_or(100);
        writer::write_task(&task)?;

        // Update parent epic's task list on disk
        if let Ok(mut epic) = reader::load_epic(&epic_id) {
            epic.tasks.push(id.clone());
            if let Err(e) = writer::write_epic(&epic) {
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
        let mut task = reader::load_task(task_id)?;
        if let Some(t) = title { task.title = t; }
        if let Some(d) = description { task.description = Some(d); }
        task.updated_at = chrono::Utc::now();
        writer::write_task(&task)?;

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
        let mut task = reader::load_task(task_id)?;
        task.priority = priority;
        task.updated_at = chrono::Utc::now();
        writer::write_task(&task)?;

        {
            let mut board = self.board.write().await;
            board.set_priority(task_id, priority);
        }

        Ok(task)
    }

    pub async fn gitzi_park_task(&self, task_id: &str, reason: String) -> anyhow::Result<()> {
        let mut task = reader::load_task(task_id)?;
        task.resume_summary = Some(reason);
        task.updated_at = chrono::Utc::now();
        writer::write_task(&task)?;

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
        review::write_review_item(&persisted)?;

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

    /// Boot the dispatcher: load tasks, build board, spawn agents, emit BootComplete,
    /// and signal agents whose columns contain work.
    pub async fn start(config: Config) -> anyhow::Result<Self> {
        let config = Arc::new(config);

        // 1. Load tasks from disk
        let tasks = reader::load_all_tasks()?;
        info!(task_count = tasks.len(), "loaded tasks from disk");

        // 2. Build KanbanBoard from tasks
        let board = Arc::new(RwLock::new(KanbanBoard::from_tasks(tasks)));

        // 3. Create EventBus (capacity 256)
        let event_bus = Arc::new(EventBus::new(256));

        // 4. Create empty HumanReviewQueue and load persisted unresolved items
        let mut queue = HumanReviewQueue::new();
        match review::load_all_unresolved() {
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

        // 5. Create WipLimits::default()
        let wip_limits = Arc::new(WipLimits::default());

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
            let path = home::chat_file().unwrap_or_else(|_| std::path::PathBuf::from("/tmp/gitzi-chat.jsonl"));
            let messages = chat_store::load(&path).unwrap_or_default();
            info!(messages = messages.len(), "loaded chat history from disk");
            Arc::new(Mutex::new(messages))
        };

        // 11. Build main agent from config
        let main_agent_def = config.resolve_agent("main");
        let main_agent = build_main_agent(&main_agent_def);

        Ok(Self {
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
        })
    }

    /// Process a user chat message through the main agent tool calling loop.
    pub async fn chat(&self, message: &str) -> anyhow::Result<String> {
        let history = self.chat_history.lock().await.clone();
        let mut messages = MainAgent::history_to_messages(&history, message);

        let final_response = loop {
            let (raw_assistant, turn) = self
                .main_agent
                .turn(&messages)
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

        // Persist both turns to disk
        if let Ok(path) = home::chat_file() {
            let user_msg = ChatMessage::user(message);
            let agent_msg = ChatMessage::agent(&final_response);
            chat_store::append(&path, &user_msg).ok();
            chat_store::append(&path, &agent_msg).ok();

            // Mirror in memory
            let mut hist = self.chat_history.lock().await;
            hist.push(user_msg);
            hist.push(agent_msg);
        }

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
                match self.gitzi_create_task(epic_id, title, description, priority).await {
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
                        if let Err(e) = review::write_review_item(&persisted) {
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
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_all_has_13_variants() {
        assert_eq!(Column::all().len(), 13);
    }

    #[test]
    fn column_order_starts_with_prioritized_ends_with_done() {
        let all = Column::all();
        assert_eq!(all[0], Column::Prioritized);
        assert_eq!(all[12], Column::Done);
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
                Column::TestBuffer,
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
            Column::Testing,
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
        assert_eq!(AgentRole::all().len(), 7);
    }

    #[test]
    fn agent_role_column_roundtrip() {
        for role in AgentRole::all() {
            let col = role.column();
            assert_eq!(col.agent_role(), Some(*role));
        }
    }
}
