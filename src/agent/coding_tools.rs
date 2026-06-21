//! Coding agent tools — filesystem and shell operations sandboxed to a worktree.

// TODO: Expand tool set based on agent needs. When an agent calls an unknown
// tool at runtime, a review item is automatically created asking whether to
// build it. Candidate tools: web_search, git_diff, git_log, run_tests,
// semantic_search, ask_human.

use std::path::{Path, PathBuf};
use serde_json::json;

use crate::agent::main_agent::{OaiTool, OaiFunctionDef};

/// Build the tool list for the coding agent.
pub fn coding_agent_tools() -> Vec<OaiTool> {
    vec![
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "bash",
                description: "Run a shell command in the task worktree. Returns stdout and stderr.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "The shell command to execute." }
                    },
                    "required": ["command"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "read_file",
                description: "Read the contents of a file (path relative to worktree root).",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Relative file path." }
                    },
                    "required": ["path"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "write_file",
                description: "Write or overwrite a file (path relative to worktree root).",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Relative file path." },
                        "content": { "type": "string", "description": "Full file content to write." }
                    },
                    "required": ["path", "content"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "edit_file",
                description: "Replace a specific string in a file. The old_str must match exactly once.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Relative file path." },
                        "old_str": { "type": "string", "description": "The exact string to find and replace." },
                        "new_str": { "type": "string", "description": "The replacement string." }
                    },
                    "required": ["path", "old_str", "new_str"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "list_dir",
                description: "List files and directories at a path (relative to worktree root).",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Relative directory path. Use '.' for root." }
                    },
                    "required": ["path"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "grep",
                description: "Search for a regex pattern across files in the worktree.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Regex pattern to search for." },
                        "path": { "type": "string", "description": "Relative path to search in (file or directory). Defaults to '.'." }
                    },
                    "required": ["pattern"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_create_review_item",
                description: "Raise a clarification item when something is unclear. The task will be parked until the human answers.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "The task ID this question is about." },
                        "question": { "type": "string", "description": "The question for the human." }
                    },
                    "required": ["task_id", "question"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_park_task",
                description: "Park the current task -- it cannot progress until a dependency is resolved.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "The task to park." },
                        "reason": { "type": "string", "description": "Why the task is blocked." }
                    },
                    "required": ["task_id", "reason"]
                }),
            },
        },
        OaiTool {
            r#type: "function",
            function: OaiFunctionDef {
                name: "gitzi_create_task",
                description: "Create a dependency task under the same epic.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "epic_id": { "type": "string", "description": "The epic to create the task under." },
                        "title": { "type": "string", "description": "Task title." },
                        "description": { "type": "string", "description": "Task description." }
                    },
                    "required": ["epic_id", "title"]
                }),
            },
        },
    ]
}

/// Execute a coding tool call. Returns the result.
/// `worktree_root` is the sandbox — all paths are resolved relative to it.
pub fn execute_tool(
    name: &str,
    args: &serde_json::Value,
    worktree_root: &Path,
) -> ToolResult {
    match name {
        "bash" => exec_bash(args, worktree_root),
        "read_file" => exec_read_file(args, worktree_root),
        "write_file" => exec_write_file(args, worktree_root),
        "edit_file" => exec_edit_file(args, worktree_root),
        "list_dir" => exec_list_dir(args, worktree_root),
        "grep" => exec_grep(args, worktree_root),
        // gitzi tools are handled by the dispatcher, not here
        "gitzi_create_review_item" | "gitzi_park_task" | "gitzi_create_task" => {
            ToolResult::DelegateToDispatcher
        }
        other => ToolResult::UnknownTool(other.to_string()),
    }
}

/// Result of executing a coding tool.
pub enum ToolResult {
    /// Tool executed successfully, here's the output.
    Output(String),
    /// This tool should be handled by the dispatcher (gitzi_* tools).
    DelegateToDispatcher,
    /// Tool name not recognized — agent tried to call something that doesn't exist.
    UnknownTool(String),
}

fn resolve_path(relative: &str, root: &Path) -> Option<PathBuf> {
    let path = root.join(relative);
    // Prevent path traversal above the worktree root
    match path.canonicalize() {
        Ok(canonical) => {
            if let Ok(root_canonical) = root.canonicalize() {
                if canonical.starts_with(&root_canonical) {
                    return Some(canonical);
                }
            }
            None
        }
        Err(_) => {
            // File might not exist yet (for write_file) — check parent
            if let Some(parent) = path.parent() {
                if let Ok(parent_canonical) = parent.canonicalize() {
                    if let Ok(root_canonical) = root.canonicalize() {
                        if parent_canonical.starts_with(&root_canonical) {
                            return Some(path);
                        }
                    }
                }
            }
            None
        }
    }
}

fn exec_bash(args: &serde_json::Value, root: &Path) -> ToolResult {
    let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
    if command.is_empty() {
        return ToolResult::Output("error: missing command".to_string());
    }

    let output = std::process::Command::new("bash")
        .args(["-c", command])
        .current_dir(root)
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut result = String::new();
            if !stdout.is_empty() {
                result.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !result.is_empty() {
                    result.push('\n');
                }
                result.push_str("[stderr] ");
                result.push_str(&stderr);
            }
            if result.is_empty() {
                result = format!("(exit code: {})", out.status.code().unwrap_or(-1));
            }
            // Truncate to 10KB to prevent context explosion
            if result.len() > 10_000 {
                result.truncate(10_000);
                result.push_str("\n[truncated]");
            }
            ToolResult::Output(result)
        }
        Err(e) => ToolResult::Output(format!("error: {e}")),
    }
}

fn exec_read_file(args: &serde_json::Value, root: &Path) -> ToolResult {
    let rel_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    if rel_path.is_empty() {
        return ToolResult::Output("error: missing path".to_string());
    }
    let path = match resolve_path(rel_path, root) {
        Some(p) => p,
        None => return ToolResult::Output("error: path outside worktree".to_string()),
    };
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            if content.len() > 50_000 {
                ToolResult::Output(format!("{}[truncated at 50KB]", &content[..50_000]))
            } else {
                ToolResult::Output(content)
            }
        }
        Err(e) => ToolResult::Output(format!("error: {e}")),
    }
}

fn exec_write_file(args: &serde_json::Value, root: &Path) -> ToolResult {
    let rel_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
    if rel_path.is_empty() {
        return ToolResult::Output("error: missing path".to_string());
    }
    let path = match resolve_path(rel_path, root) {
        Some(p) => p,
        None => return ToolResult::Output("error: path outside worktree".to_string()),
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, content) {
        Ok(()) => ToolResult::Output(format!("ok: wrote {} bytes to {rel_path}", content.len())),
        Err(e) => ToolResult::Output(format!("error: {e}")),
    }
}

fn exec_edit_file(args: &serde_json::Value, root: &Path) -> ToolResult {
    let rel_path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let old_str = args.get("old_str").and_then(|v| v.as_str()).unwrap_or("");
    let new_str = args.get("new_str").and_then(|v| v.as_str()).unwrap_or("");
    if rel_path.is_empty() || old_str.is_empty() {
        return ToolResult::Output("error: missing path or old_str".to_string());
    }
    let path = match resolve_path(rel_path, root) {
        Some(p) => p,
        None => return ToolResult::Output("error: path outside worktree".to_string()),
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return ToolResult::Output(format!("error reading file: {e}")),
    };
    let count = content.matches(old_str).count();
    if count == 0 {
        return ToolResult::Output("error: old_str not found in file".to_string());
    }
    if count > 1 {
        return ToolResult::Output(
            format!("error: old_str found {count} times -- must be unique"),
        );
    }
    let new_content = content.replacen(old_str, new_str, 1);
    match std::fs::write(&path, &new_content) {
        Ok(()) => ToolResult::Output("ok: edit applied".to_string()),
        Err(e) => ToolResult::Output(format!("error writing: {e}")),
    }
}

fn exec_list_dir(args: &serde_json::Value, root: &Path) -> ToolResult {
    let rel_path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    let path = root.join(rel_path);
    match std::fs::read_dir(&path) {
        Ok(entries) => {
            let mut lines: Vec<String> = entries
                .flatten()
                .map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if e.path().is_dir() {
                        format!("{name}/")
                    } else {
                        name
                    }
                })
                .collect();
            lines.sort();
            ToolResult::Output(lines.join("\n"))
        }
        Err(e) => ToolResult::Output(format!("error: {e}")),
    }
}

fn exec_grep(args: &serde_json::Value, root: &Path) -> ToolResult {
    let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
    let rel_path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
    if pattern.is_empty() {
        return ToolResult::Output("error: missing pattern".to_string());
    }

    // Use ripgrep if available, fall back to grep
    let output = std::process::Command::new("rg")
        .args(["--no-heading", "-n", pattern, rel_path])
        .current_dir(root)
        .output()
        .or_else(|_| {
            std::process::Command::new("grep")
                .args(["-rn", pattern, rel_path])
                .current_dir(root)
                .output()
        });

    match output {
        Ok(out) => {
            let result = String::from_utf8_lossy(&out.stdout).to_string();
            if result.len() > 10_000 {
                ToolResult::Output(format!("{}[truncated]", &result[..10_000]))
            } else if result.is_empty() {
                ToolResult::Output("no matches".to_string())
            } else {
                ToolResult::Output(result)
            }
        }
        Err(e) => ToolResult::Output(format!("error: {e}")),
    }
}
