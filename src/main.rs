use std::path::PathBuf;
use std::sync::Arc;
use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::broadcast;
use tracing::info;
use gitzi::cli::{Cli, Commands, EpicCommands, TaskCommands};
use gitzi::config::Config;
use gitzi::model::{Epic, Stage, Task};
use gitzi::model::task::{new_id, slug};
use gitzi::pipeline::{Orchestrator, Scheduler};
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

    match cli.command {
        Commands::Init => cmd_init(&repo_root)?,
        Commands::Run { port } => cmd_run(&repo_root, port).await?,
        Commands::Status => cmd_status(&repo_root)?,
        Commands::Advance { task_id, stage, note } => cmd_advance(&repo_root, &task_id, &stage, note)?,
        Commands::Task { command: TaskCommands::Create { epic, title, priority, description } } => {
            cmd_task_create(&repo_root, &epic, &title, priority, description)?;
        }
        Commands::Epic { command: EpicCommands::Create { title, description } } => {
            cmd_epic_create(&repo_root, &title, description)?;
        }
    }

    Ok(())
}

fn cmd_init(repo_root: &PathBuf) -> Result<()> {
    let gitzi = repo_root.join(".gitzi");
    std::fs::create_dir_all(gitzi.join("plan").join("epics"))?;
    std::fs::create_dir_all(gitzi.join("plan").join("tasks"))?;
    std::fs::create_dir_all(gitzi.join("wip").join("tasks"))?;

    // Committed alongside plan/ so the ignore rule travels with the repo.
    gitzi::config::atomic_write(&gitzi.join(".gitignore"), "wip/\n")?;

    let config = Config::default();
    config.write(repo_root)?;
    writer::write_wip(repo_root, &[])?;
    println!("Initialized .gitzi/ in {}", repo_root.display());
    println!("  .gitzi/plan/   ← commit this (epics, tasks)");
    println!("  .gitzi/wip/    ← gitignored (worktrees, logs, wip snapshot)");
    Ok(())
}

async fn cmd_run(repo_root: &PathBuf, port: u16) -> Result<()> {
    let config = Arc::new(Config::load(repo_root).context("Failed to load config")?);
    let (tx, _rx) = broadcast::channel::<gitzi::state::watcher::StateEvent>(64);

    let orchestrator = Arc::new(Orchestrator::new(
        repo_root.clone(),
        config.clone(),
        tx.clone(),
    ));

    let scheduler = Scheduler::new(orchestrator.clone(), config.clone(), repo_root.clone());

    #[cfg(feature = "dashboard")]
    {
        let env = gitzi::dashboard::build_env();
        let state = Arc::new(gitzi::dashboard::AppState {
            repo_root: repo_root.clone(),
            orchestrator: orchestrator.clone(),
            tx: tx.clone(),
            env,
        });
        let router = gitzi::dashboard::build_router(state);
        let addr = format!("0.0.0.0:{port}");
        info!("Dashboard listening on http://{addr}");
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        tokio::select! {
            r = axum::serve(listener, router) => { r?; }
            r = scheduler.run() => { r?; }
        }
    }

    #[cfg(not(feature = "dashboard"))]
    {
        let _ = port;
        info!("Running scheduler (no dashboard — build with --features dashboard)");
        scheduler.run().await?;
    }

    Ok(())
}

fn cmd_status(repo_root: &PathBuf) -> Result<()> {
    let wip = gitzi::state::reader::load_wip(repo_root)?;
    for (stage, ids) in &wip.stages {
        println!("{stage}: {}", ids.join(", "));
    }
    Ok(())
}

fn cmd_advance(repo_root: &PathBuf, task_id: &str, stage_str: &str, note: Option<String>) -> Result<()> {
    let stage = parse_stage(stage_str)?;
    let config = Arc::new(Config::load(repo_root)?);
    let (tx, _) = broadcast::channel(8);
    let orch = Orchestrator::new(repo_root.clone(), config, tx);
    orch.advance_task(task_id, stage, note)?;
    println!("Task {task_id} advanced to {stage_str}");
    Ok(())
}

fn cmd_task_create(
    repo_root: &PathBuf,
    epic_id: &str,
    title: &str,
    priority: u32,
    description: Option<String>,
) -> Result<()> {
    let id = new_id();
    let mut task = Task::new(&id, epic_id, title);
    task.priority = priority;
    task.description = description;
    writer::write_task(repo_root, &task)?;

    if let Ok(mut epic) = reader::load_epic(repo_root, epic_id) {
        epic.tasks.push(id.clone());
        writer::write_epic(repo_root, &epic)?;
    }

    writer::rebuild_wip(repo_root)?;
    println!("Created task {id}: {title}");
    Ok(())
}

fn cmd_epic_create(repo_root: &PathBuf, title: &str, description: Option<String>) -> Result<()> {
    let id = new_id();
    let mut epic = Epic::new(&id, title);
    epic.description = description;
    writer::write_epic(repo_root, &epic)?;
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
        other => anyhow::bail!("Unknown stage: {other}"),
    }
}
