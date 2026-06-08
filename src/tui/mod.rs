mod app;
mod ui;

use std::io::{self, stdout};
use std::path::PathBuf;
use std::time::Duration;
use ratatui::crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    cursor::Show,
};
use ratatui::{backend::CrosstermBackend, Terminal};
use crate::error::Result;
use app::App;

pub fn run(repo_root: PathBuf) -> Result<()> {
    enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;

    let result = run_event_loop(&mut terminal, repo_root);

    // Always restore terminal
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen, Show);
    let _ = terminal.show_cursor();

    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    repo_root: PathBuf,
) -> Result<()> {
    let mut app = App::load(repo_root)?;

    loop {
        terminal.draw(|frame| ui::draw(frame, &app))?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Tab => app.toggle_focus(),
                    KeyCode::Up | KeyCode::Char('k') => app.move_up(),
                    KeyCode::Down | KeyCode::Char('j') => app.move_down(),
                    KeyCode::Left | KeyCode::Char('h') => app.move_left(),
                    KeyCode::Right | KeyCode::Char('l') => app.move_right(),
                    KeyCode::Char('r') => {
                        let _ = app.reload();
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}
