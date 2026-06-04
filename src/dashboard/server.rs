#[cfg(feature = "dashboard")]
pub mod imp {
    use std::path::PathBuf;
    use std::sync::Arc;
    use axum::{routing::{get, post}, Router};
    use minijinja::Environment;
    use tokio::sync::broadcast;
    use crate::pipeline::Orchestrator;
    use crate::state::watcher::StateEvent;
    use super::super::handlers::imp::*;

    pub struct AppState {
        pub repo_root: PathBuf,
        pub orchestrator: Arc<Orchestrator>,
        pub tx: broadcast::Sender<StateEvent>,
        pub env: Environment<'static>,
    }

    pub fn build_router(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/", get(board))
            .route("/tasks/{id}", get(task_detail))
            .route("/tasks/{id}/approve", post(approve_task))
            .route("/tasks/{id}/reject", post(reject_task))
            .route("/events", get(sse_events))
            .with_state(state)
    }

    pub fn build_env() -> Environment<'static> {
        let mut env = Environment::new();
        env.add_template("board.html", include_str!("templates/board.html")).unwrap();
        env.add_template("task_detail.html", include_str!("templates/task_detail.html")).unwrap();
        env
    }
}
