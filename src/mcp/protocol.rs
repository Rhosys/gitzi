use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A JSON-RPC 2.0 request.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// A JSON-RPC 2.0 response.
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcResponse {
    pub fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError { code, message: message.into() }),
        }
    }
}

/// A single MCP tool descriptor, serialized with camelCase field names.
#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// Returns the set of gitzi_* tools exposed to sub-agents via MCP.
pub fn sub_agent_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "gitzi_create_epic",
            description: "Create a new epic. An epic is the top-level unit of work — a feature, \
                          initiative, or goal that contains one or more tasks. Supply a clear \
                          title and an optional description. The returned epic ID is what you \
                          pass to gitzi_create_task when breaking the epic into tasks.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short title for the epic (e.g. \"User authentication\")."
                    },
                    "description": {
                        "type": "string",
                        "description": "Optional detailed description of the epic's goal and scope."
                    }
                },
                "required": ["title"]
            }),
        },
        Tool {
            name: "gitzi_list_epics",
            description: "List all epics in the project. Returns an array of epic objects, each \
                          containing id, title, description, and the list of task IDs that belong \
                          to it. Use this to get an overview of the project structure before \
                          drilling into individual tasks.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        Tool {
            name: "gitzi_list_tasks",
            description: "List tasks, optionally filtered to a single epic. Returns an array of \
                          task objects including id, title, description, stage, priority, and \
                          history. Omit epic_id to list all tasks across every epic.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "epic_id": {
                        "type": "string",
                        "description": "If provided, only return tasks belonging to this epic."
                    }
                },
                "required": []
            }),
        },
        Tool {
            name: "gitzi_get_adr",
            description: "Retrieve a single Architecture Decision Record (ADR) by its ID. \
                          Returns the full persisted review item including the original question, \
                          any context that was supplied, and all actions taken on it (answers, \
                          approvals, rejections). Use this to read an ADR before referencing it \
                          in your work.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The unique ID of the ADR / review item to retrieve."
                    }
                },
                "required": ["id"]
            }),
        },
        Tool {
            name: "gitzi_prioritize_task",
            description: "Set the priority of a task, controlling the order agents pick it up. \
                          Lower numbers are worked first (1 = highest priority, 100 = default). \
                          Use this when the user wants to reorder work — e.g. \"do the auth \
                          refresh before the login page\". The change takes effect immediately: \
                          the next agent wake-up will pick the highest-priority task.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "The ID of the task to reprioritize."
                    },
                    "priority": {
                        "type": "integer",
                        "description": "New priority value. Lower is worked first. 1 = urgent, 100 = normal, 200 = low."
                    }
                },
                "required": ["task_id", "priority"]
            }),
        },
        Tool {
            name: "gitzi_create_task",
            description: "Create a new task inside the specified epic. The task is placed in the \
                          Prioritized column so the Prioritizer agent can order it. Supply a \
                          clear, concise title and, optionally, a detailed description and an \
                          initial priority (lower numbers are worked first; default 100).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "epic_id": {
                        "type": "string",
                        "description": "The ID of the epic this task belongs to."
                    },
                    "title": {
                        "type": "string",
                        "description": "Short, imperative title for the task (e.g. \"Add rate-limiting to the login endpoint\")."
                    },
                    "description": {
                        "type": "string",
                        "description": "Optional detailed description, acceptance criteria, or additional context."
                    },
                    "priority": {
                        "type": "integer",
                        "description": "Optional initial priority. Lower values are worked first. Defaults to 100."
                    }
                },
                "required": ["epic_id", "title"]
            }),
        },
        Tool {
            name: "gitzi_create_adr",
            description: "Create an Architecture Decision Record (ADR) for a task you are \
                          currently working on. Use this to surface a design question or \
                          architectural decision that requires human input before you can \
                          proceed. The task_id must match the task you were assigned (from \
                          your GITZI_MCP_TOKEN). The human reviewer will answer the question \
                          and unblock you.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "The ID of the task this ADR is associated with. Must match the task in your session token."
                    },
                    "question": {
                        "type": "string",
                        "description": "The architectural question or decision that needs human input. Be specific and self-contained."
                    },
                    "context": {
                        "type": "string",
                        "description": "Optional background context, constraints, or alternatives you have already considered."
                    }
                },
                "required": ["task_id", "question"]
            }),
        },
        Tool {
            name: "gitzi_update_task",
            description: "Update the title or description of the task you are currently working \
                          on. The task_id must match the task assigned to your session token. \
                          Use this to refine the task definition as you learn more during \
                          implementation — do not use it to change another agent's task.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "The ID of the task to update. Must match the task in your session token."
                    },
                    "title": {
                        "type": "string",
                        "description": "New title for the task. Omit to leave unchanged."
                    },
                    "description": {
                        "type": "string",
                        "description": "New description for the task. Omit to leave unchanged."
                    }
                },
                "required": ["task_id"]
            }),
        },
        Tool {
            name: "gitzi_park_task",
            description: "Park (block) the task you are currently working on, providing a \
                          reason why it cannot progress. The task_id must match the task \
                          assigned to your session token. Use this when the task is blocked \
                          by an external dependency, missing information, or a prerequisite \
                          task that has not yet completed — not for raising architectural \
                          questions (use gitzi_create_adr for that).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "The ID of the task to park. Must match the task in your session token."
                    },
                    "reason": {
                        "type": "string",
                        "description": "A clear explanation of why the task is blocked and what is needed to unblock it."
                    }
                },
                "required": ["task_id", "reason"]
            }),
        },
    ]
}
