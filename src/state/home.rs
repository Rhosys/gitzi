use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Root of the global gitzi state repo: `~/.gitzi/`
pub fn gitzi_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".gitzi")
}

/// Stable 16-char hex ID for a project, derived from the canonical repo path.
/// Changing the repo path will produce a different ID; run `gitzi init` again.
pub fn project_id(repo_root: &Path) -> String {
    let canonical = repo_root.canonicalize().unwrap_or_else(|_| repo_root.to_path_buf());
    let mut h = DefaultHasher::new();
    canonical.hash(&mut h);
    format!("{:016x}", h.finish())
}

pub fn project_dir(repo_root: &Path) -> PathBuf {
    gitzi_home().join("projects").join(project_id(repo_root))
}

pub fn chat_file(repo_root: &Path) -> PathBuf {
    project_dir(repo_root).join("chat.jsonl")
}

pub fn global_config_file() -> PathBuf {
    gitzi_home().join("config.toml")
}
