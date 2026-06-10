#[cfg(feature = "dashboard")]
pub mod imp {
    use axum::{
        extract::{Path, State},
        response::{Html, IntoResponse, Sse},
        Form,
    };
    use serde::Deserialize;
    use std::sync::Arc;
    use crate::model::Stage;
    use crate::state::reader;
    use crate::git::ops as git;
    use super::super::server::imp::AppState;
    use super::super::sse::imp::to_sse_stream;

    pub async fn board(State(state): State<Arc<AppState>>) -> impl IntoResponse {
        let tasks = reader::load_all_tasks().unwrap_or_default();
        let epics = reader::load_all_epics().unwrap_or_default();
        let html = state.env.get_template("board.html").unwrap()
            .render(minijinja::context! {
                tasks => minijinja::Value::from_serialize(&tasks),
                epics => minijinja::Value::from_serialize(&epics),
            })
            .unwrap_or_else(|e| format!("Template error: {e}"));
        Html(html)
    }

    pub async fn task_detail(
        Path(id): Path<String>,
        State(state): State<Arc<AppState>>,
    ) -> impl IntoResponse {
        let task = match reader::load_task(&id) {
            Ok(t) => t,
            Err(_) => return Html(format!("<h1>Task {id} not found</h1>")),
        };
        let diff = task.branch.as_deref().and_then(|branch| {
            git::open_repo(&state.repo_root)
                .and_then(|repo| git::get_diff(&repo, branch))
                .ok()
        });
        let html = state.env.get_template("task_detail.html").unwrap()
            .render(minijinja::context! {
                task => minijinja::Value::from_serialize(&task),
                diff => diff,
            })
            .unwrap_or_else(|e| format!("Template error: {e}"));
        Html(html)
    }

    #[derive(Deserialize)]
    pub struct RejectForm {
        feedback: String,
    }

    pub async fn approve_task(
        Path(id): Path<String>,
        State(state): State<Arc<AppState>>,
    ) -> impl IntoResponse {
        let result = state.orchestrator.advance_task(&id, Stage::InTesting, Some("approved".into()));
        match result {
            Ok(_) => Html(format!("<p>Task {id} approved and moved to testing.</p><a href='/'>Back</a>")),
            Err(e) => Html(format!("<p>Error: {e}</p><a href='/'>Back</a>")),
        }
    }

    pub async fn reject_task(
        Path(id): Path<String>,
        State(state): State<Arc<AppState>>,
        Form(form): Form<RejectForm>,
    ) -> impl IntoResponse {
        let result = state.orchestrator.reject_task(&id, &form.feedback);
        match result {
            Ok(_) => Html(format!("<p>Task {id} sent back to agent with feedback.</p><a href='/'>Back</a>")),
            Err(e) => Html(format!("<p>Error: {e}</p><a href='/'>Back</a>")),
        }
    }

    pub async fn sse_events(
        State(state): State<Arc<AppState>>,
    ) -> Sse<impl tokio_stream::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
        let rx = state.tx.subscribe();
        Sse::new(to_sse_stream(rx))
    }
}
