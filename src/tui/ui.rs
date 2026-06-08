use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use crate::model::Stage;
use super::app::{App, Screen, STAGES};

pub fn draw(frame: &mut Frame, app: &App) {
    match app.screen {
        Screen::Epics => draw_epics_screen(frame, app),
        Screen::Kanban => draw_kanban_screen(frame, app),
    }
}

// ── Epics screen ──────────────────────────────────────────────────────────────

fn draw_epics_screen(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let [header_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ]).areas(area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" gitzi", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled("  —  Epics", Style::default().fg(Color::DarkGray)),
        ])),
        header_area,
    );

    draw_epics_list(frame, app, body_area);

    frame.render_widget(
        Paragraph::new(Span::styled(
            " ↑↓/jk: navigate  Enter: open kanban  r: reload  q: quit",
            Style::default().fg(Color::DarkGray),
        )),
        footer_area,
    );
}

fn draw_epics_list(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Epics ")
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.epics.is_empty() {
        frame.render_widget(
            Paragraph::new("  No epics. Run: gitzi epic create --title \"My Epic\""),
            inner,
        );
        return;
    }

    let items: Vec<ListItem> = app.epics.iter().enumerate().map(|(i, epic)| {
        let (done, total) = app.epic_task_counts(&epic.id);
        let selected = i == app.epic_idx;
        let prefix = if selected { "▸ " } else { "  " };
        let counts = format!("{done}/{total}");
        let avail = inner.width as usize;
        let title_max = avail.saturating_sub(prefix.len() + counts.len() + 1);
        let title = truncate(&epic.title, title_max);
        let pad = avail.saturating_sub(prefix.len() + title.chars().count() + counts.len());
        let line = Line::from(Span::styled(
            format!("{prefix}{title}{:pad$}{counts}", "", pad = pad),
            if selected {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ));
        ListItem::new(line)
    }).collect();

    frame.render_widget(List::new(items), inner);
}

// ── Kanban screen ─────────────────────────────────────────────────────────────

fn draw_kanban_screen(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let [header_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ]).areas(area);

    let scope = app.kanban_epic_title().unwrap_or("All tasks");
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" gitzi", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled("  —  ", Style::default().fg(Color::DarkGray)),
            Span::styled(scope, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ])),
        header_area,
    );

    draw_kanban_columns(frame, app, body_area);

    frame.render_widget(
        Paragraph::new(Span::styled(
            " ↑↓/jk: navigate  ←→/hl: column  r: reload  Esc/Backspace: back  q: quit",
            Style::default().fg(Color::DarkGray),
        )),
        footer_area,
    );
}

fn draw_kanban_columns(frame: &mut Frame, app: &App, area: Rect) {
    let col_constraints: Vec<Constraint> = (0..STAGES.len())
        .map(|_| Constraint::Ratio(1, STAGES.len() as u32))
        .collect();
    let col_areas = Layout::horizontal(col_constraints).split(area);

    for (i, stage) in STAGES.iter().enumerate() {
        let tasks = app.tasks_in_stage(stage);
        let active = i == app.col_idx;
        let border_style = if active {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let col_block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ({}) ", stage_label(stage), tasks.len()))
            .border_style(border_style);

        let col_inner = col_block.inner(col_areas[i]);
        frame.render_widget(col_block, col_areas[i]);

        if tasks.is_empty() { continue; }

        let items: Vec<ListItem> = tasks.iter().enumerate().map(|(j, task)| {
            let selected = active && j == app.task_idx;
            let prefix = if selected { "▸ " } else { "  " };
            let title = truncate(&task.title, col_inner.width as usize);
            ListItem::new(Line::from(Span::styled(
                format!("{prefix}{title}"),
                if selected {
                    Style::default().add_modifier(Modifier::BOLD).fg(Color::Green)
                } else {
                    Style::default()
                },
            )))
        }).collect();

        frame.render_widget(List::new(items), col_inner);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn stage_label(stage: &Stage) -> &'static str {
    match stage {
        Stage::Backlog => "BACKLOG",
        Stage::Prioritized => "PRIORITIZED",
        Stage::InProgress => "IN PROGRESS",
        Stage::WaitingForReview => "REVIEW",
        Stage::InTesting => "TESTING",
        Stage::Done => "DONE",
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if max_chars == 0 { return String::new(); }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else if max_chars <= 1 {
        "…".to_string()
    } else {
        let t: String = chars[..max_chars - 1].iter().collect();
        format!("{t}…")
    }
}
