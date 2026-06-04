use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use crate::model::task::Stage;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WipSnapshot {
    pub stages: HashMap<String, Vec<String>>,
}

impl WipSnapshot {
    pub fn task_ids_in(&self, stage: &Stage) -> &[String] {
        self.stages
            .get(&stage.to_string())
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub fn count_in(&self, stage: &Stage) -> usize {
        self.task_ids_in(stage).len()
    }
}
