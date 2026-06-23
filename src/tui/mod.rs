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
use app::{App, DaemonCommand, DaemonMessage, Panel};

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
    let rt = tokio::runtime::Handle::try_current();
    match rt {
        Ok(handle) => {
            std::thread::scope(|s| {
                s.spawn(|| handle.block_on(run_async())).join().unwrap()
            })
        }
        Err(_) => {
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
                DaemonMessage::ReviewPending(_pending) => {
                    // Review state is managed by the main agent via chat;
                    // TUI only needs to know something is pending for status display.
                }
                DaemonMessage::Epics(epics) => {
                    app.apply_epics(epics);
                }
                DaemonMessage::QueueLen(count) => {
                    app.apply_queue_len(count);
                }
                DaemonMessage::Event(json) => {
                    app.push_log(json.clone());
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
                DaemonMessage::ForkCreated { id, name } => {
                    app.apply_fork_created(id, name);
                }
                DaemonMessage::ForkClosed { id } => {
                    app.apply_fork_closed(&id);
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
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('q')
        {
            break;
        }

        if app.editor_focused {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && key.code == KeyCode::Char('s')
            {
                app.save_editor();
            } else {
                match key.code {
                    KeyCode::Esc => app.editor_esc(),
                    KeyCode::Enter => {
                        app.editor_buffer.push('\n');
                        app.editor_dirty = true;
                        app.editor_esc_warned = false;
                    }
                    KeyCode::Backspace => {
                        app.editor_buffer.pop();
                        app.editor_dirty = true;
                        app.editor_esc_warned = false;
                    }
                    KeyCode::Char(c) => {
                        app.editor_buffer.push(c);
                        app.editor_dirty = true;
                        app.editor_esc_warned = false;
                    }
                    _ => {}
                }
            }
        } else {
            match key.code {
                KeyCode::Tab => app.enter_editor(),
                KeyCode::Esc => app.close_current_fork(),
                KeyCode::Enter => app.submit_chat(),
                KeyCode::Backspace => { app.chat_input.pop(); }
                KeyCode::Char(c) => app.chat_input.push(c),
                // Up/Down switch panels (S/E/K/T/L)
                KeyCode::Up => app.prev_panel(),
                KeyCode::Down => app.next_panel(),
                // Left/Right only navigate inside Kanban board
                KeyCode::Left => {
                    if app.panel == Panel::Kanban {
                        app.move_left();
                    }
                }
                KeyCode::Right => {
                    if app.panel == Panel::Kanban {
                        app.move_right();
                    }
                }
                _ => {}
            }
        }
    }

    Ok(())
}
