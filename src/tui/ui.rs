use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use crate::dispatcher::Column;
use super::app::{App, Panel, column_order, column_abbrev};

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

// -- Body (conditionally includes fork strip) -------------------------------

fn draw_body(frame: &mut Frame, app: &App, area: Rect) {
    if !app.llm_available {
        // No LLM — full width right panel (no chat, no fork strip)
        let [right_area, sidebar_area] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(3),
        ]).areas(area);
        draw_right_panel(frame, app, right_area);
        draw_sidebar(frame, app, sidebar_area);
        return;
    }
    if app.fork_stack.is_empty() {
        let [chat_area, right_area, sidebar_area] = Layout::horizontal([
            Constraint::Ratio(35, 100),
            Constraint::Fill(1),
            Constraint::Length(3),
        ]).areas(area);
        draw_chat_pane(frame, app, chat_area);
        draw_right_panel(frame, app, right_area);
        draw_sidebar(frame, app, sidebar_area);
    } else {
        let [fork_strip, chat_area, right_area, sidebar_area] = Layout::horizontal([
            Constraint::Length(3),
            Constraint::Ratio(33, 100),
            Constraint::Fill(1),
            Constraint::Length(3),
        ]).areas(area);
        draw_fork_strip(frame, app, fork_strip);
        draw_chat_pane(frame, app, chat_area);
        draw_right_panel(frame, app, right_area);
        draw_sidebar(frame, app, sidebar_area);
    }
}

// -- Fork strip (3-char wide, left edge when forks active) -------------------

fn draw_fork_strip(frame: &mut Frame, app: &App, area: Rect) {
    let total = app.fork_stack.len();
    let mut constraints: Vec<Constraint> = Vec::new();
    constraints.push(Constraint::Fill(1)); // top padding
    for _ in 0..total {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Fill(1)); // bottom padding
    let rows = Layout::vertical(constraints).split(area);

    for (i, _fork) in app.fork_stack.iter().enumerate() {
        let active = i == total - 1;
        let style = if active {
            Style::default()
                .bg(Color::Magenta)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let label = format!(" {} ", i + 1);
        frame.render_widget(
            Paragraph::new(Span::styled(label, style)),
            rows[i + 1],
        );
    }
}

// -- Header -----------------------------------------------------------------

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let connected_indicator = if app.connected { "●" } else { "○" };
    let conn_color = if app.connected { Color::Green } else { Color::Red };

    let mut spans = vec![
        Span::styled(
            " gitzi",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(connected_indicator, Style::default().fg(conn_color)),
        Span::styled("  --  ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.panel.label(), Style::default().fg(Color::White)),
    ];

    if let Some(fork) = app.fork_stack.last() {
        spans.push(Span::styled(
            format!("  #{}: {}", app.fork_stack.len(), fork.name),
            Style::default().fg(Color::Magenta),
        ));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        area,
    );
}

// -- Chat pane (left, 35%) --------------------------------------------------

fn draw_chat_pane(frame: &mut Frame, app: &App, area: Rect) {
    let title = if app.chat_pending {
        " Chat  thinking... "
    } else {
        " Chat * "
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Cyan));
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
    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let input_inner = input_block.inner(area);
    frame.render_widget(input_block, area);

    let cursor = if app.chat_pending {
        format!("{}...", app.chat_input)
    } else if app.chat_input.is_empty() {
        "|".to_string()
    } else {
        format!("{}|", app.chat_input)
    };

    frame.render_widget(
        Paragraph::new(Span::styled(cursor, Style::default().fg(Color::White))),
        input_inner,
    );
}

// -- Sidebar (3-char wide, far right) ---------------------------------------

fn draw_sidebar(frame: &mut Frame, app: &App, area: Rect) {
    let mut constraints: Vec<Constraint> = Vec::new();
    constraints.push(Constraint::Fill(1)); // top padding
    for _ in Panel::ALL {
        constraints.push(Constraint::Length(3)); // 3-high cell per panel
    }
    constraints.push(Constraint::Fill(1)); // bottom padding

    let rows = Layout::vertical(constraints).split(area);

    for (i, panel) in Panel::ALL.iter().enumerate() {
        let active = *panel == app.panel;
        let style = if active {
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let label = format!(" {} ", panel.key());
        // Render letter on the middle row of the 3-high cell
        let cell = rows[i + 1]; // +1 to skip top padding
        let [_, mid, _] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ]).areas(cell);
        frame.render_widget(
            Paragraph::new(Span::styled(label, style)),
            mid,
        );
    }
}

// -- Right panel (dispatched by app.panel) ----------------------------------

fn draw_right_panel(frame: &mut Frame, app: &App, area: Rect) {
    if app.editor_focused {
        draw_editor(frame, app, area);
        return;
    }
    match app.panel {
        Panel::Status => draw_status(frame, app, area),
        Panel::Kanban => draw_board(frame, app, area),
        Panel::Epic => draw_epic(frame, app, area),
        Panel::Task => draw_task(frame, app, area),
        Panel::Logs => draw_logs(frame, app, area),
    }
}

fn draw_editor(frame: &mut Frame, app: &App, area: Rect) {
    let dirty_indicator = if app.editor_dirty { " [modified]" } else { "" };
    let title = format!(" Edit{dirty_indicator} ");
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let text = format!("{}|", app.editor_buffer);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White)),
        inner,
    );
}

fn draw_epic(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Epic ")
        .border_style(Style::default().fg(Color::Magenta));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    match app.current_epic_status() {
        Some(epic_status) => {
            // Find the full epic data
            let epic_data = app.epics.iter().find(|e| e.title == epic_status.title);

            lines.push(Line::from(Span::styled(
                epic_status.title.clone(),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));

            // Progress
            lines.push(Line::from(Span::styled(
                format!("Progress: {} / {} tasks done", epic_status.done, epic_status.total),
                Style::default().fg(Color::Cyan),
            )));
            lines.push(Line::from(""));

            // Description
            if let Some(epic) = epic_data {
                if let Some(ref desc) = epic.description {
                    lines.push(section_header("Description"));
                    let width = inner.width.saturating_sub(2) as usize;
                    for line in wrap_text(desc, width) {
                        lines.push(Line::from(Span::styled(
                            line,
                            Style::default().fg(Color::Gray),
                        )));
                    }
                    lines.push(Line::from(""));
                }

                // Task list
                lines.push(section_header(&format!("Tasks ({})", epic.tasks.len())));
                for task_id in &epic.tasks {
                    // Try to find the task in the board to show its column
                    let col_label = column_order()
                        .iter()
                        .find_map(|col| {
                            app.board.get(&col.to_string()).and_then(|tasks| {
                                tasks.iter().find(|t| t.id == *task_id)
                                    .map(|_| column_abbrev(col))
                            })
                        })
                        .unwrap_or("???");

                    let task_title = column_order()
                        .iter()
                        .find_map(|col| {
                            app.board.get(&col.to_string()).and_then(|tasks| {
                                tasks.iter().find(|t| t.id == *task_id)
                                    .map(|t| t.title.as_str())
                            })
                        })
                        .unwrap_or(task_id);

                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("  [{col_label}] "),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled(
                            task_title.to_string(),
                            Style::default().fg(Color::White),
                        ),
                    ]));
                }
            }
        }
        None => {
            lines.push(Line::from(Span::styled(
                "No active epic",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        inner,
    );
}

fn draw_task(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Task ")
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    match app.selected_board_task() {
        Some(task) => {
            // Title
            lines.push(Line::from(Span::styled(
                task.title.clone(),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));

            // ID and priority
            lines.push(Line::from(vec![
                Span::styled("ID: ", Style::default().fg(Color::DarkGray)),
                Span::styled(task.id.clone(), Style::default().fg(Color::Cyan)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Priority: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    task.priority.to_string(),
                    Style::default().fg(Color::White),
                ),
            ]));

            // Column
            let col_name = column_order()
                .iter()
                .find_map(|col| {
                    app.board.get(&col.to_string()).and_then(|tasks| {
                        tasks.iter().find(|t| t.id == task.id)
                            .map(|_| col.to_string())
                    })
                })
                .unwrap_or_else(|| "unknown".to_string());
            lines.push(Line::from(vec![
                Span::styled("Stage: ", Style::default().fg(Color::DarkGray)),
                Span::styled(col_name, Style::default().fg(Color::Green)),
            ]));

            if !task.epic.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("Epic: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(task.epic.clone(), Style::default().fg(Color::Magenta)),
                ]));
            }

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Select a task on the Kanban board to view details",
                Style::default().fg(Color::DarkGray),
            )));
        }
        None => {
            lines.push(Line::from(Span::styled(
                "No task selected",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Navigate to Kanban [k] and select a task",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        inner,
    );
}

fn draw_logs(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Logs ")
        .border_style(Style::default().fg(Color::Red));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible_height = inner.height as usize;
    let skip = app.logs.len().saturating_sub(visible_height);

    let lines: Vec<Line> = app.logs.iter().skip(skip).map(|entry| {
        Line::from(Span::styled(
            truncate(entry, inner.width as usize),
            Style::default().fg(Color::Gray),
        ))
    }).collect();

    frame.render_widget(Paragraph::new(lines), inner);
}

// -- Status card ------------------------------------------------------------

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    if !app.llm_available {
        draw_status_no_llm(frame, app, area);
    } else if !app.discovered_providers.is_empty() && app.epics.is_empty() {
        draw_status_first_time(frame, app, area);
    } else {
        draw_status_normal(frame, app, area);
    }
}

fn draw_status_no_llm(frame: &mut Frame, _app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Status ")
        .border_style(Style::default().fg(Color::Red));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  No LLM provider available",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  gitzi needs a local LLM to operate. Install one of:",
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  * LM Studio  — lmstudio.ai",
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            "  * Ollama     — ollama.com",
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Then restart gitzi.",
            Style::default().fg(Color::Gray),
        )),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_status_first_time(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Status ")
        .border_style(Style::default().fg(Color::Blue));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    // Providers
    lines.push(section_header("LLM Providers"));
    if app.discovered_providers.is_empty() {
        lines.push(Line::from(Span::styled(
            "  None found",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for (name, running) in &app.discovered_providers {
            let status = if *running { "running" } else { "installed" };
            let color = if *running { Color::Green } else { Color::Yellow };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {name}"),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("  ({status})"),
                    Style::default().fg(color),
                ),
            ]));
        }
    }
    lines.push(Line::from(""));

    // Repos
    lines.push(section_header("Repositories"));
    if app.discovered_repos.is_empty() {
        lines.push(Line::from(Span::styled(
            "  None discovered",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for (path, summary) in &app.discovered_repos {
            let short_path = path
                .rsplit('/')
                .take(2)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("/");
            lines.push(Line::from(Span::styled(
                format!("  {short_path}"),
                Style::default().fg(Color::Cyan),
            )));
            if !summary.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("    {summary}"),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        inner,
    );
}

fn draw_status_normal(frame: &mut Frame, app: &App, area: Rect) {
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
        lines.push(Line::from(Span::styled(
            "Nothing in flight",
            Style::default().fg(Color::DarkGray),
        )));
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
        lines.push(Line::from(Span::styled(
            "Nothing pending",
            Style::default().fg(Color::DarkGray),
        )));
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
        if app.question_count > 0 {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        },
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
    Line::from(Span::styled(
        format!("  * {text}"),
        Style::default().fg(color),
    ))
}

// -- Board rendering (work columns only, buffer items shown inline) ---------

fn draw_board(frame: &mut Frame, app: &App, area: Rect) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(" Board ")
        .border_style(Style::default().fg(Color::Green));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    // Only display non-buffer columns
    let visible_columns: Vec<&Column> = column_order()
        .iter()
        .filter(|c| !c.is_buffer())
        .collect();

    let col_constraints: Vec<Constraint> = (0..visible_columns.len())
        .map(|_| Constraint::Ratio(1, visible_columns.len() as u32))
        .collect();
    let col_areas = Layout::horizontal(col_constraints).split(inner);

    for (i, col) in visible_columns.iter().enumerate() {
        // Combine the column's own tasks with any tasks from the next buffer column
        let own_tasks = app.board.get(&col.to_string())
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let buffer_col = col.next().filter(|c| c.is_buffer());
        let buffer_tasks = buffer_col
            .and_then(|bc| app.board.get(&bc.to_string()))
            .map(|v| v.as_slice())
            .unwrap_or(&[]);

        let active = i == app.board_col_visible();
        let border_color = if active { Color::Green } else { Color::DarkGray };
        let title_color = column_title_color(col);

        let col_block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", column_abbrev(col)))
            .title_style(Style::default().fg(title_color))
            .border_style(Style::default().fg(border_color));

        let col_inner = col_block.inner(col_areas[i]);
        frame.render_widget(col_block, col_areas[i]);

        if own_tasks.is_empty() && buffer_tasks.is_empty() {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    ".",
                    Style::default().fg(Color::DarkGray),
                )),
                col_inner,
            );
            continue;
        }

        let mut items: Vec<ListItem> = Vec::new();
        let mut task_idx = 0;

        // Buffer tasks first (highlighted in orange with asterisk)
        for task in buffer_tasks {
            let selected = active && task_idx == app.board_task;
            let prefix = if selected { ">*" } else { " *" };
            let title = truncate(
                &task.title,
                col_inner.width.saturating_sub(3) as usize,
            );
            let style = if selected {
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Rgb(255, 165, 0))
            } else {
                Style::default().fg(Color::Rgb(255, 165, 0))
            };
            items.push(ListItem::new(Line::from(Span::styled(
                format!("{prefix}{title}"),
                style,
            ))));
            task_idx += 1;
        }

        // Own tasks (normal styling)
        for task in own_tasks {
            let selected = active && task_idx == app.board_task;
            let prefix = if selected { "> " } else { "  " };
            let title = truncate(
                &task.title,
                col_inner.width.saturating_sub(3) as usize,
            );
            let style = if selected {
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            };
            items.push(ListItem::new(Line::from(Span::styled(
                format!("{prefix}{title}"),
                style,
            ))));
            task_idx += 1;
        }

        frame.render_widget(List::new(items), col_inner);
    }
}

// -- Footer -----------------------------------------------------------------

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let controls = if app.editor_focused {
        " [ctrl+s] save  [esc] leave editor  [ctrl+q] quit"
    } else if app.panel_focused {
        " [arrows] navigate  [enter] select  [esc] back to chat  [ctrl+q] quit"
    } else if !app.fork_stack.is_empty() {
        " [enter] send  [tab] panel  [ctrl+↑↓] panels  [esc] close fork  [ctrl+q] quit"
    } else {
        " [enter] send  [tab] panel  [ctrl+↑↓] panels  [ctrl+q] quit"
    };

    let status = &app.status;
    let footer = format!("{controls}  | {status}");

    frame.render_widget(
        Paragraph::new(Span::styled(footer, Style::default().fg(Color::DarkGray))),
        area,
    );
}

// -- Helpers ----------------------------------------------------------------

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
        "..".to_string()
    } else {
        let t: String = chars[..max_chars - 1].iter().collect();
        format!("{t}..")
    }
}
