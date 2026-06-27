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
use gitzi::state::{reader, writer, home};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("gitzi=info".parse()?),
        )
        .init();

    let cli = Cli::parse();

    if cli.uninstall {
        info!("Unregistering gitzi daemon service...");
        daemon::uninstall()?;
        info!("Done.");
        return Ok(());
    }

    if cli.daemon {
        return cmd_daemon().await;
    }

    match cli.command {
        None => cmd_default().await?,
        Some(Commands::Status) => cmd_status()?,
        Some(Commands::GenerateConfig) => cmd_generate_config().await?,
        Some(Commands::Log) => {
            #[cfg(feature = "tui")]
            cmd_log().await?;
            #[cfg(not(feature = "tui"))]
            anyhow::bail!("log viewer not available — build with --features tui");
        }
        Some(Commands::Advance { task_id, stage, note }) => {
            cmd_advance(&task_id, &stage, note)?;
        }
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
async fn cmd_default() -> Result<()> {
    // Ensure state directories exist
    home::ensure_dirs()?;

    // Ensure daemon is running (registers systemd service on first run)
    daemon::ensure_running().await?;
    info!("Daemon is running");

    // Launch TUI
    #[cfg(feature = "tui")]
    {
        gitzi::tui::run_async().await?;
    }
    #[cfg(not(feature = "tui"))]
    {
        anyhow::bail!("TUI not available — build with --features tui");
    }

    Ok(())
}

/// Fullscreen scrollable journal viewer with live follow.
#[cfg(feature = "tui")]
async fn cmd_log() -> Result<()> {
    use std::process::Stdio;
    use ratatui::crossterm::{
        event::{self as ct_event, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
        cursor::Show,
    };
    use ratatui::{
        backend::CrosstermBackend,
        layout::{Constraint, Layout},
        style::{Color, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Paragraph},
        Terminal,
    };
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command as TokioCommand;

    let mut child = TokioCommand::new("journalctl")
        .args(["--user", "-u", "gitzi.service", "--no-pager", "-n", "200", "-f", "-o", "short"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn journalctl")?;

    let child_stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(child_stdout).lines();

    enable_raw_mode()?;
    let mut out = std::io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let mut log_lines: Vec<String> = Vec::new();
    let mut scroll_offset: usize = 0;
    let mut auto_follow = true;

    loop {
        // Read available lines non-blocking
        loop {
            tokio::select! {
                biased;
                result = reader.next_line() => {
                    match result {
                        Ok(Some(line)) => {
                            log_lines.push(line);
                            if log_lines.len() > 10000 {
                                log_lines.drain(..log_lines.len() - 10000);
                            }
                            if auto_follow {
                                scroll_offset = log_lines.len().saturating_sub(1);
                            }
                        }
                        _ => break,
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => break,
            }
        }

        terminal.draw(|frame| {
            let area = frame.area();
            let [header, body] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Fill(1),
            ]).areas(area);

            let follow_indicator = if auto_follow { " [following]" } else { "" };
            frame.render_widget(
                Paragraph::new(Span::styled(
                    format!(
                        " gitzi log{follow_indicator}  [q] quit  [up/dn] scroll  [f] follow"
                    ),
                    Style::default().fg(Color::DarkGray),
                )),
                header,
            );

            let height = body.height.saturating_sub(2) as usize;
            let start = scroll_offset.saturating_sub(height.saturating_sub(1));
            let visible: Vec<Line> = log_lines
                .iter()
                .skip(start)
                .take(height)
                .map(|l| {
                    Line::from(Span::styled(l.as_str(), Style::default().fg(Color::Gray)))
                })
                .collect();

            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray));
            let inner = block.inner(body);
            frame.render_widget(block, body);
            frame.render_widget(Paragraph::new(visible), inner);
        })?;

        if ct_event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = ct_event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('f') => {
                        auto_follow = !auto_follow;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        auto_follow = false;
                        scroll_offset = scroll_offset.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        auto_follow = false;
                        if scroll_offset < log_lines.len().saturating_sub(1) {
                            scroll_offset += 1;
                        }
                    }
                    KeyCode::PageUp => {
                        auto_follow = false;
                        scroll_offset = scroll_offset.saturating_sub(20);
                    }
                    KeyCode::PageDown => {
                        if scroll_offset + 20 >= log_lines.len() {
                            auto_follow = true;
                        }
                        scroll_offset =
                            (scroll_offset + 20).min(log_lines.len().saturating_sub(1));
                    }
                    _ => {}
                }
            }
        }
    }

    let _ = child.kill().await;
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen, Show);
    Ok(())
}

/// Run the daemon process: dispatcher event loop + unix socket server.
/// Invoked by systemd, not directly by the user.
async fn cmd_daemon() -> Result<()> {
    info!("gitzi daemon starting");

    // Ensure all state directories exist before loading anything
    home::ensure_dirs()?;

    let gitzi_home = home::gitzi_home();
    let config = Config::load(&gitzi_home).context("Failed to load config")?;

    // Populate repo cache from config globs
    if !config.repo_paths.is_empty() {
        let repos = gitzi::state::repo_cache::populate(&config.repo_paths);
        info!("discovered {} repos", repos.len());
    }

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
            let _ = std::fs::remove_file(home::mcp_socket_path());
            Ok(())
        }
    }
}

fn cmd_status() -> Result<()> {
    let wip = reader::load_wip()?;
    for (stage, ids) in &wip.stages {
        println!("{stage}: {}", ids.join(", "));
    }
    Ok(())
}

async fn cmd_generate_config() -> Result<()> {
    let config = tokio::task::spawn_blocking(gitzi::bootstrap::run)
        .await
        .map_err(|e| anyhow::anyhow!("bootstrap task panicked: {e}"))??;
    let path = home::global_config_file();
    println!("Generated config at {}", path.display());
    println!("  providers: {}", config.providers.keys().cloned().collect::<Vec<_>>().join(", "));
    println!("  repo_paths: {}", config.repo_paths.len());
    println!("  agents: {}", config.agents.len());
    Ok(())
}

fn cmd_advance(task_id: &str, stage_str: &str, note: Option<String>) -> Result<()> {
    let stage = parse_stage(stage_str)?;
    let gitzi_home = home::gitzi_home();
    let config = Arc::new(Config::load(&gitzi_home)?);
    let (tx, _) = broadcast::channel(8);
    let orch = Orchestrator::new(gitzi_home, config, tx);
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
        "done" => Ok(Stage::Done),
        "designing" => Ok(Stage::Designing),
        "coding-buffer" => Ok(Stage::CodingBuffer),
        "coding" => Ok(Stage::Coding),
        "review-buffer" => Ok(Stage::ReviewBuffer),
        "reviewing" => Ok(Stage::Reviewing),
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
