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

// ─── Editor target type ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorTarget {
    Epic,
    Task,
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

    /// Whether the right panel currently has keyboard focus
    pub panel_focused: bool,

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

    /// Editor buffer for the right panel (Epic/Task editing)
    pub editor_buffer: String,
    /// Whether an LLM provider is available (false = hide chat, show error)
    pub llm_available: bool,

    /// Discovered providers (for first-time status display)
    pub discovered_providers: Vec<(String, bool)>,  // (name, is_running)

    /// Discovered repo summaries (for first-time status display)
    pub discovered_repos: Vec<(String, String)>,  // (path, summary)

    /// Whether the editor is focused (Tab was pressed)
    pub editor_focused: bool,
    /// Whether the editor has unsaved changes
    pub editor_dirty: bool,
    /// Whether Esc warning has been shown (second Esc discards)
    pub editor_esc_warned: bool,
    /// The ID of what's being edited (epic or task id)
    pub editor_target_id: Option<String>,
    /// Whether we're editing an epic or task
    pub editor_target_type: Option<EditorTarget>,
}

/// Commands sent from the TUI event loop to the daemon client task.
#[derive(Debug)]
pub enum DaemonCommand {
    Chat { message: String, view_context: ViewContext },
    CloseFork,
    RefreshBoard,
    RefreshReview,
    RefreshEpics,
    RefreshQueueLen,
    UpdateEntity { id: String, target: EditorTarget, title: String, description: Option<String> },
}

/// Describes what the user is currently looking at in the TUI.
/// Sent alongside each chat message so the main agent can discuss
/// on-screen content without the user having to describe it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ViewContext {
    /// Which panel is currently displayed on the right side.
    pub panel: String,
    /// The selected task's ID (if any task is highlighted on the board).
    pub selected_task_id: Option<String>,
    /// The selected task's title (for convenience — avoids a lookup).
    pub selected_task_title: Option<String>,
    /// The board column the user is looking at.
    pub selected_column: Option<String>,
    /// Number of pending review items (questions from agents).
    pub pending_questions: usize,
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
            panel_focused: false,
            chat_input: String::new(),
            chat_history: Vec::new(),
            logs: Vec::new(),
            board_col: 0,
            board_task: 0,
            status: "connecting…".to_string(),
            cmd_tx,
            connected: false,
            llm_available: true,
            discovered_providers: Vec::new(),
            discovered_repos: Vec::new(),
            editor_buffer: String::new(),
            editor_focused: false,
            editor_dirty: false,
            editor_esc_warned: false,
            editor_target_id: None,
            editor_target_type: None,
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
    /// Works with visible columns (buffer tasks merged into their preceding work column).
    pub fn selected_board_task(&self) -> Option<&BoardTask> {
        let vis_idx = self.board_col_visible();
        let tasks = self.tasks_in_visible_column(vis_idx);
        tasks.get(self.board_task).copied()
    }

    /// Append a log entry (capped at 500 lines).
    pub fn push_log(&mut self, line: String) {
        self.logs.push(line);
        if self.logs.len() > 500 {
            self.logs.drain(..self.logs.len() - 500);
        }
    }

    /// Map board_col (which indexes all columns) to a visible-only index.
    /// The board only displays non-buffer columns, so this maps the internal
    /// col index to the display index used by draw_board.
    pub fn board_col_visible(&self) -> usize {
        let visible: Vec<&Column> = column_order()
            .iter()
            .filter(|c| !c.is_buffer())
            .collect();
        let current_col = &column_order()[self.board_col];
        // If we're on a buffer column, find its preceding work column
        let target = if current_col.is_buffer() {
            current_col.prev().unwrap_or(*current_col)
        } else {
            *current_col
        };
        visible.iter().position(|c| **c == target).unwrap_or(0)
    }

    /// Get the combined tasks for a visible column (work column tasks + its
    /// buffer column tasks). Buffer tasks come first.
    pub fn tasks_in_visible_column(&self, visible_idx: usize) -> Vec<&BoardTask> {
        let visible: Vec<&Column> = column_order()
            .iter()
            .filter(|c| !c.is_buffer())
            .collect();
        let Some(col) = visible.get(visible_idx) else {
            return Vec::new();
        };
        let mut result: Vec<&BoardTask> = Vec::new();
        // Buffer tasks first
        if let Some(buffer_col) = col.next().filter(|c| c.is_buffer())
            && let Some(tasks) = self.board.get(&buffer_col.to_string())
        {
            result.extend(tasks.iter());
        }
        // Own tasks
        if let Some(tasks) = self.board.get(&col.to_string()) {
            result.extend(tasks.iter());
        }
        result
    }



    // ── Chat ──────────────────────────────────────────────────────────────────

    /// Submit the current chat message with visual context.
    pub fn submit_chat(&mut self) {
        let message = std::mem::take(&mut self.chat_input).trim().to_string();
        if message.is_empty() {
            return;
        }
        self.chat_history.push(ChatEntry { is_user: true, content: message.clone() });

        let selected_task = self.selected_board_task();
        let view_context = ViewContext {
            panel: self.panel.label().to_string(),
            selected_task_id: selected_task.map(|t| t.id.clone()),
            selected_task_title: selected_task.map(|t| t.title.clone()),
            selected_column: column_order()
                .get(self.board_col)
                .map(|c| c.to_string()),
            pending_questions: self.question_count,
        };

        let _ = self.cmd_tx.send(DaemonCommand::Chat { message, view_context });
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

    // Navigation — only traverses visible (non-buffer) columns
    pub fn move_left(&mut self) {
        let vis_idx = self.board_col_visible();
        if vis_idx > 0 {
            self.board_col = self.visible_to_all_index(vis_idx - 1);
            self.board_task = 0;
        }
    }

    pub fn move_right(&mut self) {
        let visible_count = self.visible_column_count();
        let vis_idx = self.board_col_visible();
        if vis_idx < visible_count - 1 {
            self.board_col = self.visible_to_all_index(vis_idx + 1);
            self.board_task = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.board_task > 0 {
            self.board_task -= 1;
        }
    }

    pub fn move_down(&mut self) {
        let vis_idx = self.board_col_visible();
        let n = self.tasks_in_visible_column(vis_idx).len();
        if n > 0 && self.board_task < n - 1 {
            self.board_task += 1;
        }
    }

    /// Cycle to the previous panel (Ctrl+Up).
    pub fn prev_panel(&mut self) {
        let idx = Panel::ALL.iter().position(|p| *p == self.panel).unwrap_or(0);
        self.panel = if idx == 0 {
            Panel::ALL[Panel::ALL.len() - 1]
        } else {
            Panel::ALL[idx - 1]
        };
    }

    /// Cycle to the next panel (Ctrl+Down).
    pub fn next_panel(&mut self) {
        let idx = Panel::ALL.iter().position(|p| *p == self.panel).unwrap_or(0);
        self.panel = if idx >= Panel::ALL.len() - 1 {
            Panel::ALL[0]
        } else {
            Panel::ALL[idx + 1]
        };
    }

    fn clamp_board_nav(&mut self) {
        let visible_count = self.visible_column_count();
        let vis_idx = self.board_col_visible();
        if vis_idx >= visible_count {
            self.board_col = self.visible_to_all_index(visible_count.saturating_sub(1));
        }
        let n = self.tasks_in_visible_column(self.board_col_visible()).len();
        if n == 0 {
            self.board_task = 0;
        } else if self.board_task >= n {
            self.board_task = n - 1;
        }
    }

    /// Number of visible (non-buffer) columns.
    fn visible_column_count(&self) -> usize {
        column_order().iter().filter(|c| !c.is_buffer()).count()
    }

    /// Convert a visible-column index to an all-columns index.
    fn visible_to_all_index(&self, visible_idx: usize) -> usize {
        column_order()
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.is_buffer())
            .nth(visible_idx)
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    // ── Editor ────────────────────────────────────────────────────────────────

    /// Enter editor mode. Populates buffer from current panel content.
    pub fn enter_editor(&mut self) {
        match self.panel {
            Panel::Epic => {
                if let Some(epic_status) = self.current_epic_status() {
                    if let Some(epic) =
                        self.epics.iter().find(|e| e.title == epic_status.title)
                    {
                        let desc = epic.description.as_deref().unwrap_or("");
                        self.editor_buffer = format!(
                            "# Title\n{}\n\n# Description\n{}",
                            epic.title, desc
                        );
                        self.editor_target_id = Some(epic.id.clone());
                        self.editor_target_type = Some(EditorTarget::Epic);
                        self.editor_focused = true;
                        self.editor_dirty = false;
                        self.editor_esc_warned = false;
                    }
                }
            }
            Panel::Task => {
                if let Some(task) = self.selected_board_task() {
                    let task_title = task.title.clone();
                    let task_id = task.id.clone();
                    self.editor_buffer =
                        format!("# Title\n{}\n\n# Description\n", task_title);
                    self.editor_target_id = Some(task_id);
                    self.editor_target_type = Some(EditorTarget::Task);
                    self.editor_focused = true;
                    self.editor_dirty = false;
                    self.editor_esc_warned = false;
                }
            }
            _ => {}
        }
    }

    /// Save the editor buffer to disk via daemon command.
    pub fn save_editor(&mut self) {
        if let (Some(id), Some(target)) =
            (&self.editor_target_id, self.editor_target_type)
        {
            let (title, description) = parse_editor_buffer(&self.editor_buffer);
            let _ = self.cmd_tx.send(DaemonCommand::UpdateEntity {
                id: id.clone(),
                target,
                title,
                description,
            });
            self.editor_dirty = false;
            self.editor_esc_warned = false;
            self.status = "saved".to_string();
        }
    }

    /// Handle Esc in editor mode.
    pub fn editor_esc(&mut self) {
        if !self.editor_dirty {
            self.editor_focused = false;
        } else if self.editor_esc_warned {
            self.editor_focused = false;
            self.editor_dirty = false;
            self.editor_esc_warned = false;
        } else {
            self.editor_esc_warned = true;
            self.status =
                "unsaved changes \u{2014} Ctrl+S to save, Esc to discard".to_string();
        }
    }
}

/// Parse the editor buffer into (title, optional description).
fn parse_editor_buffer(buf: &str) -> (String, Option<String>) {
    let mut title = String::new();
    let mut description = String::new();
    let mut section = "";

    for line in buf.lines() {
        if line.trim() == "# Title" {
            section = "title";
            continue;
        }
        if line.trim() == "# Description" {
            section = "description";
            continue;
        }
        match section {
            "title" => {
                if !line.trim().is_empty() || !title.is_empty() {
                    if !title.is_empty() {
                        title.push('\n');
                    }
                    title.push_str(line);
                }
            }
            "description" => {
                if !description.is_empty() {
                    description.push('\n');
                }
                description.push_str(line);
            }
            _ => {}
        }
    }

    let title = title.trim().to_string();
    let desc = description.trim().to_string();
    (title, if desc.is_empty() { None } else { Some(desc) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_editor_buffer_basic() {
        let buf = "# Title\nMy Epic\n\n# Description\nSome description text";
        let (title, desc) = parse_editor_buffer(buf);
        assert_eq!(title, "My Epic");
        assert_eq!(desc, Some("Some description text".to_string()));
    }

    #[test]
    fn parse_editor_buffer_empty_description() {
        let buf = "# Title\nTask Name\n\n# Description\n";
        let (title, desc) = parse_editor_buffer(buf);
        assert_eq!(title, "Task Name");
        assert_eq!(desc, None);
    }

    #[test]
    fn parse_editor_buffer_multiline_description() {
        let buf = "# Title\nHello\n\n# Description\nLine 1\nLine 2\nLine 3";
        let (title, desc) = parse_editor_buffer(buf);
        assert_eq!(title, "Hello");
        assert_eq!(desc, Some("Line 1\nLine 2\nLine 3".to_string()));
    }
}
