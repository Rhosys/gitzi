use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct TokenEntry {
    pub task_id: String,
}

#[derive(Clone, Default)]
pub struct TokenStore(Arc<Mutex<HashMap<String, TokenEntry>>>);

impl TokenStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn issue(&self, task_id: impl Into<String>) -> String {
        let token = Uuid::new_v4().to_string();
        self.0.lock().await.insert(token.clone(), TokenEntry { task_id: task_id.into() });
        token
    }

    pub async fn validate(&self, token: &str) -> Option<TokenEntry> {
        self.0.lock().await.get(token).cloned()
    }

    pub async fn revoke(&self, token: &str) {
        self.0.lock().await.remove(token);
    }
}
