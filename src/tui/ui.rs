use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use crate::model::Stage;
use super::app::{App, Focus, STAGES};

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Vertical layout: header | body | footer
    let [header_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);

    // Header
    let header = Paragraph::new(Line::from(vec![Span::styled(
        " gitzi",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )]));
    frame.render_widget(header, header_area);

    // Body: epics (30%) + kanban (70%)
    let [epics_area, kanban_area] = Layout::vertical([
        Constraint::Percentage(30),
        Constraint::Percentage(70),
    ])
    .areas(body_area);

    draw_epics(frame, app, epics_area);
    draw_kanban(frame, app, kanban_area);

    // Footer
    let hints = if app.focus == Focus::Epics {
        " Tab: switch panel  ↑↓/jk: navigate  r: reload  q/Esc: quit"
    } else {
        " Tab: switch panel  ↑↓/jk: navigate  ←→/hl: switch column  r: reload  q/Esc: quit"
    };
    let footer = Paragraph::new(Line::from(vec![Span::styled(
        hints,
        Style::default().fg(Color::DarkGray),
    )]));
    frame.render_widget(footer, footer_area);
}

fn draw_epics(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == Focus::Epics {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Epics ")
        .border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.epics.is_empty() {
        let empty = Paragraph::new("No epics found.");
        frame.render_widget(empty, inner);
        return;
    }

    // Build list items
    let items: Vec<ListItem> = app
        .epics
        .iter()
        .enumerate()
        .map(|(i, epic)| {
            let (done, total) = app.epic_task_counts(&epic.id);
            let selected = i == app.epic_idx;
            let prefix = if selected { "▸ " } else { "  " };
            let counts = format!("{done}/{total}");

            // Compute available width for title
            let available = inner.width as usize;
            let counts_len = counts.len();
            let prefix_len = prefix.len();
            // "prefix + title + spaces + counts" should fit in available width
            let title_max = available.saturating_sub(prefix_len + counts_len + 1);
            let title = truncate(&epic.title, title_max);
            let padding = available
                .saturating_sub(prefix_len + title.chars().count() + counts_len);
            let row_text = format!("{prefix}{title}{:padding$}{counts}", "", padding = padding);

            let style = if selected {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            ListItem::new(Line::from(Span::styled(row_text, style)))
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, inner);
}

fn draw_kanban(frame: &mut Frame, app: &App, area: Rect) {
    let outer_block = Block::default()
        .borders(Borders::ALL)
        .title(" Kanban ")
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = outer_block.inner(area);
    frame.render_widget(outer_block, area);

    // 6 equal columns
    let col_constraints: Vec<Constraint> = (0..STAGES.len())
        .map(|_| Constraint::Ratio(1, STAGES.len() as u32))
        .collect();

    let col_areas = Layout::horizontal(col_constraints).split(inner);

    for (i, stage) in STAGES.iter().enumerate() {
        let tasks = app.tasks_in_stage(stage);
        let count = tasks.len();
        let col_title = format!(" {} ({}) ", stage_label(stage), count);

        let is_active_col = app.focus == Focus::Kanban && i == app.col_idx;
        let col_border_style = if is_active_col {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let col_block = Block::default()
            .borders(Borders::ALL)
            .title(col_title.as_str())
            .border_style(col_border_style);

        let col_inner = col_block.inner(col_areas[i]);
        frame.render_widget(col_block, col_areas[i]);

        if tasks.is_empty() {
            continue;
        }

        let items: Vec<ListItem> = tasks
            .iter()
            .enumerate()
            .map(|(j, task)| {
                let selected = is_active_col && j == app.task_idx;
                let prefix = if selected { "▸ " } else { "  " };
                let max_chars = col_inner.width as usize;
                let title_max = max_chars.saturating_sub(prefix.len());
                let title = truncate(&task.title, title_max);
                let text = format!("{prefix}{title}");

                let style = if selected {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(Span::styled(text, style)))
            })
            .collect();

        let list = List::new(items);
        frame.render_widget(list, col_inner);
    }
}

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
    if max_chars == 0 {
        return String::new();
    }
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else if max_chars <= 1 {
        "…".to_string()
    } else {
        let truncated: String = s.chars().take(max_chars - 1).collect();
        format!("{truncated}…")
    }
}
