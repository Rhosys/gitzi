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

// ─── Chat entry (for local display) ──────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ChatEntry {
    pub is_user: bool,
    pub content: String,
}

// ─── Fork info (received from daemon events) ─────────────────────────────────

/// Display info for an active fork (received from daemon events).
#[derive(Debug, Clone)]
pub struct ForkInfo {
    pub id: String,
    pub name: String,
}

// ─── TUI Mode ─────────────────────────────────────────────────────────────────
// No Mode enum — chat input is always active. Arrow keys navigate the board,
// printable chars go to chat, Enter submits, Backspace deletes, Ctrl+Q quits.

/// Which panel is displayed on the right side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Status,
    Epic,
    Kanban,
    Task,
    Logs,
}

impl Panel {
    pub const ALL: &[Panel] = &[
        Panel::Status,
        Panel::Epic,
        Panel::Kanban,
        Panel::Task,
        Panel::Logs,
    ];

    pub fn key(self) -> char {
        match self {
            Panel::Status => 'S',
            Panel::Epic => 'E',
            Panel::Kanban => 'K',
            Panel::Task => 'T',
            Panel::Logs => 'L',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Panel::Status => "Status",
            Panel::Epic => "Epic",
            Panel::Kanban => "Kanban",
            Panel::Task => "Task",
            Panel::Logs => "Logs",
        }
    }
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
    /// Active fork stack — empty means we're in the main session.
    pub fork_stack: Vec<ForkInfo>,

    /// Board state: column name → list of tasks
    pub board: HashMap<String, Vec<BoardTask>>,

    /// All epics (id, title, child task IDs) — used by the Status panel.
    pub epics: Vec<crate::model::Epic>,

    /// Count of pending agent-question review items (clarification queue size).
    pub question_count: usize,

    /// True while waiting for a chat response from the main agent.
    pub chat_pending: bool,

    /// Which panel is shown on the right
    pub panel: Panel,



    /// Chat message being composed
    pub chat_input: String,

    /// Chat history displayed in the left pane
    pub chat_history: Vec<ChatEntry>,

    /// Log entries for the Logs panel (daemon events, agent activity)
    pub logs: Vec<String>,

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
    Chat(String),               // message to main agent
    CloseFork,
    RefreshBoard,
    RefreshReview,
    RefreshEpics,
    RefreshQueueLen,
}

/// Messages received from the daemon client task into the TUI event loop.
#[derive(Debug)]
pub enum DaemonMessage {
    BoardSnapshot(Vec<BoardColumn>),
    ReviewPending(bool),
    Epics(Vec<crate::model::Epic>),
    QueueLen(usize),
    ChatHistory(Vec<ChatEntry>),
    ChatResponse(String),
    ForkCreated { id: String, name: String },
    ForkClosed { id: String },
    Event(String),  // raw JSON line from subscribe stream
    SwitchPanel(String),
    Connected,
    Disconnected(String),
}

impl App {
    pub fn new(cmd_tx: mpsc::UnboundedSender<DaemonCommand>) -> Self {
        Self {
            fork_stack: Vec::new(),
            board: HashMap::new(),
            epics: Vec::new(),
            question_count: 0,
            chat_pending: false,
            panel: Panel::Status,
            chat_input: String::new(),
            chat_history: Vec::new(),
            logs: Vec::new(),
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

    /// Apply a panel switch command from the main agent.
    pub fn apply_panel_switch(&mut self, view: &str) {
        match view {
            "status" => self.panel = Panel::Status,
            "epic" => self.panel = Panel::Epic,
            "board" | "kanban" => self.panel = Panel::Kanban,
            "task" => self.panel = Panel::Task,
            "logs" => self.panel = Panel::Logs,
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

    /// Get the currently selected task from the board (board_col + board_task).
    pub fn selected_board_task(&self) -> Option<&BoardTask> {
        let tasks = self.tasks_in_column(self.board_col);
        tasks.get(self.board_task)
    }

    /// Append a log entry (capped at 500 lines).
    pub fn push_log(&mut self, line: String) {
        self.logs.push(line);
        if self.logs.len() > 500 {
            self.logs.drain(..self.logs.len() - 500);
        }
    }

    /// Get tasks for a given column index.
    pub fn tasks_in_column(&self, col_idx: usize) -> &[BoardTask] {
        let col = &column_order()[col_idx];
        let key = col.to_string();
        self.board.get(&key).map(|v| v.as_slice()).unwrap_or(&[])
    }



    // ── Chat ──────────────────────────────────────────────────────────────────

    /// Submit the current chat message.
    pub fn submit_chat(&mut self) {
        let message = std::mem::take(&mut self.chat_input).trim().to_string();
        if message.is_empty() {
            return;
        }
        self.chat_history.push(ChatEntry { is_user: true, content: message.clone() });
        let _ = self.cmd_tx.send(DaemonCommand::Chat(message));
        self.chat_pending = true;
        self.status = "thinking…".to_string();
    }

    /// Apply a chat response from the daemon.
    pub fn apply_chat_response(&mut self, response: String) {
        self.chat_history.push(ChatEntry { is_user: false, content: response });
        self.chat_pending = false;
        self.status = String::new();
    }

    /// Apply loaded chat history from the daemon.
    pub fn apply_chat_history(&mut self, entries: Vec<ChatEntry>) {
        self.chat_history = entries;
    }

    // ── Forks ─────────────────────────────────────────────────────────────────

    /// Push a new fork onto the stack.
    pub fn apply_fork_created(&mut self, id: String, name: String) {
        self.fork_stack.push(ForkInfo { id, name });
    }

    /// Remove a fork from the stack by id.
    pub fn apply_fork_closed(&mut self, id: &str) {
        self.fork_stack.retain(|f| f.id != id);
    }

    /// Request the daemon close the current (topmost) fork.
    pub fn close_current_fork(&mut self) {
        if !self.fork_stack.is_empty() {
            let _ = self.cmd_tx.send(DaemonCommand::CloseFork);
        }
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
