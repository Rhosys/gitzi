use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "gitzi", about = "Kanban agent harness for software development pipelines")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Run as background daemon (used by systemd, not invoked directly)
    #[arg(long, hide = true)]
    pub daemon: bool,

    /// Unregister the background daemon service
    #[arg(long)]
    pub uninstall: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Show current WIP snapshot
    Status,

    /// Manually advance a task to a new stage
    Advance {
        task_id: String,
        stage: String,
        #[arg(long)]
        note: Option<String>,
    },

    /// Create a new task
    Task {
        #[command(subcommand)]
        command: TaskCommands,
    },

    /// Create a new epic
    Epic {
        #[command(subcommand)]
        command: EpicCommands,
    },
}

#[derive(Subcommand)]
pub enum TaskCommands {
    Create {
        #[arg(long)]
        epic: String,
        #[arg(long)]
        title: String,
        #[arg(long, default_value = "100")]
        priority: u32,
        #[arg(long)]
        description: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum EpicCommands {
    Create {
        #[arg(long)]
        title: String,
        #[arg(long)]
        description: Option<String>,
    },
}
