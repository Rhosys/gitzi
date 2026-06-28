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
                DaemonMessage::SetupState(state) => {
                    app.apply_setup_state(state);
                }
                DaemonMessage::SetupMessage(msg) => {
                    app.setup_message = Some(msg);
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

        // Bootstrap setup (ADR-002) owns all input until the gate clears: pick a
        // provider to activate, or rescan. Nothing else is reachable.
        if app.in_setup() {
            match key.code {
                KeyCode::Up => app.setup_move_up(),
                KeyCode::Down => app.setup_move_down(),
                KeyCode::Enter => app.setup_select(),
                KeyCode::Char('r') | KeyCode::Char('R') => app.setup_rescan(),
                _ => {}
            }
            continue;
        }

        // Global: Ctrl+Up/Down switch panels and focus them (works in any state)
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Up => { app.prev_panel(); app.panel_focused = true; continue; }
                KeyCode::Down => { app.next_panel(); app.panel_focused = true; continue; }
                _ => {}
            }
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
        } else if app.panel_focused {
            match key.code {
                KeyCode::Esc => { app.panel_focused = false; }
                KeyCode::Up => app.move_up(),
                KeyCode::Down => app.move_down(),
                KeyCode::Left => app.move_left(),
                KeyCode::Right => app.move_right(),
                KeyCode::Enter => {
                    if app.panel == Panel::Kanban && app.selected_board_task().is_some() {
                        app.panel = Panel::Task;
                    }
                }
                KeyCode::Tab => app.enter_editor(),
                _ => {}
            }
        } else if app.llm_available {
            match key.code {
                KeyCode::Tab => { app.panel_focused = true; }
                KeyCode::Esc => app.close_current_fork(),
                KeyCode::Enter => app.submit_chat(),
                KeyCode::Backspace => { app.chat_input.pop(); }
                KeyCode::Char(c) => app.chat_input.push(c),
                _ => {}
            }
        }
    }

    Ok(())
}
