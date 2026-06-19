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

    // Two-pane split: 35% chat left, 65% board/review right
    let [chat_area, right_area] = Layout::horizontal([
        Constraint::Ratio(35, 100),
        Constraint::Fill(1),
    ]).areas(body_area);

    draw_chat_pane(frame, app, chat_area);
    draw_right_pane(frame, app, right_area);

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

// ── Chat pane (left, 35%) ─────────────────────────────────────────────────────

fn draw_chat_pane(frame: &mut Frame, app: &App, area: Rect) {
    let chat_focused = matches!(app.mode, Mode::ChatInput | Mode::ChatWaiting);
    let border_color = if chat_focused { Color::Cyan } else { Color::DarkGray };
    let title = if matches!(app.mode, Mode::ChatWaiting) {
        " Chat  thinking… "
    } else if chat_focused {
        " Chat ● "
    } else {
        " Chat  [c] to type "
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Split inner: history fills top, input sits at bottom (3 lines)
    let [history_area, input_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(3),
    ]).areas(inner);

    draw_chat_history(frame, app, history_area);
    draw_chat_input(frame, app, input_area);
}

fn draw_chat_history(frame: &mut Frame, app: &App, area: Rect) {
    let width = area.width.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::new();

    for entry in &app.chat_history {
        if entry.is_user {
            lines.push(Line::from(Span::styled(
                "You",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "gitzi",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )));
        }

        // Word-wrap content to fit the pane width
        for wrapped_line in wrap_text(&entry.content, width) {
            let style = if entry.is_user {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::Gray)
            };
            lines.push(Line::from(Span::styled(wrapped_line, style)));
        }
        lines.push(Line::from(""));
    }

    // Scroll to show the bottom of history
    let visible_height = area.height as usize;
    let skip = lines.len().saturating_sub(visible_height);

    frame.render_widget(
        Paragraph::new(lines.into_iter().skip(skip).collect::<Vec<_>>()),
        area,
    );
}

fn draw_chat_input(frame: &mut Frame, app: &App, area: Rect) {
    let active = matches!(app.mode, Mode::ChatInput | Mode::ChatWaiting);
    let border_color = if active { Color::Cyan } else { Color::DarkGray };

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let input_inner = input_block.inner(area);
    frame.render_widget(input_block, area);

    let cursor = if matches!(app.mode, Mode::ChatWaiting) {
        "…".to_string()
    } else if app.chat_input.is_empty() {
        "▌".to_string()
    } else {
        format!("{}▌", app.chat_input)
    };

    let style = if active {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    frame.render_widget(
        Paragraph::new(Span::styled(cursor, style)),
        input_inner,
    );
}

// ── Right pane (65%) ──────────────────────────────────────────────────────────

fn draw_right_pane(frame: &mut Frame, app: &App, area: Rect) {
    match &app.mode {
        Mode::Status => draw_status(frame, app, area),
        Mode::Idle | Mode::ChatInput | Mode::ChatWaiting => draw_board(frame, app, area),
        Mode::Review => draw_review_with_board(frame, app, area),
        Mode::AnswerInput => draw_input_panel(frame, app, area),
    }
}

// ── Status card (default landing view) ────────────────────────────────────────

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Status ")
        .border_style(Style::default().fg(Color::Blue));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    // Current epic
    lines.push(section_header("Current epic"));
    match app.current_epic_status() {
        Some(epic) => {
            lines.push(Line::from(Span::styled(
                epic.title,
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                format!("{} / {} tasks done", epic.done, epic.total),
                Style::default().fg(Color::DarkGray),
            )));
        }
        None => lines.push(Line::from(Span::styled(
            "No active epic",
            Style::default().fg(Color::DarkGray),
        ))),
    }
    lines.push(Line::from(""));

    // In progress
    let in_progress = app.tasks_in_progress();
    lines.push(section_header(&format!("In progress ({})", in_progress.len())));
    if in_progress.is_empty() {
        lines.push(Line::from(Span::styled("Nothing in flight", Style::default().fg(Color::DarkGray))));
    } else {
        for task in &in_progress {
            lines.push(bullet_line(&task.title, Color::Yellow));
        }
    }
    lines.push(Line::from(""));

    // Waiting for you
    let waiting = app.tasks_waiting_for_you();
    lines.push(section_header(&format!("Waiting for you ({})", waiting.len())));
    if waiting.is_empty() {
        lines.push(Line::from(Span::styled("Nothing pending", Style::default().fg(Color::DarkGray))));
    } else {
        for task in &waiting {
            lines.push(bullet_line(&task.title, Color::Green));
        }
    }
    lines.push(Line::from(""));

    // Clarification queue
    lines.push(section_header("Clarification queue"));
    lines.push(Line::from(Span::styled(
        format!("{} pending", app.question_count),
        if app.question_count > 0 { Style::default().fg(Color::Cyan) } else { Style::default().fg(Color::DarkGray) },
    )));

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        inner,
    );
}

fn section_header(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
    ))
}

fn bullet_line(text: &str, color: Color) -> Line<'static> {
    Line::from(Span::styled(format!("  • {text}"), Style::default().fg(color)))
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

    // Task title (preferred) or ID as fallback
    if let Some(ref title) = item.task_title {
        lines.push(Line::from(Span::styled(
            title.as_str(),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(vec![
            Span::styled("id: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                item.task_id.get(..8).unwrap_or(&item.task_id),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("Task: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&item.task_id, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]));
    }
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
                Span::styled("[a] advance  ", Style::default().fg(Color::Green)),
                Span::styled("[c] rework + explain", Style::default().fg(Color::Yellow)),
            ]));
        }
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        inner,
    );
}

// ── Input panel (answer) ───────────────────────────────────────────────────────

fn draw_input_panel(frame: &mut Frame, app: &App, area: Rect) {
    let [prompt_area, input_area, board_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Fill(1),
    ]).areas(area);

    let prompt_text = match app.mode {
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
        Mode::Status => " [c] chat  [←→↑↓] board  [ctrl+q] quit",
        Mode::Idle => " [c] chat  [s] status  [←→↑↓] board  [ctrl+q] quit",
        Mode::Review => " [a] advance/answer  [c] chat to rework+explain  [←→↑↓] board  [ctrl+q] quit",
        Mode::AnswerInput => " [enter] submit  [esc] cancel",
        Mode::ChatInput => " [enter] send  [esc] cancel",
        Mode::ChatWaiting => " waiting for response…",
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
        Mode::Status => "Status",
        Mode::Idle => "Board",
        Mode::Review => "Review",
        Mode::AnswerInput => "Answer",
        Mode::ChatInput => "Chat",
        Mode::ChatWaiting => "Chat — thinking…",
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if current.is_empty() {
                current = word.to_string();
            } else if current.len() + 1 + word.len() <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(std::mem::take(&mut current));
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    lines
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
