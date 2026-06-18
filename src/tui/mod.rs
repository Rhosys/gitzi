mod app;
mod daemon_client;
mod ui;

use std::io::{self, stdout};
use std::time::Duration;
use ratatui::crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    cursor::Show,
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::mpsc;

use crate::error::Result;
use app::{App, DaemonCommand, DaemonMessage, Mode};

/// Run the TUI. Must be called from within a tokio runtime.
pub async fn run_async() -> Result<()> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let result = run_event_loop(&mut terminal).await;

    // Always restore terminal
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen, Show);
    let _ = terminal.show_cursor();

    result
}

/// Legacy synchronous entry point — spawns a tokio runtime internally.
pub fn run(_repo_root: std::path::PathBuf) -> Result<()> {
    // If we're already in a tokio runtime, use block_in_place
    let rt = tokio::runtime::Handle::try_current();
    match rt {
        Ok(handle) => {
            // Already in a runtime — use block_on from a new thread
            std::thread::scope(|s| {
                s.spawn(|| {
                    handle.block_on(run_async())
                }).join().unwrap()
            })
        }
        Err(_) => {
            // No runtime — create one
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(run_async())
        }
    }
}

async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<DaemonCommand>();
    let mut msg_rx = daemon_client::spawn(cmd_rx);
    let mut app = App::new(cmd_tx);

    loop {
        terminal.draw(|frame| ui::draw(frame, &app))?;

        // Check for daemon messages (non-blocking)
        while let Ok(msg) = msg_rx.try_recv() {
            match msg {
                DaemonMessage::BoardSnapshot(columns) => {
                    app.apply_board_snapshot(columns);
                }
                DaemonMessage::ReviewItem(item) => {
                    app.apply_review_item(item);
                }
                DaemonMessage::Epics(epics) => {
                    app.apply_epics(epics);
                }
                DaemonMessage::QueueLen(count) => {
                    app.apply_queue_len(count);
                }
                DaemonMessage::Event(json) => {
                    app.handle_event(&json);
                }
                DaemonMessage::SwitchPanel(view) => {
                    app.apply_panel_switch(&view);
                }
                DaemonMessage::Connected => {
                    app.connected = true;
                    app.status = "connected".to_string();
                }
                DaemonMessage::Disconnected(reason) => {
                    app.connected = false;
                    app.status = format!("disconnected: {reason}");
                }
                DaemonMessage::ChatHistory(entries) => {
                    app.apply_chat_history(entries);
                }
                DaemonMessage::ChatResponse(response) => {
                    app.apply_chat_response(response);
                }
                DaemonMessage::CommandResult(result) => {
                    match result {
                        Ok(resp) => app.status = resp,
                        Err(e) => app.status = e,
                    }
                    // After a command, refresh state
                    let _ = app.cmd_tx.send(DaemonCommand::RefreshBoard);
                    let _ = app.cmd_tx.send(DaemonCommand::RefreshReview);
                }
            }
        }

        // Poll for terminal events
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        let Event::Key(key) = event::read()? else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        // Ctrl+Q always quits
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
            break;
        }

        match app.mode {
            Mode::Status => match key.code {
                KeyCode::Left | KeyCode::Char('h') => { app.mode = Mode::Idle; app.move_left(); }
                KeyCode::Right | KeyCode::Char('l') => { app.mode = Mode::Idle; app.move_right(); }
                KeyCode::Up | KeyCode::Char('k') => { app.mode = Mode::Idle; app.move_up(); }
                KeyCode::Down | KeyCode::Char('j') => { app.mode = Mode::Idle; app.move_down(); }
                KeyCode::Char('c') => app.begin_chat(),
                _ => {}
            },
            Mode::Idle => match key.code {
                KeyCode::Left | KeyCode::Char('h') => app.move_left(),
                KeyCode::Right | KeyCode::Char('l') => app.move_right(),
                KeyCode::Up | KeyCode::Char('k') => app.move_up(),
                KeyCode::Down | KeyCode::Char('j') => app.move_down(),
                KeyCode::Char('c') => app.begin_chat(),
                KeyCode::Char('s') => app.mode = Mode::Status,
                _ => {}
            },
            Mode::Review => match key.code {
                KeyCode::Char('a') => {
                    // 'a' = approve (buffer) or answer (question)
                    if let Some(ref item) = app.review_item {
                        use app::ReviewItemKind;
                        match &item.kind {
                            ReviewItemKind::AgentQuestion { .. } => app.begin_answer(),
                            ReviewItemKind::BufferApproval { .. } => app.approve_current(),
                        }
                    }
                }
                KeyCode::Char('r') => app.begin_reject(),
                KeyCode::Char('c') => app.begin_chat(),
                KeyCode::Left | KeyCode::Char('h') => app.move_left(),
                KeyCode::Right | KeyCode::Char('l') => app.move_right(),
                KeyCode::Up | KeyCode::Char('k') => app.move_up(),
                KeyCode::Down | KeyCode::Char('j') => app.move_down(),
                _ => {}
            },
            Mode::RejectInput => match key.code {
                KeyCode::Enter => app.submit_reject(),
                KeyCode::Esc => app.cancel_input(),
                KeyCode::Backspace => { app.input.pop(); }
                KeyCode::Char(c) => app.input.push(c),
                _ => {}
            },
            Mode::AnswerInput => match key.code {
                KeyCode::Enter => app.submit_answer(),
                KeyCode::Esc => app.cancel_input(),
                KeyCode::Backspace => { app.input.pop(); }
                KeyCode::Char(c) => app.input.push(c),
                _ => {}
            },
            Mode::ChatInput => match key.code {
                KeyCode::Enter => app.submit_chat(),
                KeyCode::Esc => app.cancel_chat(),
                KeyCode::Backspace => { app.chat_input.pop(); }
                KeyCode::Char(c) => app.chat_input.push(c),
                _ => {}
            },
            Mode::ChatWaiting => {} // no input while waiting for response
        }
    }

    Ok(())
}
