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

use crate::config::Config;
use crate::state::reader;

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
/// review queue, agent pool, and WIP limits. Replaces the polling Scheduler.
pub struct Dispatcher {
    pub event_bus: Arc<EventBus>,
    pub board: Arc<RwLock<KanbanBoard>>,
    pub review_queue: Arc<Mutex<HumanReviewQueue>>,
    pub agent_pool: AgentPool,
    pub config: Arc<Config>,
    pub wip_limits: WipLimits,
    /// Agents waiting to advance into a full column.
    pub wip_waiting: Arc<Mutex<HashMap<Column, AgentRole>>>,
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

    /// Answer an agent's question: dequeue the review item and unblock the agent.
    pub async fn answer_question(&self, item_id: &str, answer: String) -> anyhow::Result<()> {
        // Dequeue the item from the review queue
        let item = {
            let mut queue = self.review_queue.lock().await;
            queue.dequeue(item_id)
                .ok_or_else(|| anyhow::anyhow!("review item '{item_id}' not found in queue"))?
        };

        // Determine which agent role is blocked (from task's current column)
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

        // 4. Create empty HumanReviewQueue
        let review_queue = Arc::new(Mutex::new(HumanReviewQueue::new()));

        // 5. Create WipLimits::default()
        let wip_limits = WipLimits::default();

        // 6. Spawn AgentPool
        let agent_pool = AgentPool::spawn(
            Arc::clone(&event_bus),
            Arc::clone(&board),
            Arc::clone(&config),
        );

        // 7. Emit BootComplete
        event_bus.emit(DispatchEvent::BootComplete);
        info!("boot complete event emitted");

        // 8. Signal all agents that have work in their column
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

        Ok(Self {
            event_bus,
            board,
            review_queue,
            agent_pool,
            config,
            wip_limits,
        })
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

                DispatchEvent::TaskStageChanged { task_id, from: _, to } => {
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
                        let item = review_queue::HumanReviewItem::new(
                            &task_id,
                            review_queue::ReviewItemKind::BufferApproval {
                                buffer_column: to,
                                task_priority: priority,
                            },
                        );
                        let mut q = self.review_queue.lock().await;
                        q.enqueue(item);
                        info!(%task_id, column = %to, "buffer entry — review item created");
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
