use crate::model::{Epic, Stage, Task};
use crate::model::task::new_id;
use crate::state::{chat, home, reader, writer};
use crate::state::chat::ChatMessage;
use crate::error::Result;

pub const STAGES: &[Stage] = &[
    Stage::Backlog,
    Stage::Prioritized,
    Stage::InProgress,
    Stage::WaitingForReview,
    Stage::InTesting,
    Stage::Done,
];

#[derive(PartialEq)]
pub enum Focus {
    Chat,
    Panel,
}

pub enum RightPanel {
    Board,
    EpicDetail(String),  // epic id
    TaskDetail(String),  // task id
}

pub struct App {
    // Data
    pub epics: Vec<Epic>,
    pub tasks: Vec<Task>,

    // Chat
    pub messages: Vec<ChatMessage>,
    pub input: String,
    pub chat_scroll: usize,  // lines offset from bottom (0 = newest visible)

    // Right panel
    pub panel: RightPanel,
    pub board_col: usize,
    pub board_task: usize,

    // Focus
    pub focus: Focus,
}

impl App {
    pub fn load() -> Result<Self> {
        let mut epics = reader::load_all_epics().unwrap_or_default();
        epics.sort_by(|a, b| a.title.cmp(&b.title));
        let tasks = reader::load_all_tasks().unwrap_or_default();

        let messages = if let Ok(path) = home::chat_file() {
            chat::load(&path).unwrap_or_default()
        } else {
            Vec::new()
        };

        let mut app = Self {
            epics,
            tasks,
            messages,
            input: String::new(),
            chat_scroll: 0,
            panel: RightPanel::Board,
            board_col: 0,
            board_task: 0,
            focus: Focus::Chat,
        };

        if app.messages.is_empty() {
            let msg = ChatMessage::system(
                "Session ready. Commands: board | epic <title> | task <title> | open <id>",
            );
            app.add_message(msg);
        }

        Ok(app)
    }

    pub fn reload(&mut self) -> Result<()> {
        let mut epics = reader::load_all_epics()?;
        epics.sort_by(|a, b| a.title.cmp(&b.title));
        self.tasks = reader::load_all_tasks()?;
        self.epics = epics;
        // Clamp board_col
        if self.board_col >= STAGES.len() {
            self.board_col = STAGES.len().saturating_sub(1);
        }
        // Clamp board_task
        let n = self.tasks_in_stage(&STAGES[self.board_col]).len();
        self.board_task = if n > 0 { self.board_task.min(n - 1) } else { 0 };
        Ok(())
    }

    pub fn process_input(&mut self) -> Result<()> {
        let raw = std::mem::take(&mut self.input);
        let input = raw.trim().to_string();
        if input.is_empty() {
            return Ok(());
        }

        self.add_message(ChatMessage::user(&input));

        let (cmd, rest) = match input.find(' ') {
            Some(pos) => (&input[..pos], input[pos + 1..].trim()),
            None => (input.as_str(), ""),
        };

        match cmd {
            "board" => {
                self.panel = RightPanel::Board;
                self.add_message(ChatMessage::system("Showing board"));
            }
            "epic" if !rest.is_empty() => {
                let id = new_id();
                let epic = Epic::new(&id, rest);
                let _ = writer::write_epic(&epic);
                self.epics.push(epic);
                self.epics.sort_by(|a, b| a.title.cmp(&b.title));
                let reply = format!("Created epic {id}: {rest}");
                self.panel = RightPanel::EpicDetail(id);
                self.add_message(ChatMessage::system(reply));
            }
            "task" if !rest.is_empty() => {
                let id = new_id();
                let epic_id = match &self.panel {
                    RightPanel::EpicDetail(eid) => eid.clone(),
                    _ => self.epics.first().map(|e| e.id.clone()).unwrap_or_else(|| "?".to_string()),
                };
                let task = Task::new(&id, &epic_id, rest);
                let _ = writer::write_task(&task);
                let _ = self.reload();
                let reply = format!("Created task {id}: {rest}");
                self.add_message(ChatMessage::system(reply));
            }
            "open" if !rest.is_empty() => {
                let prefix = rest;
                if let Some(epic) = self.epics.iter().find(|e| e.id.starts_with(prefix)) {
                    let eid = epic.id.clone();
                    let title = epic.title.clone();
                    self.panel = RightPanel::EpicDetail(eid.clone());
                    self.add_message(ChatMessage::system(format!("Showing epic {eid}: {title}")));
                } else if let Some(task) = self.tasks.iter().find(|t| t.id.starts_with(prefix)) {
                    let tid = task.id.clone();
                    let title = task.title.clone();
                    self.panel = RightPanel::TaskDetail(tid.clone());
                    self.add_message(ChatMessage::system(format!("Showing task {tid}: {title}")));
                } else {
                    self.add_message(ChatMessage::system(format!("Not found: {prefix}")));
                }
            }
            _ => {
                self.add_message(ChatMessage::system(
                    "Unknown command. Try: board | epic <title> | task <title> | open <id>",
                ));
            }
        }

        self.chat_scroll = 0;
        Ok(())
    }

    pub fn add_message(&mut self, msg: ChatMessage) {
        if let Ok(path) = home::chat_file() {
            let _ = chat::append(&path, &msg);
        }
        self.messages.push(msg);
        self.chat_scroll = 0;
    }

    pub fn scroll_chat_up(&mut self) {
        self.chat_scroll += 1;
    }

    pub fn scroll_chat_down(&mut self) {
        if self.chat_scroll > 0 {
            self.chat_scroll -= 1;
        }
    }

    pub fn move_up(&mut self) {
        if self.focus != Focus::Panel {
            return;
        }
        match &self.panel {
            RightPanel::Board => {
                if self.board_task > 0 {
                    self.board_task -= 1;
                }
            }
            _ => {}
        }
    }

    pub fn move_down(&mut self) {
        if self.focus != Focus::Panel {
            return;
        }
        match &self.panel {
            RightPanel::Board => {
                let n = self.tasks_in_stage(&STAGES[self.board_col]).len();
                if n > 0 && self.board_task < n - 1 {
                    self.board_task += 1;
                }
            }
            _ => {}
        }
    }

    pub fn move_left(&mut self) {
        if self.focus != Focus::Panel {
            return;
        }
        if let RightPanel::Board = &self.panel {
            if self.board_col > 0 {
                self.board_col -= 1;
                self.board_task = 0;
            }
        }
    }

    pub fn move_right(&mut self) {
        if self.focus != Focus::Panel {
            return;
        }
        if let RightPanel::Board = &self.panel {
            if self.board_col < STAGES.len() - 1 {
                self.board_col += 1;
                self.board_task = 0;
            }
        }
    }

    pub fn tasks_in_stage(&self, stage: &Stage) -> Vec<&Task> {
        let mut filtered: Vec<&Task> = self.tasks.iter()
            .filter(|t| &t.stage == stage)
            .collect();
        filtered.sort_by_key(|t| t.priority);
        filtered
    }
}
