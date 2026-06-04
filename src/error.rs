use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitziError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML deserialize error: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("TOML serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error("Git error: {0}")]
    Git(#[from] git2::Error),

    #[error("Agent failed: {0}")]
    AgentFailed(String),

    #[error("WIP limit exceeded for stage {stage} (limit {limit})")]
    WipLimitExceeded { stage: String, limit: u32 },

    #[error("Invalid stage transition from {from} to {to}")]
    InvalidTransition { from: String, to: String },

    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Epic not found: {0}")]
    EpicNotFound(String),

    #[error("Config error: {0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, GitziError>;
