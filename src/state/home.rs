use crate::error::Result;
use std::path::PathBuf;

/// Root of the global gitzi state: `~/.gitzi/`
pub fn gitzi_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".gitzi")
}

pub fn global_config_file() -> PathBuf {
    gitzi_home().join("config.toml")
}

/// Board state file: `~/.gitzi/board.toml`
pub fn board_file() -> PathBuf {
    gitzi_home().join("board.toml")
}

/// Plan directory: `~/.gitzi/plan/`
pub fn plan_dir() -> PathBuf {
    gitzi_home().join("plan")
}

/// Reviews directory: `~/.gitzi/plan/reviews/`
pub fn reviews_dir() -> PathBuf {
    plan_dir().join("reviews")
}

/// Chat sessions directory: `~/.gitzi/chats/`
pub fn chats_dir() -> PathBuf {
    gitzi_home().join("chats")
}

/// Tmp directory: `~/.gitzi/tmp/`
pub fn tmp_dir() -> PathBuf {
    gitzi_home().join("tmp")
}

/// Unix socket path for the daemon: `~/.gitzi/tmp/daemon.sock`
pub fn daemon_socket_path() -> PathBuf {
    tmp_dir().join("daemon.sock")
}

/// Unix socket path for the MCP HTTP server: `~/.gitzi/tmp/mcp.sock`
pub fn mcp_socket_path() -> PathBuf {
    tmp_dir().join("mcp.sock")
}

/// Repo cache directory: `~/.gitzi/tmp/cache/repos/`
pub fn repo_cache_dir() -> PathBuf {
    tmp_dir().join("cache").join("repos")
}

/// Per-task tmp directory: `~/.gitzi/tmp/tasks/<id>/`
pub fn task_tmp_dir(task_id: &str) -> PathBuf {
    tmp_dir().join("tasks").join(task_id)
}

/// Agent log for a task: `~/.gitzi/tmp/tasks/<id>/agent.log`
pub fn task_log_path(task_id: &str) -> PathBuf {
    task_tmp_dir(task_id).join("agent.log")
}

/// Worktree path for a task + repo: `~/.gitzi/tmp/tasks/<id>/worktrees/<repo_slug>/`
pub fn task_worktree_path(task_id: &str, repo_slug: &str) -> PathBuf {
    task_tmp_dir(task_id).join("worktrees").join(repo_slug)
}

/// Default repo path. Uses the first discovered repo from config, or falls back to cwd.
pub fn repo_path() -> PathBuf {
    let config_path = global_config_file();
    if config_path.exists()
        && let Ok(text) = std::fs::read_to_string(&config_path)
        && let Ok(config) = toml::from_str::<toml::Value>(&text)
        && let Some(patterns) = config.get("repo_paths").and_then(|v| v.as_array())
    {
        let string_patterns: Vec<String> = patterns
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        let repos = discover_repos(&string_patterns);
        if let Some(first) = repos.first() {
            return first.clone();
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Current chat session file. For now uses a single `current.jsonl` in the chats dir.
/// Future: multiple named sessions.
pub fn current_chat_file() -> PathBuf {
    chats_dir().join("current.jsonl")
}

/// Rolling context summary persisted across daemon restarts.
pub fn summary_file() -> PathBuf {
    chats_dir().join("summary.txt")
}

/// Chat file for a specific fork session.
pub fn fork_chat_file(fork_id: &str) -> PathBuf {
    chats_dir().join(format!("{fork_id}.jsonl"))
}

/// Discover repos from glob patterns. Returns paths to directories containing `.git/`.
pub fn discover_repos(patterns: &[String]) -> Vec<PathBuf> {
    let mut repos = Vec::new();
    for pattern in patterns {
        if let Ok(entries) = glob::glob(pattern) {
            for entry in entries.flatten() {
                if entry.join(".git").is_dir() {
                    repos.push(entry);
                }
            }
        }
    }
    repos.sort();
    repos.dedup();
    repos
}

/// Ensure all required directories exist. Called lazily on daemon startup.
pub fn ensure_dirs() -> Result<()> {
    std::fs::create_dir_all(plan_dir().join("epics"))?;
    std::fs::create_dir_all(plan_dir().join("tasks"))?;
    std::fs::create_dir_all(reviews_dir())?;
    std::fs::create_dir_all(chats_dir())?;
    std::fs::create_dir_all(tmp_dir())?;
    std::fs::create_dir_all(repo_cache_dir())?;
    Ok(())
}
