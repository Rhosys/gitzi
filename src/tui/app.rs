use std::collections::HashMap;

use serde::Deserialize;
use tokio::sync::mpsc;

use crate::dispatcher::Column;

// ─── Board snapshot types (deserialized from daemon JSON) ─────────────────────

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct BoardTask {
    pub id: String,
    #[serde(default)]
    pub epic: String,
    pub title: String,
    pub priority: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BoardColumn {
    pub column: String,
    pub tasks: Vec<BoardTask>,
}

// ─── Review item (deserialized from daemon `peek_review` JSON) ────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct ReviewItem {
    pub id: String,
    pub task_id: String,
    pub kind: ReviewItemKind,
    /// Human-readable task title looked up by the daemon at serialization time.
    #[serde(default)]
    pub task_title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReviewItemKind {
    AgentQuestion { question: String },
    BufferApproval { buffer_column: String, task_priority: u32 },
}

// ─── Chat entry (for local display) ──────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ChatEntry {
    pub is_user: bool,
    pub content: String,
}

// ─── TUI Mode ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Structured status card — default landing view on open
    Status,
    /// Board overview — idle, no review items pending
    Idle,
    /// Showing a review item with controls
    Review,
    /// Typing rejection feedback
    RejectInput,
    /// Typing answer to agent question
    AnswerInput,
    /// Typing a chat message to the main agent
    ChatInput,
    /// Waiting for the main agent to respond
    ChatWaiting,
}

// ─── Column abbreviations for compact rendering ───────────────────────────────

pub fn column_order() -> &'static [Column] {
    Column::all()
}

pub fn column_abbrev(col: &Column) -> &'static str {
    match col {
        Column::Prioritized => "PRI",
        Column::Designing => "DES",
        Column::CodingBuffer => "C·B",
        Column::Coding => "COD",
        Column::ReviewBuffer => "R·B",
        Column::Reviewing => "REV",
        Column::TestBuffer => "T·B",
        Column::Testing => "TST",
        Column::SecurityAuditBuffer => "A·B",
        Column::Auditing => "AUD",
        Column::DeploymentBuffer => "D·B",
        Column::Deploying => "DEP",
        Column::Done => "DON",
    }
}

// ─── Status card summary ───────────────────────────────────────────────────────

/// Computed progress for the epic with the most outstanding work.
pub struct EpicStatus {
    pub title: String,
    pub done: usize,
    pub total: usize,
}

// ─── App state ────────────────────────────────────────────────────────────────

pub struct App {
    /// Board state: column name → list of tasks
    pub board: HashMap<String, Vec<BoardTask>>,

    /// All epics (id, title, child task IDs) — used by the Status panel.
    pub epics: Vec<crate::model::Epic>,

    /// Count of pending agent-question review items (clarification queue size).
    pub question_count: usize,

    /// Current topmost review item (from peek_review)
    pub review_item: Option<ReviewItem>,

    /// Current mode
    pub mode: Mode,

    /// Input buffer for reject feedback / answer text (review flows)
    pub input: String,

    /// Chat message being composed
    pub chat_input: String,

    /// Chat history displayed in the left pane
    pub chat_history: Vec<ChatEntry>,

    /// Board navigation: selected column index
    pub board_col: usize,

    /// Board navigation: selected task index within column
    pub board_task: usize,

    /// Status message shown in footer
    pub status: String,

    /// Channel to send commands to the daemon client task
    pub cmd_tx: mpsc::UnboundedSender<DaemonCommand>,

    /// Whether we're connected to the daemon
    pub connected: bool,
}

/// Commands sent from the TUI event loop to the daemon client task.
#[derive(Debug)]
pub enum DaemonCommand {
    Approve(String),            // task_id
    Reject(String, String),     // task_id, feedback
    Answer(String, String),     // item_id, answer
    Chat(String),               // message to main agent
    RefreshBoard,
    RefreshReview,
    RefreshEpics,
    RefreshQueueLen,
}

/// Messages received from the daemon client task into the TUI event loop.
#[derive(Debug)]
pub enum DaemonMessage {
    BoardSnapshot(Vec<BoardColumn>),
    ReviewItem(Option<ReviewItem>),
    Epics(Vec<crate::model::Epic>),
    QueueLen(usize),
    ChatHistory(Vec<ChatEntry>),
    ChatResponse(String),
    Event(String),  // raw JSON line from subscribe stream
    SwitchPanel(String),
    Connected,
    Disconnected(String),
    CommandResult(std::result::Result<String, String>),
}

impl App {
    pub fn new(cmd_tx: mpsc::UnboundedSender<DaemonCommand>) -> Self {
        Self {
            board: HashMap::new(),
            epics: Vec::new(),
            question_count: 0,
            review_item: None,
            mode: Mode::Status,
            input: String::new(),
            chat_input: String::new(),
            chat_history: Vec::new(),
            board_col: 0,
            board_task: 0,
            status: "connecting…".to_string(),
            cmd_tx,
            connected: false,
        }
    }

    /// Apply a board snapshot from the daemon.
    pub fn apply_board_snapshot(&mut self, columns: Vec<BoardColumn>) {
        self.board.clear();
        for col in columns {
            self.board.insert(col.column, col.tasks);
        }
        // Clamp navigation indices
        self.clamp_board_nav();
    }

    /// Apply a review item update.
    pub fn apply_review_item(&mut self, item: Option<ReviewItem>) {
        self.review_item = item;
        // Update mode based on review state
        if self.review_item.is_some() && matches!(self.mode, Mode::Idle | Mode::Status) {
            self.mode = Mode::Review;
        } else if self.review_item.is_none() && self.mode == Mode::Review {
            self.mode = Mode::Idle;
        }
    }

    /// Apply a panel switch command from the main agent.
    pub fn apply_panel_switch(&mut self, view: &str) {
        match view {
            "status" => self.mode = Mode::Status,
            "board" => self.mode = Mode::Idle,
            "review" => self.mode = Mode::Review,
            _ => {} // ignore unknown views
        }
    }

    /// Apply a fresh epics list from the daemon.
    pub fn apply_epics(&mut self, epics: Vec<crate::model::Epic>) {
        self.epics = epics;
    }

    /// Apply a fresh clarification-queue count from the daemon.
    pub fn apply_queue_len(&mut self, count: usize) {
        self.question_count = count;
    }

    /// The epic with the most outstanding (non-Done) work, with its progress.
    /// Used by the Status panel's "Current epic" section.
    pub fn current_epic_status(&self) -> Option<EpicStatus> {
        let done_ids: std::collections::HashSet<&str> = self
            .board
            .get(&Column::Done.to_string())
            .map(|tasks| tasks.iter().map(|t| t.id.as_str()).collect())
            .unwrap_or_default();

        self.epics
            .iter()
            .filter(|e| !e.tasks.is_empty())
            .max_by_key(|e| {
                let total = e.tasks.len();
                let done = e.tasks.iter().filter(|id| done_ids.contains(id.as_str())).count();
                (total - done, total)
            })
            .map(|e| {
                let total = e.tasks.len();
                let done = e.tasks.iter().filter(|id| done_ids.contains(id.as_str())).count();
                EpicStatus { title: e.title.clone(), done, total }
            })
    }

    /// Tasks currently in a work column (an agent is actively on them).
    pub fn tasks_in_progress(&self) -> Vec<&BoardTask> {
        column_order()
            .iter()
            .filter(|c| c.agent_role().is_some())
            .flat_map(|c| self.board.get(&c.to_string()).into_iter().flatten())
            .collect()
    }

    /// Tasks sitting in a buffer column awaiting human approval/rejection.
    pub fn tasks_waiting_for_you(&self) -> Vec<&BoardTask> {
        column_order()
            .iter()
            .filter(|c| c.is_buffer())
            .flat_map(|c| self.board.get(&c.to_string()).into_iter().flatten())
            .collect()
    }

    /// Handle an incoming dispatch event — refresh everything the Status panel
    /// and board depend on.
    pub fn handle_event(&mut self, _raw_json: &str) {
        // On any event, request fresh state from daemon
        let _ = self.cmd_tx.send(DaemonCommand::RefreshBoard);
        let _ = self.cmd_tx.send(DaemonCommand::RefreshReview);
        let _ = self.cmd_tx.send(DaemonCommand::RefreshEpics);
        let _ = self.cmd_tx.send(DaemonCommand::RefreshQueueLen);
    }

    /// Get tasks for a given column index.
    pub fn tasks_in_column(&self, col_idx: usize) -> &[BoardTask] {
        let col = &column_order()[col_idx];
        let key = col.to_string();
        self.board.get(&key).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Approve the current review item.
    pub fn approve_current(&mut self) {
        if let Some(ref item) = self.review_item {
            let _ = self.cmd_tx.send(DaemonCommand::Approve(item.task_id.clone()));
            self.status = format!("approving {}…", &item.task_id[..8.min(item.task_id.len())]);
        }
    }

    /// Begin reject flow — switch to input mode.
    pub fn begin_reject(&mut self) {
        if self.review_item.is_some() {
            self.mode = Mode::RejectInput;
            self.input.clear();
        }
    }

    /// Submit rejection with feedback.
    pub fn submit_reject(&mut self) {
        if let Some(ref item) = self.review_item {
            let feedback = std::mem::take(&mut self.input);
            if feedback.trim().is_empty() {
                self.status = "feedback required".to_string();
                return;
            }
            let _ = self.cmd_tx.send(DaemonCommand::Reject(item.task_id.clone(), feedback));
            self.status = format!("rejecting {}…", &item.task_id[..8.min(item.task_id.len())]);
            self.mode = Mode::Review;
        }
    }

    /// Begin answer flow — switch to input mode.
    pub fn begin_answer(&mut self) {
        if self.review_item.is_some() {
            self.mode = Mode::AnswerInput;
            self.input.clear();
        }
    }

    /// Submit answer to agent question.
    pub fn submit_answer(&mut self) {
        if let Some(ref item) = self.review_item {
            let answer = std::mem::take(&mut self.input);
            if answer.trim().is_empty() {
                self.status = "answer required".to_string();
                return;
            }
            let _ = self.cmd_tx.send(DaemonCommand::Answer(item.id.clone(), answer));
            self.status = "answering…".to_string();
            self.mode = Mode::Review;
        }
    }

    /// Cancel input mode, return to review.
    pub fn cancel_input(&mut self) {
        self.input.clear();
        self.mode = if self.review_item.is_some() { Mode::Review } else { Mode::Idle };
    }

    // ── Chat ──────────────────────────────────────────────────────────────────

    /// Enter chat input mode.
    pub fn begin_chat(&mut self) {
        self.mode = Mode::ChatInput;
    }

    /// Cancel chat input, return to board mode.
    pub fn cancel_chat(&mut self) {
        self.chat_input.clear();
        self.mode = if self.review_item.is_some() { Mode::Review } else { Mode::Idle };
    }

    /// Submit the current chat message.
    pub fn submit_chat(&mut self) {
        let message = std::mem::take(&mut self.chat_input).trim().to_string();
        if message.is_empty() {
            return;
        }
        // Show immediately in local history
        self.chat_history.push(ChatEntry { is_user: true, content: message.clone() });
        // Send to daemon
        let _ = self.cmd_tx.send(DaemonCommand::Chat(message));
        self.mode = Mode::ChatWaiting;
        self.status = "thinking…".to_string();
    }

    /// Apply a chat response from the daemon.
    pub fn apply_chat_response(&mut self, response: String) {
        self.chat_history.push(ChatEntry { is_user: false, content: response });
        self.status = String::new();
        self.mode = if self.review_item.is_some() { Mode::Review } else { Mode::Idle };
    }

    /// Apply loaded chat history from the daemon.
    pub fn apply_chat_history(&mut self, entries: Vec<ChatEntry>) {
        self.chat_history = entries;
    }

    // Navigation
    pub fn move_left(&mut self) {
        if self.board_col > 0 {
            self.board_col -= 1;
            self.board_task = 0;
        }
    }

    pub fn move_right(&mut self) {
        if self.board_col < column_order().len() - 1 {
            self.board_col += 1;
            self.board_task = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.board_task > 0 {
            self.board_task -= 1;
        }
    }

    pub fn move_down(&mut self) {
        let n = self.tasks_in_column(self.board_col).len();
        if n > 0 && self.board_task < n - 1 {
            self.board_task += 1;
        }
    }

    fn clamp_board_nav(&mut self) {
        if self.board_col >= column_order().len() {
            self.board_col = column_order().len().saturating_sub(1);
        }
        let n = self.tasks_in_column(self.board_col).len();
        if n == 0 {
            self.board_task = 0;
        } else if self.board_task >= n {
            self.board_task = n - 1;
        }
    }
}
