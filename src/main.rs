use std::path::PathBuf;
use std::sync::Arc;
use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::broadcast;
use tracing::{error, info};
use gitzi::cli::{Cli, Commands, EpicCommands, TaskCommands};
use gitzi::config::Config;
use gitzi::daemon;
use gitzi::id::new_id;
use gitzi::model::{Epic, Stage, Task};
use gitzi::pipeline::Orchestrator;
use gitzi::state::{reader, writer};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("gitzi=info".parse()?),
        )
        .init();

    let cli = Cli::parse();
    let repo_root = PathBuf::from(".");

    if cli.uninstall {
        info!("Unregistering gitzi daemon service...");
        daemon::uninstall()?;
        info!("Done.");
        return Ok(());
    }

    if cli.daemon {
        return cmd_daemon(&repo_root).await;
    }

    match cli.command {
        None => cmd_default(repo_root).await?,
        Some(Commands::Init) => cmd_init(&repo_root)?,
        Some(Commands::Status) => cmd_status()?,
        Some(Commands::Advance { task_id, stage, note }) => cmd_advance(&repo_root, &task_id, &stage, note)?,
        Some(Commands::Task { command: TaskCommands::Create { epic, title, priority, description } }) => {
            cmd_task_create(&epic, &title, priority, description)?;
        }
        Some(Commands::Epic { command: EpicCommands::Create { title, description } }) => {
            cmd_epic_create(&title, description)?;
        }
    }

    Ok(())
}

/// Default command: ensure daemon is running, then launch TUI.
async fn cmd_default(repo_root: PathBuf) -> Result<()> {
    // Ensure daemon is running (registers systemd service on first run)
    daemon::ensure_running().await?;
    info!("Daemon is running");

    // Launch TUI
    #[cfg(feature = "tui")]
    {
        let _ = repo_root;
        gitzi::tui::run_async().await?;
    }
    #[cfg(not(feature = "tui"))]
    {
        let _ = repo_root;
        anyhow::bail!("TUI not available — build with --features tui");
    }

    Ok(())
}

/// Run the daemon process: dispatcher event loop + unix socket server.
/// Invoked by systemd, not directly by the user.
async fn cmd_daemon(repo_root: &std::path::Path) -> Result<()> {
    info!("gitzi daemon starting");

    let config = Config::load(repo_root).context("Failed to load config")?;

    let dispatcher = Arc::new(
        gitzi::dispatcher::Dispatcher::start(config)
            .await
            .context("Failed to start dispatcher")?
    );

    let token_store = Arc::clone(&dispatcher.token_store);

    tokio::select! {
        result = daemon::serve(Arc::clone(&dispatcher)) => {
            error!("Daemon socket server exited: {:?}", result);
            result
        }
        result = gitzi::mcp::serve(Arc::clone(&dispatcher), token_store) => {
            error!("MCP server exited: {:?}", result);
            result
        }
        result = dispatcher.run() => {
            error!("Dispatcher event loop exited: {:?}", result);
            result
        }
        result = dispatcher.watch_config() => {
            error!("Config watcher exited: {:?}", result);
            result
        }
        _ = shutdown_signal() => {
            info!("Shutdown signal received — exiting");
            let _ = std::fs::remove_file(daemon::socket_path());
            let _ = std::fs::remove_file(gitzi::state::home::mcp_socket_path());
            Ok(())
        }
    }
}

fn cmd_init(repo_root: &std::path::Path) -> Result<()> {
    use gitzi::state::home;

    let gitzi_home = home::gitzi_home();
    let session_id = home::init_session()?;
    let session_dir = home::session_dir()?;

    // Ensure ~/.gitzi/ exists and is a git repo
    std::fs::create_dir_all(&gitzi_home)?;
    if !gitzi_home.join(".git").exists() {
        git2::Repository::init(&gitzi_home)
            .map_err(|e| anyhow::anyhow!("Failed to init ~/.gitzi repo: {e}"))?;
    }

    // Create session state directories
    std::fs::create_dir_all(session_dir.join("plan").join("epics"))?;
    std::fs::create_dir_all(session_dir.join("plan").join("tasks"))?;
    std::fs::create_dir_all(session_dir.join("wip").join("tasks"))?;

    // Persist the repo root so the daemon can locate git operations
    home::write_repo_path(repo_root)?;

    // Write global config if not already present
    let config_path = home::global_config_file();
    if !config_path.exists() {
        Config::default().write(repo_root)?;
    }

    writer::write_wip(&[])?;

    println!("Initialized gitzi");
    println!("  home:    {}", gitzi_home.display());
    println!("  session: {session_id}");
    Ok(())
}

fn cmd_status() -> Result<()> {
    let wip = reader::load_wip()?;
    for (stage, ids) in &wip.stages {
        println!("{stage}: {}", ids.join(", "));
    }
    Ok(())
}

fn cmd_advance(repo_root: &std::path::Path, task_id: &str, stage_str: &str, note: Option<String>) -> Result<()> {
    let stage = parse_stage(stage_str)?;
    let config = Arc::new(Config::load(repo_root)?);
    let (tx, _) = broadcast::channel(8);
    let orch = Orchestrator::new(repo_root.to_path_buf(), config, tx);
    orch.advance_task(task_id, stage, note)?;
    println!("Task {task_id} advanced to {stage_str}");
    Ok(())
}

fn cmd_task_create(
    epic_id: &str,
    title: &str,
    priority: u32,
    description: Option<String>,
) -> Result<()> {
    let id = new_id(title);
    let mut task = Task::new(&id, epic_id, title);
    task.priority = priority;
    task.description = description;
    writer::write_task(&task)?;

    if let Ok(mut epic) = reader::load_epic(epic_id) {
        epic.tasks.push(id.clone());
        writer::write_epic(&epic)?;
    }

    writer::rebuild_wip()?;
    println!("Created task {id}: {title}");
    Ok(())
}

fn cmd_epic_create(title: &str, description: Option<String>) -> Result<()> {
    let id = new_id(title);
    let mut epic = Epic::new(&id, title);
    epic.description = description;
    writer::write_epic(&epic)?;
    println!("Created epic {id}: {title}");
    Ok(())
}

fn parse_stage(s: &str) -> Result<Stage> {
    match s {
        "backlog" => Ok(Stage::Backlog),
        "prioritized" => Ok(Stage::Prioritized),
        "in-progress" => Ok(Stage::InProgress),
        "waiting-for-review" => Ok(Stage::WaitingForReview),
        "in-testing" => Ok(Stage::InTesting),
        "done" => Ok(Stage::Done),
        "designing" => Ok(Stage::Designing),
        "coding-buffer" => Ok(Stage::CodingBuffer),
        "coding" => Ok(Stage::Coding),
        "review-buffer" => Ok(Stage::ReviewBuffer),
        "reviewing" => Ok(Stage::Reviewing),
        "test-buffer" => Ok(Stage::TestBuffer),
        "testing" => Ok(Stage::Testing),
        "security-audit-buffer" => Ok(Stage::SecurityAuditBuffer),
        "auditing" => Ok(Stage::Auditing),
        "deployment-buffer" => Ok(Stage::DeploymentBuffer),
        "deploying" => Ok(Stage::Deploying),
        other => anyhow::bail!("Unknown stage: {other}"),
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigint = signal(SignalKind::interrupt()).expect("failed to bind SIGINT");
        let mut sigterm = signal(SignalKind::terminate()).expect("failed to bind SIGTERM");
        tokio::select! {
            _ = sigint.recv() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.expect("failed to bind Ctrl-C");
    }
}
