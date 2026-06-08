use std::path::PathBuf;
use crate::model::{Epic, Stage, Task};
use crate::state::reader;
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
pub enum Screen {
    Epics,
    Kanban,
}

pub struct App {
    pub epics: Vec<Epic>,
    pub tasks: Vec<Task>,
    pub screen: Screen,
    /// Which epic the kanban is scoped to (None = all tasks)
    pub kanban_epic: Option<String>,
    pub epic_idx: usize,
    pub col_idx: usize,
    pub task_idx: usize,
    repo_root: PathBuf,
}

impl App {
    pub fn load(repo_root: PathBuf) -> Result<Self> {
        let mut epics = reader::load_all_epics(&repo_root)?;
        epics.sort_by(|a, b| a.title.cmp(&b.title));
        let tasks = reader::load_all_tasks(&repo_root)?;
        Ok(Self {
            epics,
            tasks,
            screen: Screen::Epics,
            kanban_epic: None,
            epic_idx: 0,
            col_idx: 0,
            task_idx: 0,
            repo_root,
        })
    }

    pub fn reload(&mut self) -> Result<()> {
        let mut epics = reader::load_all_epics(&self.repo_root)?;
        epics.sort_by(|a, b| a.title.cmp(&b.title));
        self.tasks = reader::load_all_tasks(&self.repo_root)?;
        self.epics = epics;
        if !self.epics.is_empty() {
            self.epic_idx = self.epic_idx.min(self.epics.len() - 1);
        } else {
            self.epic_idx = 0;
        }
        let n = self.tasks_in_stage(&STAGES[self.col_idx]).len();
        self.task_idx = if n > 0 { self.task_idx.min(n - 1) } else { 0 };
        Ok(())
    }

    pub fn tasks_in_stage(&self, stage: &Stage) -> Vec<&Task> {
        let mut filtered: Vec<&Task> = self.tasks.iter()
            .filter(|t| &t.stage == stage)
            .filter(|t| match &self.kanban_epic {
                Some(id) => &t.epic == id,
                None => true,
            })
            .collect();
        filtered.sort_by_key(|t| t.priority);
        filtered
    }

    /// Returns (done, total) task counts for an epic
    pub fn epic_task_counts(&self, epic_id: &str) -> (usize, usize) {
        let total = self.tasks.iter().filter(|t| t.epic == epic_id).count();
        let done = self.tasks.iter()
            .filter(|t| t.epic == epic_id && t.stage == Stage::Done)
            .count();
        (done, total)
    }

    pub fn enter_kanban(&mut self) {
        self.kanban_epic = self.epics.get(self.epic_idx).map(|e| e.id.clone());
        self.screen = Screen::Kanban;
        self.col_idx = 0;
        self.task_idx = 0;
    }

    pub fn exit_kanban(&mut self) {
        self.screen = Screen::Epics;
        self.kanban_epic = None;
    }

    pub fn kanban_epic_title(&self) -> Option<&str> {
        self.kanban_epic.as_deref().and_then(|id| {
            self.epics.iter().find(|e| e.id == id).map(|e| e.title.as_str())
        })
    }

    pub fn move_up(&mut self) {
        match self.screen {
            Screen::Epics => {
                if self.epic_idx > 0 { self.epic_idx -= 1; }
            }
            Screen::Kanban => {
                if self.task_idx > 0 { self.task_idx -= 1; }
            }
        }
    }

    pub fn move_down(&mut self) {
        match self.screen {
            Screen::Epics => {
                if !self.epics.is_empty() && self.epic_idx < self.epics.len() - 1 {
                    self.epic_idx += 1;
                }
            }
            Screen::Kanban => {
                let n = self.tasks_in_stage(&STAGES[self.col_idx]).len();
                if n > 0 && self.task_idx < n - 1 { self.task_idx += 1; }
            }
        }
    }

    pub fn move_left(&mut self) {
        if self.screen == Screen::Kanban && self.col_idx > 0 {
            self.col_idx -= 1;
            self.task_idx = 0;
        }
    }

    pub fn move_right(&mut self) {
        if self.screen == Screen::Kanban && self.col_idx < STAGES.len() - 1 {
            self.col_idx += 1;
            self.task_idx = 0;
        }
    }
}
