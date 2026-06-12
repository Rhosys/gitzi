use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use crate::model::Stage;
use crate::state::chat::Role;
use super::app::{App, Focus, RightPanel, STAGES};

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let [header_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ]).areas(area);

    // ── Header ────────────────────────────────────────────────────────────────
    let panel_title = match &app.panel {
        RightPanel::Board => "Board".to_string(),
        RightPanel::EpicDetail(id) => {
            let title = app.epics.iter().find(|e| &e.id == id)
                .map(|e| e.title.as_str())
                .unwrap_or("?");
            format!("Epic: {title}")
        }
        RightPanel::TaskDetail(id) => {
            let title = app.tasks.iter().find(|t| &t.id == id)
                .map(|t| t.title.as_str())
                .unwrap_or("?");
            format!("Task: {title}")
        }
    };

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" gitzi", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled("  —  ", Style::default().fg(Color::DarkGray)),
            Span::styled(panel_title, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ])),
        header_area,
    );

    // ── Body: horizontal split 35% / 65% ─────────────────────────────────────
    let [left_area, right_area] = Layout::horizontal([
        Constraint::Ratio(35, 100),
        Constraint::Fill(1),
    ]).areas(body_area);

    draw_left_pane(frame, app, left_area);
    draw_right_pane(frame, app, right_area);

    // ── Footer ────────────────────────────────────────────────────────────────
    let footer_text = match app.focus {
        Focus::Chat => " [ctrl+q] quit  [tab] panel  [↑↓] scroll  [enter] send",
        Focus::Panel => " [ctrl+q] quit  [tab] chat  [↑↓↔] navigate  [r] reload",
    };
    frame.render_widget(
        Paragraph::new(Span::styled(footer_text, Style::default().fg(Color::DarkGray))),
        footer_area,
    );
}

// ── Left pane ─────────────────────────────────────────────────────────────────

fn draw_left_pane(frame: &mut Frame, app: &App, area: Rect) {
    let [chat_area, input_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(3),
    ]).areas(area);

    draw_chat_history(frame, app, chat_area);
    draw_input_box(frame, app, input_area);
}

fn draw_chat_history(frame: &mut Frame, app: &App, area: Rect) {
    let chat_focused = app.focus == Focus::Chat;
    let border_color = if chat_focused { Color::Yellow } else { Color::DarkGray };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Chat ")
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let inner_height = inner.height as usize;
    let inner_width = inner.width as usize;

    if inner_height == 0 || inner_width == 0 {
        return;
    }

    // Build all lines from messages
    let all_lines: Vec<Line> = app.messages.iter().map(|msg| {
        match msg.role {
            Role::User => {
                let text = format!("> {}", truncate(&msg.content, inner_width.saturating_sub(2)));
                Line::from(Span::styled(text, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)))
            }
            Role::System => {
                let text = format!("  {}", truncate(&msg.content, inner_width.saturating_sub(2)));
                Line::from(Span::styled(text, Style::default().fg(Color::DarkGray)))
            }
            Role::Agent => {
                let text = format!("  {}", truncate(&msg.content, inner_width.saturating_sub(2)));
                Line::from(Span::styled(text, Style::default().fg(Color::Green)))
            }
        }
    }).collect();

    let total = all_lines.len();

    // Compute which lines to show, accounting for scroll offset
    // chat_scroll = 0 means newest visible (show last N lines)
    // chat_scroll = k means scroll up k lines from bottom
    let scroll = app.chat_scroll.min(total.saturating_sub(1));
    let end = total.saturating_sub(scroll);
    let start = end.saturating_sub(inner_height);
    let visible: Vec<Line> = all_lines[start..end].to_vec();

    frame.render_widget(
        Paragraph::new(visible),
        inner,
    );
}

fn draw_input_box(frame: &mut Frame, app: &App, area: Rect) {
    let chat_focused = app.focus == Focus::Chat;
    let border_color = if chat_focused { Color::Yellow } else { Color::DarkGray };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let cursor_text = if app.input.is_empty() {
        "> ▌".to_string()
    } else {
        format!("> {}▌", app.input)
    };

    frame.render_widget(
        Paragraph::new(Span::styled(cursor_text, Style::default().fg(Color::White))),
        inner,
    );
}

// ── Right pane ────────────────────────────────────────────────────────────────

fn draw_right_pane(frame: &mut Frame, app: &App, area: Rect) {
    match &app.panel {
        RightPanel::Board => draw_board(frame, app, area),
        RightPanel::EpicDetail(id) => draw_epic_detail(frame, app, area, id.clone()),
        RightPanel::TaskDetail(id) => draw_task_detail(frame, app, area, id.clone()),
    }
}

fn draw_board(frame: &mut Frame, app: &App, area: Rect) {
    let panel_focused = app.focus == Focus::Panel;
    let outer_border_color = if panel_focused { Color::Green } else { Color::DarkGray };

    let outer_block = Block::default()
        .borders(Borders::ALL)
        .title(" Board ")
        .border_style(Style::default().fg(outer_border_color));

    let inner = outer_block.inner(area);
    frame.render_widget(outer_block, area);

    // 6 equal columns
    let col_constraints: Vec<Constraint> = (0..STAGES.len())
        .map(|_| Constraint::Ratio(1, STAGES.len() as u32))
        .collect();
    let col_areas = Layout::horizontal(col_constraints).split(inner);

    for (i, stage) in STAGES.iter().enumerate() {
        let tasks = app.tasks_in_stage(stage);
        let active = panel_focused && i == app.board_col;
        let border_color = if active { Color::Green } else { Color::DarkGray };

        let col_block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ({}) ", stage_label(stage), tasks.len()))
            .border_style(Style::default().fg(border_color));

        let col_inner = col_block.inner(col_areas[i]);
        frame.render_widget(col_block, col_areas[i]);

        if tasks.is_empty() {
            continue;
        }

        let items: Vec<ListItem> = tasks.iter().enumerate().map(|(j, task)| {
            let selected = active && j == app.board_task;
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

fn draw_epic_detail(frame: &mut Frame, app: &App, area: Rect, id: String) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Epic ")
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let epic = match app.epics.iter().find(|e| e.id == id) {
        Some(e) => e,
        None => {
            frame.render_widget(Paragraph::new("Epic not found"), inner);
            return;
        }
    };

    let mut lines: Vec<Line> = Vec::new();

    // Title
    lines.push(Line::from(Span::styled(
        epic.title.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    )));

    // Description
    if let Some(desc) = &epic.description {
        lines.push(Line::from(Span::styled(
            desc.clone(),
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::from(""));

    // Tasks grouped by stage
    for stage in STAGES.iter() {
        let stage_tasks: Vec<_> = app.tasks.iter()
            .filter(|t| &t.stage == stage && t.epic == id)
            .collect();
        if stage_tasks.is_empty() {
            continue;
        }

        lines.push(Line::from(Span::styled(
            format!("── {} ──", stage_label(stage)),
            Style::default().fg(Color::DarkGray),
        )));

        for task in stage_tasks {
            lines.push(Line::from(Span::styled(
                format!("  {} (p{})", task.title, task.priority),
                Style::default(),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_task_detail(frame: &mut Frame, app: &App, area: Rect, id: String) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Task ")
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let task = match app.tasks.iter().find(|t| t.id == id) {
        Some(t) => t,
        None => {
            frame.render_widget(Paragraph::new("Task not found"), inner);
            return;
        }
    };

    let mut lines: Vec<Line> = Vec::new();

    // Title
    lines.push(Line::from(Span::styled(
        task.title.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    )));

    // Stage
    lines.push(Line::from(vec![
        Span::styled("Stage: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            stage_label(&task.stage).to_string(),
            Style::default().fg(stage_color(&task.stage)),
        ),
    ]));

    // Priority
    lines.push(Line::from(Span::styled(
        format!("Priority: {}", task.priority),
        Style::default().fg(Color::DarkGray),
    )));

    // Epic
    lines.push(Line::from(Span::styled(
        format!("Epic: {}", task.epic),
        Style::default().fg(Color::DarkGray),
    )));

    // Description
    if let Some(desc) = &task.description {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            desc.clone(),
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::from(""));

    // History: last 5 transitions
    let history_start = task.history.len().saturating_sub(5);
    for entry in &task.history[history_start..] {
        let line_text = match entry {
            crate::model::HistoryEntry::StageChange { from, to, at, .. } => {
                let ts = at.format("%Y-%m-%d %H:%M").to_string();
                format!(
                    "  {} → {}  at {}",
                    stage_label(from),
                    stage_label(to),
                    ts
                )
            }
            crate::model::HistoryEntry::Approval { at, target_stage } => {
                let ts = at.format("%Y-%m-%d %H:%M").to_string();
                format!("  ✓ approved → {}  at {}", stage_label(target_stage), ts)
            }
            crate::model::HistoryEntry::Rejection { at, feedback, returned_to } => {
                let ts = at.format("%Y-%m-%d %H:%M").to_string();
                format!("  ✗ rejected → {} ({})  at {}", stage_label(returned_to), feedback, ts)
            }
        };
        lines.push(Line::from(Span::styled(
            line_text,
            Style::default().fg(Color::DarkGray),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
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
        Stage::Designing => "DESIGNING",
        Stage::CodingBuffer => "CODING BUF",
        Stage::Coding => "CODING",
        Stage::ReviewBuffer => "REVIEW BUF",
        Stage::Reviewing => "REVIEWING",
        Stage::TestBuffer => "TEST BUF",
        Stage::Testing => "TESTING",
        Stage::SecurityAuditBuffer => "AUDIT BUF",
        Stage::Auditing => "AUDITING",
        Stage::DeploymentBuffer => "DEPLOY BUF",
        Stage::Deploying => "DEPLOYING",
    }
}

fn stage_color(stage: &Stage) -> Color {
    match stage {
        Stage::Backlog => Color::DarkGray,
        Stage::Prioritized => Color::Blue,
        Stage::InProgress => Color::Yellow,
        Stage::WaitingForReview => Color::Magenta,
        Stage::InTesting => Color::Cyan,
        Stage::Done => Color::Green,
        Stage::Designing => Color::Blue,
        Stage::CodingBuffer => Color::DarkGray,
        Stage::Coding => Color::Yellow,
        Stage::ReviewBuffer => Color::DarkGray,
        Stage::Reviewing => Color::Magenta,
        Stage::TestBuffer => Color::DarkGray,
        Stage::Testing => Color::Cyan,
        Stage::SecurityAuditBuffer => Color::DarkGray,
        Stage::Auditing => Color::Red,
        Stage::DeploymentBuffer => Color::DarkGray,
        Stage::Deploying => Color::Green,
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
