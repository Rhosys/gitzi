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
pub enum Focus {
    Epics,
    Kanban,
}

pub struct App {
    pub epics: Vec<Epic>,
    pub tasks: Vec<Task>,
    pub epic_idx: usize,
    pub col_idx: usize,
    pub task_idx: usize,
    pub focus: Focus,
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
            epic_idx: 0,
            col_idx: 0,
            task_idx: 0,
            focus: Focus::Epics,
            repo_root,
        })
    }

    pub fn reload(&mut self) -> Result<()> {
        let mut epics = reader::load_all_epics(&self.repo_root)?;
        epics.sort_by(|a, b| a.title.cmp(&b.title));
        let tasks = reader::load_all_tasks(&self.repo_root)?;
        self.epics = epics;
        self.tasks = tasks;
        // Clamp indices
        if !self.epics.is_empty() {
            self.epic_idx = self.epic_idx.min(self.epics.len() - 1);
        } else {
            self.epic_idx = 0;
        }
        self.col_idx = self.col_idx.min(STAGES.len().saturating_sub(1));
        let tasks_in_col = self.tasks_in_stage(&STAGES[self.col_idx]).len();
        if tasks_in_col > 0 {
            self.task_idx = self.task_idx.min(tasks_in_col - 1);
        } else {
            self.task_idx = 0;
        }
        Ok(())
    }

    pub fn tasks_in_stage(&self, stage: &Stage) -> Vec<&Task> {
        let mut filtered: Vec<&Task> = self.tasks.iter().filter(|t| &t.stage == stage).collect();
        filtered.sort_by_key(|t| t.priority);
        filtered
    }

    pub fn move_up(&mut self) {
        match self.focus {
            Focus::Epics => {
                if self.epic_idx > 0 {
                    self.epic_idx -= 1;
                }
            }
            Focus::Kanban => {
                if self.task_idx > 0 {
                    self.task_idx -= 1;
                }
            }
        }
    }

    pub fn move_down(&mut self) {
        match self.focus {
            Focus::Epics => {
                if !self.epics.is_empty() && self.epic_idx < self.epics.len() - 1 {
                    self.epic_idx += 1;
                }
            }
            Focus::Kanban => {
                let count = self.tasks_in_stage(&STAGES[self.col_idx]).len();
                if count > 0 && self.task_idx < count - 1 {
                    self.task_idx += 1;
                }
            }
        }
    }

    pub fn move_left(&mut self) {
        if self.focus == Focus::Kanban && self.col_idx > 0 {
            self.col_idx -= 1;
            self.task_idx = 0;
        }
    }

    pub fn move_right(&mut self) {
        if self.focus == Focus::Kanban && self.col_idx < STAGES.len() - 1 {
            self.col_idx += 1;
            self.task_idx = 0;
        }
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Epics => Focus::Kanban,
            Focus::Kanban => Focus::Epics,
        };
    }

    /// Returns (done_count, total_count) for an epic
    pub fn epic_task_counts(&self, epic_id: &str) -> (usize, usize) {
        let total = self.tasks.iter().filter(|t| t.epic == epic_id).count();
        let done = self
            .tasks
            .iter()
            .filter(|t| t.epic == epic_id && t.stage == Stage::Done)
            .count();
        (done, total)
    }
}
