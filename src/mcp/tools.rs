use std::sync::Arc;

use serde_json::Value;

use super::auth::TokenEntry;
use crate::dispatcher::Dispatcher;

/// Dispatch a `tools/call` request to the appropriate gitzi_* method on the
/// `Dispatcher`.
///
/// # Authorization
///
/// The caller is expected to have already validated the Bearer token and
/// produced a `TokenEntry` before calling this function.  For write operations
/// that are scoped to a specific task (`gitzi_create_review_item`, `gitzi_update_task`,
/// `gitzi_park_task`) the `entry.task_id` must match the `task_id` argument
/// supplied by the agent.
///
/// # Errors
///
/// Returns `Err(String)` for:
/// - Missing required arguments
/// - Task-ID ownership mismatches
/// - Dispatcher errors (wrapped as strings)
/// - Unknown tool names
pub async fn dispatch(
    name: &str,
    args: &Value,
    entry: &TokenEntry,
    dispatcher: &Arc<Dispatcher>,
) -> Result<Value, String> {
    match name {
        // ── Read operations (no ownership check needed) ───────────────────────

        "gitzi_list_epics" => {
            let epics = dispatcher
                .gitzi_list_epics()
                .await
                .map_err(|e| format!("gitzi_list_epics failed: {e}"))?;
            Ok(serde_json::to_value(epics).unwrap_or(Value::Null))
        }

        "gitzi_list_tasks" => {
            let epic_id = args.get("epic_id").and_then(Value::as_str);
            let tasks = dispatcher
                .gitzi_list_tasks(epic_id)
                .await
                .map_err(|e| format!("gitzi_list_tasks failed: {e}"))?;
            Ok(serde_json::to_value(tasks).unwrap_or(Value::Null))
        }

        "gitzi_get_review_item" => {
            let id = args
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required argument: id".to_string())?;
            let adr = dispatcher
                .gitzi_get_review_item(id)
                .await
                .map_err(|e| format!("gitzi_get_review_item failed: {e}"))?;
            Ok(serde_json::to_value(adr).unwrap_or(Value::Null))
        }

        // ── Create task (no ownership check — creates a *new* task) ──────────

        "gitzi_create_task" => {
            let epic_id = args
                .get("epic_id")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required argument: epic_id".to_string())?
                .to_string();
            let title = args
                .get("title")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required argument: title".to_string())?
                .to_string();
            let description = args
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string);
            let priority = args
                .get("priority")
                .and_then(Value::as_u64)
                .map(|v| v as u32);
            let repo = args
                .get("repo")
                .and_then(Value::as_str)
                .map(str::to_string);
            let task = dispatcher
                .gitzi_create_task(epic_id, title, description, priority, repo)
                .await
                .map_err(|e| format!("gitzi_create_task failed: {e}"))?;
            Ok(serde_json::to_value(task).unwrap_or(Value::Null))
        }

        // ── Write operations scoped to the agent's assigned task ─────────────

        "gitzi_create_review_item" => {
            let task_id = args
                .get("task_id")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required argument: task_id".to_string())?;
            if entry.task_id != task_id {
                return Err(format!(
                    "token is scoped to task '{}' but request targets task '{task_id}'",
                    entry.task_id
                ));
            }
            let question = args
                .get("question")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required argument: question".to_string())?
                .to_string();
            let context = args
                .get("context")
                .and_then(Value::as_str)
                .map(str::to_string);
            let adr = dispatcher
                .gitzi_create_review_item(task_id.to_string(), question, context)
                .await
                .map_err(|e| format!("gitzi_create_review_item failed: {e}"))?;
            Ok(serde_json::to_value(adr).unwrap_or(Value::Null))
        }

        "gitzi_update_task" => {
            let task_id = args
                .get("task_id")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required argument: task_id".to_string())?;
            if entry.task_id != task_id {
                return Err(format!(
                    "token is scoped to task '{}' but request targets task '{task_id}'",
                    entry.task_id
                ));
            }
            let title = args
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string);
            let description = args
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string);
            let task = dispatcher
                .gitzi_update_task(task_id, title, description)
                .await
                .map_err(|e| format!("gitzi_update_task failed: {e}"))?;
            Ok(serde_json::to_value(task).unwrap_or(Value::Null))
        }

        "gitzi_park_task" => {
            let task_id = args
                .get("task_id")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required argument: task_id".to_string())?;
            if entry.task_id != task_id {
                return Err(format!(
                    "token is scoped to task '{}' but request targets task '{task_id}'",
                    entry.task_id
                ));
            }
            let reason = args
                .get("reason")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing required argument: reason".to_string())?
                .to_string();
            dispatcher
                .gitzi_park_task(task_id, reason)
                .await
                .map_err(|e| format!("gitzi_park_task failed: {e}"))?;
            Ok(serde_json::to_value(()).unwrap_or(Value::Null))
        }

        other => Err(format!("unknown tool: {other}")),
    }
}
