mod app;
mod daemon_client;
mod ui;

use ratatui::crossterm::{
    cursor::Show,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io::{self, stdout};
use std::time::Duration;
use tokio::sync::mpsc;

use crate::error::Result;
use app::{App, DaemonCommand, DaemonMessage, Panel};

/// Run the TUI. Must be called from within a tokio runtime.
pub async fn run_async() -> Result<()> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;

    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let result = run_event_loop(&mut terminal).await;

    // Always restore terminal
    let _ = disable_raw_mode();
    let _ = execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableBracketedPaste,
        Show
    );
    let _ = terminal.show_cursor();

    result
}

/// Legacy synchronous entry point — spawns a tokio runtime internally.
pub fn run(_repo_root: std::path::PathBuf) -> Result<()> {
    let rt = tokio::runtime::Handle::try_current();
    match rt {
        Ok(handle) => {
            std::thread::scope(|s| s.spawn(|| handle.block_on(run_async())).join().unwrap())
        }
        Err(_) => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(run_async())
        }
    }
}

async fn run_event_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
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
                DaemonMessage::ReviewPending(review) => {
                    app.apply_review(review);
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
                DaemonMessage::StreamingTokens(count) => {
                    app.stream_tokens += count;
                }
                DaemonMessage::StreamingToolCall(name) => {
                    if !app.stream_tools.contains(&name) {
                        app.stream_tools.push(name);
                    }
                }
                DaemonMessage::StreamingDone => {
                    app.stream_tokens = 0;
                    app.stream_tools.clear();
                }
                DaemonMessage::SettingsState { providers, active } => {
                    app.apply_settings_state(providers, active);
                }
                DaemonMessage::SettingsSetupState(state) => {
                    app.settings_setup_state = Some(state);
                }
                DaemonMessage::SettingsMessage(msg) => {
                    app.settings_message = Some(msg);
                    // Clear SSO state on success/error
                    app.settings_sso_provider = None;
                    app.settings_sso_account_id = None;
                    app.settings_sso_choices.clear();
                    app.settings_sso_selected = 0;
                    app.settings_sso_label = None;
                }
                DaemonMessage::SettingsNeedsInput(msg) => {
                    app.settings_apply_needs_input(&msg);
                }
            }
        }

        // Poll for terminal events
        if !event::poll(Duration::from_millis(50))? {
            continue;
        }
        let ev = event::read()?;

        // Bracketed paste: batch the entire pasted string in one redraw
        if let Event::Paste(text) = ev {
            if app.editor_focused {
                app.editor_buffer.push_str(&text);
                app.editor_dirty = true;
                app.editor_esc_warned = false;
            } else if app.llm_available && !app.panel_focused && !app.in_setup() {
                app.chat_input.push_str(&text);
            }
            continue;
        }

        let Event::Key(key) = ev else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        // Ctrl+Q or Ctrl+C always quits
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && (key.code == KeyCode::Char('q') || key.code == KeyCode::Char('c'))
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
                KeyCode::Up => {
                    app.prev_panel();
                    app.panel_focused = true;
                    if app.panel == Panel::Settings {
                        let _ = app.cmd_tx.send(DaemonCommand::SettingsState);
                    }
                    continue;
                }
                KeyCode::Down => {
                    app.next_panel();
                    app.panel_focused = true;
                    if app.panel == Panel::Settings {
                        let _ = app.cmd_tx.send(DaemonCommand::SettingsState);
                    }
                    continue;
                }
                _ => {}
            }
        }

        if app.editor_focused {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
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
                KeyCode::Esc => {
                    if app.panel == Panel::Settings && app.settings_in_sso_picker() {
                        app.settings_sso_cancel();
                    } else {
                        app.panel_focused = false;
                    }
                }
                KeyCode::Up => {
                    if app.panel == Panel::Settings {
                        if app.settings_in_sso_picker() {
                            if app.settings_sso_selected > 0 {
                                app.settings_sso_selected -= 1;
                            }
                        } else {
                            app.settings_move_up();
                        }
                    } else {
                        app.move_up();
                    }
                }
                KeyCode::Down => {
                    if app.panel == Panel::Settings {
                        if app.settings_in_sso_picker() {
                            let n = app.settings_sso_choices.len();
                            if n > 0 && app.settings_sso_selected < n - 1 {
                                app.settings_sso_selected += 1;
                            }
                        } else {
                            app.settings_move_down();
                        }
                    } else {
                        app.move_down();
                    }
                }
                KeyCode::Left => app.move_left(),
                KeyCode::Right => app.move_right(),
                KeyCode::Enter => {
                    if app.panel == Panel::Kanban && app.selected_board_task().is_some() {
                        app.panel = Panel::Task;
                    } else if app.panel == Panel::Settings {
                        if app.settings_in_sso_picker() {
                            app.settings_sso_confirm();
                        } else {
                            app.settings_select_provider();
                        }
                    }
                }
                KeyCode::Char('r') => {
                    if app.panel == Panel::Settings {
                        app.settings_message = Some("Rescanning...".to_string());
                        let _ = app.cmd_tx.send(DaemonCommand::SettingsRescan);
                    }
                }
                KeyCode::Tab => app.enter_editor(),
                _ => {}
            }
        } else if app.llm_available {
            match key.code {
                KeyCode::Tab => {
                    app.panel_focused = true;
                    if app.panel == Panel::Settings {
                        let _ = app.cmd_tx.send(DaemonCommand::SettingsState);
                    }
                }
                KeyCode::Esc => app.close_current_fork(),
                KeyCode::Enter => app.submit_chat(),
                KeyCode::Backspace => {
                    app.chat_input.pop();
                }
                KeyCode::Char(c) => app.chat_input.push(c),
                _ => {}
            }
        }
    }

    Ok(())
}
