use anyhow::{Context, Result};
use clap::Parser;
use gitzi::cli::{Cli, Commands, EpicCommands, TaskCommands};
use gitzi::config::Config;
use gitzi::daemon;
use gitzi::id::new_id;
use gitzi::model::{Epic, Stage, Task};
use gitzi::pipeline::Orchestrator;
use gitzi::state::{home, reader, writer};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{error, info};
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> Result<()> {
    let _error_rx = gitzi::error_relay::init();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("gitzi=info".parse()?),
        ))
        .with(gitzi::error_relay::ErrorRelayLayer)
        .init();

    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            // On any parse error (invalid command, missing args), print full help
            // instead of a terse "use --help" message
            use clap::CommandFactory;
            let mut cmd = Cli::command();
            cmd.print_help().ok();
            println!();
            if !e.to_string().contains("--help") {
                eprintln!("\n{e}");
            }
            std::process::exit(2);
        }
    };

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
        Some(Commands::Advance {
            task_id,
            stage,
            note,
        }) => {
            cmd_advance(&task_id, &stage, note)?;
        }
        Some(Commands::Task {
            command:
                TaskCommands::Create {
                    epic,
                    title,
                    priority,
                    description,
                },
        }) => {
            cmd_task_create(&epic, &title, priority, description)?;
        }
        Some(Commands::Epic {
            command: EpicCommands::Create { title, description },
        }) => {
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
    use ratatui::crossterm::{
        cursor::Show,
        event::{self as ct_event, Event, KeyCode, KeyEventKind},
        execute,
        terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
    };
    use ratatui::{
        Terminal,
        backend::CrosstermBackend,
        layout::{Constraint, Layout},
        style::{Color, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Paragraph},
    };
    use std::process::Stdio;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::Command as TokioCommand;

    let mut child = TokioCommand::new("journalctl")
        .args([
            "--user",
            "-u",
            "gitzi.service",
            "--no-pager",
            "-n",
            "200",
            "-f",
            "-o",
            "short",
        ])
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
            let [header, body] =
                Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);

            let follow_indicator = if auto_follow { " [following]" } else { "" };
            frame.render_widget(
                Paragraph::new(Span::styled(
                    format!(" gitzi log{follow_indicator}  [q] quit  [up/dn] scroll  [f] follow"),
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
                .map(|l| Line::from(Span::styled(l.as_str(), Style::default().fg(Color::Gray))))
                .collect();

            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray));
            let inner = block.inner(body);
            frame.render_widget(block, body);
            frame.render_widget(Paragraph::new(visible), inner);
        })?;

        if ct_event::poll(std::time::Duration::from_millis(50))?
            && let Event::Key(key) = ct_event::read()?
        {
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
                    scroll_offset = (scroll_offset + 20).min(log_lines.len().saturating_sub(1));
                }
                _ => {}
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
    let mut config = Config::load(&gitzi_home).context("Failed to load config")?;

    // ADR-002: the experience is binary — set up an LLM or use one. If no valid
    // control-plane provider exists, enter setup mode on the socket and don't
    // build the dispatcher (so no agents come alive) until the gate clears.
    if !gitzi::setup::gate_ready(&config) {
        config = daemon::run_setup(config)
            .await
            .context("setup phase failed")?;
    }

    // Populate repo cache from config globs
    if !config.repo_paths.is_empty() {
        let repos = gitzi::state::repo_cache::populate(&config.repo_paths);
        info!("discovered {} repos", repos.len());
    }

    // Ensure the configured model is loaded for OpenAI-compatible providers
    // (e.g. LM Studio daemon needs `lms load` before it can serve requests)
    if let Some(provider_name) = config.resolve_agent("main").provider
        && let Some(provider) = config.providers.get(&provider_name)
        && provider.kind == gitzi::config::ProviderKind::OpenaiCompatible
        && !provider.api_url.is_empty()
    {
        let lms = dirs::home_dir()
            .unwrap_or_default()
            .join(".lmstudio/bin/lms");

        // Start the LM Studio server if the port isn't open
        let port_open = std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], 1234)),
            std::time::Duration::from_millis(200),
        )
        .is_ok();

        if !port_open && lms.exists() {
            info!("LM Studio server not running — starting via lms server start");
            let _ = std::process::Command::new(&lms)
                .args(["server", "start"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            // Wait for the server to become available
            for _ in 0..10 {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                if std::net::TcpStream::connect_timeout(
                    &std::net::SocketAddr::from(([127, 0, 0, 1], 1234)),
                    std::time::Duration::from_millis(200),
                )
                .is_ok()
                {
                    break;
                }
            }
        }

        // Now check if a model is loaded; if not, load the default
        let model_loaded = {
            let url = format!("{}/models", provider.api_url.trim_end_matches('/'));
            reqwest::Client::new()
                .get(&url)
                .timeout(std::time::Duration::from_secs(2))
                .send()
                .await
                .ok()
                .and_then(|r| {
                    if r.status().is_success() {
                        Some(r)
                    } else {
                        None
                    }
                })
                .is_some()
        };
        if !model_loaded
            && let Some(ref model) = provider.default_model
            && lms.exists()
        {
            info!(
                "no model loaded at {} — loading {}",
                provider.api_url, model
            );
            let _ = std::process::Command::new(&lms)
                .args(["load", model, "-y"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            // Give it time to load into memory
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    }

    let dispatcher = Arc::new(
        gitzi::dispatcher::Dispatcher::start(config)
            .await
            .context("Failed to start dispatcher")?,
    );

    // Relay errors to the event bus for TUI
    {
        let bus = Arc::clone(&dispatcher.event_bus);
        let mut error_rx = gitzi::error_relay::subscribe().unwrap();
        tokio::spawn(async move {
            while let Ok(msg) = error_rx.recv().await {
                bus.emit(gitzi::dispatcher::event_bus::DispatchEvent::LogEntry {
                    level: "ERROR".to_string(),
                    message: msg,
                });
            }
        });
    }

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
        _ = watch_binary_for_restart() => {
            info!("Binary changed on disk — exec'ing new version");
            let _ = std::fs::remove_file(daemon::socket_path());
            let _ = std::fs::remove_file(home::mcp_socket_path());
            exec_self();
            // exec_self only returns on failure
            Ok(())
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
    println!(
        "  providers: {}",
        config
            .providers
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("  repo_paths: {}", config.repo_paths.len());
    println!("  agents: {}", config.agents.len());

    // Restart daemon so it re-evaluates the gate with fresh config
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "restart", "gitzi.service"])
        .status();

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

/// Watch the running binary for changes on disk. Returns when the file is
/// modified (e.g. after `cargo build`). Debounces for 2 seconds to let the
/// linker finish writing before triggering exec.
async fn watch_binary_for_restart() {
    use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
    use tokio::sync::mpsc;

    let exe_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            error!("cannot resolve own binary path for hot-reload: {e}");
            // Park forever — this select branch never fires
            std::future::pending::<()>().await;
            return;
        }
    };

    let (tx, mut rx) = mpsc::channel::<()>(1);

    let mut watcher = match RecommendedWatcher::new(
        move |res: std::result::Result<Event, notify::Error>| {
            if let Ok(event) = res
                && matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                )
            {
                let _ = tx.try_send(());
            }
        },
        notify::Config::default(),
    ) {
        Ok(w) => w,
        Err(e) => {
            error!("failed to create binary watcher: {e}");
            std::future::pending::<()>().await;
            return;
        }
    };

    // Watch the parent directory (some linkers atomically rename, which the
    // file-level watch misses)
    let watch_dir = exe_path.parent().unwrap_or(&exe_path);
    if let Err(e) = watcher.watch(watch_dir, RecursiveMode::NonRecursive) {
        error!("failed to watch binary directory: {e}");
        std::future::pending::<()>().await;
        return;
    }

    info!("watching binary for changes: {}", exe_path.display());

    // Wait for first modification event
    rx.recv().await;

    // Debounce: wait for writes to settle (linker may still be writing)
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    info!("binary modification detected — preparing to restart");
}

/// Replace the current process with a fresh exec of the same binary.
/// This preserves the PID (systemd doesn't notice) and picks up the new code.
/// Only returns if exec fails.
fn exec_self() {
    use std::os::unix::process::CommandExt;

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            error!("cannot resolve own binary for exec: {e}");
            return;
        }
    };

    let args: Vec<String> = std::env::args().collect();
    let err = std::process::Command::new(&exe).args(&args[1..]).exec();
    // exec() only returns on failure
    error!("exec failed: {err}");
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigint = signal(SignalKind::interrupt()).expect("failed to bind SIGINT");
        let mut sigterm = signal(SignalKind::terminate()).expect("failed to bind SIGTERM");
        tokio::select! {
            _ = sigint.recv() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to bind Ctrl-C");
    }
}
