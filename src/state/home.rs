use std::path::PathBuf;
use crate::error::{GitziError, Result};

/// Root of the global gitzi state repo: `~/.gitzi/`
pub fn gitzi_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".gitzi")
}

pub fn global_config_file() -> PathBuf {
    gitzi_home().join("config.toml")
}

/// Path to the file that records the active session ID.
pub fn current_session_file() -> PathBuf {
    gitzi_home().join("current")
}

/// Read the active session ID from `~/.gitzi/current`.
/// Returns an error if `gitzi init` hasn't been run yet.
pub fn session_id() -> Result<String> {
    let path = current_session_file();
    if !path.exists() {
        return Err(GitziError::Config(
            "No active session. Run `gitzi init` first.".into(),
        ));
    }
    Ok(std::fs::read_to_string(&path)?.trim().to_string())
}

/// State directory for the active session: `~/.gitzi/<session-id>/`
pub fn session_dir() -> Result<PathBuf> {
    Ok(gitzi_home().join(session_id()?))
}

/// Create a new session ID, write it to `~/.gitzi/current`, return it.
/// Does NOT overwrite an existing session — use `new_session()` explicitly for that.
pub fn init_session() -> Result<String> {
    let path = current_session_file();
    if path.exists() {
        return session_id();
    }
    new_session()
}

/// Always generate a fresh session ID and make it active.
pub fn new_session() -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let path = current_session_file();
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, &id)?;
    Ok(id)
}

pub fn chat_file() -> Result<PathBuf> {
    Ok(session_dir()?.join("chat.jsonl"))
}

/// Read the repo path from `~/.gitzi/<session>/repo`.
/// Falls back to the current working directory if not set.
pub fn repo_path() -> PathBuf {
    session_dir()
        .ok()
        .map(|d| d.join("repo"))
        .filter(|p| p.exists())
        .and_then(|p| std::fs::read_to_string(&p).ok())
        .map(|s| PathBuf::from(s.trim()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Persist the repo path into the session state: `~/.gitzi/<session>/repo`.
pub fn write_repo_path(repo_root: &std::path::Path) -> Result<()> {
    let dir = session_dir()?;
    std::fs::create_dir_all(&dir)?;
    let canonical = std::fs::canonicalize(repo_root)
        .unwrap_or_else(|_| repo_root.to_path_buf());
    std::fs::write(dir.join("repo"), canonical.to_string_lossy().as_bytes())?;
    Ok(())
}
