use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use crate::dispatcher::Column;
use super::app::{App, Mode, ReviewItemKind, column_order, column_abbrev};

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let [header_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ]).areas(area);

    draw_header(frame, app, header_area);
    draw_body(frame, app, body_area);
    draw_footer(frame, app, footer_area);
}

// ── Header ────────────────────────────────────────────────────────────────────

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let connected_indicator = if app.connected { "●" } else { "○" };
    let conn_color = if app.connected { Color::Green } else { Color::Red };

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" gitzi", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled("  ", Style::default()),
            Span::styled(connected_indicator, Style::default().fg(conn_color)),
            Span::styled("  —  ", Style::default().fg(Color::DarkGray)),
            Span::styled(mode_label(&app.mode), Style::default().fg(Color::White)),
        ])),
        area,
    );
}

// ── Body ──────────────────────────────────────────────────────────────────────

fn draw_body(frame: &mut Frame, app: &App, area: Rect) {
    match &app.mode {
        Mode::Idle => draw_board(frame, app, area),
        Mode::Review => draw_review_with_board(frame, app, area),
        Mode::RejectInput | Mode::AnswerInput => draw_input_panel(frame, app, area),
    }
}

// ── Board rendering (13 columns, compact) ─────────────────────────────────────

fn draw_board(frame: &mut Frame, app: &App, area: Rect) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(" Board ")
        .border_style(Style::default().fg(Color::Green));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let col_constraints: Vec<Constraint> = (0..column_order().len())
        .map(|_| Constraint::Ratio(1, column_order().len() as u32))
        .collect();
    let col_areas = Layout::horizontal(col_constraints).split(inner);

    for (i, col) in column_order().iter().enumerate() {
        let tasks = app.tasks_in_column(i);
        let active = i == app.board_col;
        let border_color = if active { Color::Green } else { Color::DarkGray };
        let title_color = column_title_color(col);

        let col_block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", column_abbrev(col)))
            .title_style(Style::default().fg(title_color))
            .border_style(Style::default().fg(border_color));

        let col_inner = col_block.inner(col_areas[i]);
        frame.render_widget(col_block, col_areas[i]);

        if tasks.is_empty() {
            // Show count indicator
            frame.render_widget(
                Paragraph::new(Span::styled("·", Style::default().fg(Color::DarkGray))),
                col_inner,
            );
            continue;
        }

        let items: Vec<ListItem> = tasks.iter().enumerate().map(|(j, task)| {
            let selected = active && j == app.board_task;
            let prefix = if selected { "▸" } else { " " };
            let title = truncate(&task.title, col_inner.width.saturating_sub(2) as usize);
            let style = if selected {
                Style::default().add_modifier(Modifier::BOLD).fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Line::from(Span::styled(format!("{prefix}{title}"), style)))
        }).collect();

        frame.render_widget(List::new(items), col_inner);
    }
}

// ── Review + Board split ──────────────────────────────────────────────────────

fn draw_review_with_board(frame: &mut Frame, app: &App, area: Rect) {
    // Top: review panel, Bottom: compact board
    let [review_area, board_area] = Layout::vertical([
        Constraint::Ratio(60, 100),
        Constraint::Fill(1),
    ]).areas(area);

    draw_review_panel(frame, app, review_area);
    draw_board(frame, app, board_area);
}

fn draw_review_panel(frame: &mut Frame, app: &App, area: Rect) {
    let border_color = Color::Yellow;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Review ")
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(ref item) = app.review_item else {
        frame.render_widget(
            Paragraph::new("No items pending"),
            inner,
        );
        return;
    };

    let mut lines: Vec<Line> = Vec::new();

    // Task ID
    lines.push(Line::from(vec![
        Span::styled("Task: ", Style::default().fg(Color::DarkGray)),
        Span::styled(&item.task_id, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
    ]));
    lines.push(Line::from(""));

    match &item.kind {
        ReviewItemKind::AgentQuestion { question } => {
            lines.push(Line::from(Span::styled(
                "Agent Question:",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                question.as_str(),
                Style::default().fg(Color::White),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "[a] answer",
                Style::default().fg(Color::Green),
            )));
        }
        ReviewItemKind::BufferApproval { buffer_column, task_priority } => {
            lines.push(Line::from(Span::styled(
                "Buffer Approval:",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("Column: ", Style::default().fg(Color::DarkGray)),
                Span::styled(buffer_column.as_str(), Style::default().fg(Color::White)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Priority: ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{task_priority}"), Style::default().fg(Color::White)),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("[a] approve  ", Style::default().fg(Color::Green)),
                Span::styled("[r] reject", Style::default().fg(Color::Red)),
            ]));
        }
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        inner,
    );
}

// ── Input panel (reject feedback / answer) ────────────────────────────────────

fn draw_input_panel(frame: &mut Frame, app: &App, area: Rect) {
    let [prompt_area, input_area, board_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Fill(1),
    ]).areas(area);

    let prompt_text = match app.mode {
        Mode::RejectInput => "Rejection feedback:",
        Mode::AnswerInput => "Answer:",
        _ => "",
    };

    let prompt_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    let prompt_inner = prompt_block.inner(prompt_area);
    frame.render_widget(prompt_block, prompt_area);
    frame.render_widget(
        Paragraph::new(Span::styled(prompt_text, Style::default().fg(Color::Yellow))),
        prompt_inner,
    );

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::White));
    let input_inner = input_block.inner(input_area);
    frame.render_widget(input_block, input_area);

    let cursor = if app.input.is_empty() {
        "▌".to_string()
    } else {
        format!("{}▌", app.input)
    };
    frame.render_widget(
        Paragraph::new(Span::styled(cursor, Style::default().fg(Color::White))),
        input_inner,
    );

    draw_board(frame, app, board_area);
}

// ── Footer ────────────────────────────────────────────────────────────────────

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let controls = match app.mode {
        Mode::Idle => " [ctrl+q] quit  [←→↑↓] navigate",
        Mode::Review => " [ctrl+q] quit  [a] approve/answer  [r] reject  [←→↑↓] board",
        Mode::RejectInput | Mode::AnswerInput => " [enter] submit  [esc] cancel",
    };

    let status = &app.status;
    let footer = format!("{controls}  │ {status}");

    frame.render_widget(
        Paragraph::new(Span::styled(footer, Style::default().fg(Color::DarkGray))),
        area,
    );
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn mode_label(mode: &Mode) -> &'static str {
    match mode {
        Mode::Idle => "Board",
        Mode::Review => "Review",
        Mode::RejectInput => "Reject",
        Mode::AnswerInput => "Answer",
    }
}

fn column_title_color(col: &Column) -> Color {
    if col.is_buffer() {
        Color::DarkGray
    } else {
        match col {
            Column::Prioritized => Color::Blue,
            Column::Designing => Color::Blue,
            Column::Coding => Color::Yellow,
            Column::Reviewing => Color::Magenta,
            Column::Testing => Color::Cyan,
            Column::Auditing => Color::Red,
            Column::Deploying => Color::Green,
            Column::Done => Color::Green,
            _ => Color::White,
        }
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
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
