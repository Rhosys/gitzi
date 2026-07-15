use super::app::{App, Panel, column_abbrev, column_order};
use crate::dispatcher::Column;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

pub fn draw(frame: &mut Frame, app: &App) {
    // Bootstrap setup (ADR-002) owns the whole screen until the gate clears.
    if app.in_setup() {
        draw_setup(frame, app, frame.area());
        return;
    }

    let area = frame.area();
    let [header_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, app, header_area);
    draw_body(frame, app, body_area);
    draw_footer(frame, app, footer_area);
}

// -- Bootstrap setup screen (ADR-002) ---------------------------------------

/// Full-screen setup renderer. A thin switch over the daemon-owned
/// `SetupState`: splash while loading, a provider picker, or an error with a
/// rescan hint. All logic lives in the backend; this only draws.
fn draw_setup(frame: &mut Frame, app: &App, area: Rect) {
    use crate::setup::SetupState;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" gitzi setup ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [title_area, body_area, msg_area, footer_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Set up a language model to get started",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ))),
        title_area,
    );

    let (body, footer): (Vec<Line>, &str) = match &app.setup_state {
        Some(SetupState::Loading) | None => (
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  Discovering LLM providers on this machine…",
                    Style::default().fg(Color::Gray),
                )),
            ],
            "Ctrl+Q quit",
        ),
        Some(SetupState::NeedsProvider { candidates }) => {
            let mut lines = vec![
                Line::from(Span::styled(
                    "  Choose a provider to activate:",
                    Style::default().fg(Color::Gray),
                )),
                Line::from(""),
            ];
            for (i, c) in candidates.iter().enumerate() {
                let selected = i == app.setup_selected;
                let marker = if selected { "▶ " } else { "  " };
                let style = if selected {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::default().fg(Color::White)
                };
                let ready = if c.ready { "" } else { "  (needs setup)" };
                lines.push(Line::from(Span::styled(
                    format!("{marker}{}  [{}]  {}{ready}", c.name, c.kind, c.status),
                    style,
                )));
            }
            (
                lines,
                "↑/↓ select   Enter activate   r rescan   Ctrl+Q quit",
            )
        }
        Some(SetupState::Error {
            messages,
            can_rescan,
        }) => {
            let mut lines = vec![
                Line::from(Span::styled(
                    "  Setup can't continue yet:",
                    Style::default()
                        .fg(Color::LightRed)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            for m in messages {
                lines.push(Line::from(Span::styled(
                    format!("  {m}"),
                    Style::default().fg(Color::LightYellow),
                )));
            }
            let footer = if *can_rescan {
                "r rescan   Ctrl+Q quit"
            } else {
                "Ctrl+Q quit"
            };
            (lines, footer)
        }
        Some(SetupState::Ready) => (vec![Line::from("  Ready — launching…")], "Ctrl+Q quit"),
    };

    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), body_area);

    if let Some(msg) = &app.setup_message {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("  {msg}"),
                Style::default().fg(Color::Yellow),
            )))
            .wrap(Wrap { trim: false }),
            msg_area,
        );
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            footer,
            Style::default().fg(Color::DarkGray),
        ))),
        footer_area,
    );
}

// -- Body (conditionally includes fork strip) -------------------------------

fn draw_body(frame: &mut Frame, app: &App, area: Rect) {
    if !app.llm_available {
        // No LLM — full width right panel (no chat, no fork strip)
        let [right_area, sidebar_area] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(3)]).areas(area);
        draw_right_panel(frame, app, right_area);
        draw_sidebar(frame, app, sidebar_area);
        return;
    }
    if app.fork_stack.is_empty() {
        let [chat_area, right_area, sidebar_area] = Layout::horizontal([
            Constraint::Ratio(35, 100),
            Constraint::Fill(1),
            Constraint::Length(3),
        ])
        .areas(area);
        draw_chat_pane(frame, app, chat_area);
        draw_right_panel(frame, app, right_area);
        draw_sidebar(frame, app, sidebar_area);
    } else {
        let [fork_strip, chat_area, right_area, sidebar_area] = Layout::horizontal([
            Constraint::Length(20),
            Constraint::Ratio(33, 100),
            Constraint::Fill(1),
            Constraint::Length(3),
        ])
        .areas(area);
        draw_fork_strip(frame, app, fork_strip);
        draw_chat_pane(frame, app, chat_area);
        draw_right_panel(frame, app, right_area);
        draw_sidebar(frame, app, sidebar_area);
    }
}

// -- Fork strip (3-char wide, left edge when forks active) -------------------

fn draw_fork_strip(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Build the stack path: "main > fork1 > fork2"
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        "main",
        Style::default().fg(Color::DarkGray),
    )));

    for (i, fork) in app.fork_stack.iter().enumerate() {
        let active = i == app.fork_stack.len() - 1;
        let style = if active {
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        // Truncate name to fit the strip width
        let max_w = inner.width as usize;
        let display = if fork.name.len() > max_w {
            format!("{}..", &fork.name[..max_w.saturating_sub(2)])
        } else {
            fork.name.clone()
        };
        lines.push(Line::from(Span::styled(format!(" > {display}"), style)));
    }

    // Render from top
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

// -- Header -----------------------------------------------------------------

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let connected_indicator = if app.connected { "●" } else { "○" };
    let conn_color = if app.connected {
        Color::Green
    } else {
        Color::Red
    };

    let mut spans = vec![
        Span::styled(
            " gitzi",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
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

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// -- Chat pane (left, 35%) --------------------------------------------------

fn draw_chat_pane(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Chat ")
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Split inner: history fills top, input height expands with wrapped text
    let input_width = inner.width.saturating_sub(4) as usize; // inner of input box
    let input_lines = (app.chat_input.len() + 1)
        .checked_div(input_width)
        .map(|q| (q + 1).min(6) as u16)
        .unwrap_or(1);
    let input_height = input_lines + 2; // +2 for border

    let [history_area, input_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(input_height)]).areas(inner);

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
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "gitzi",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
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

    // Show spinner + progress bar while waiting for LLM response
    if app.chat_pending {
        // Tool list
        if !app.stream_tools.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("Tools: [{}]", app.stream_tools.join(", ")),
                Style::default().fg(Color::Magenta),
            )));
        }

        // Progress bar
        if app.stream_tokens > 0 {
            let progress = app.stream_progress();
            let bar_width = width.min(30);
            let filled = (progress * bar_width as f64) as usize;
            let empty = bar_width.saturating_sub(filled);
            let bar = format!(
                "[{}{}] {:>3}%",
                "█".repeat(filled),
                "░".repeat(empty),
                (progress * 100.0) as u8,
            );
            lines.push(Line::from(Span::styled(
                bar,
                Style::default().fg(Color::Cyan),
            )));
        } else {
            let spinner_frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let tick = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                / 100) as usize;
            let frame_char = spinner_frames[tick % spinner_frames.len()];
            lines.push(Line::from(Span::styled(
                format!("{frame_char} thinking..."),
                Style::default().fg(Color::Yellow),
            )));
        }
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

    // Word-wrap the input text across multiple lines within available width
    let width = input_inner.width as usize;
    let display = format!("{}|", app.chat_input);
    let wrapped = wrap_text_char(&display, width);
    let lines: Vec<Line> = wrapped
        .into_iter()
        .map(|l| Line::from(Span::styled(l, Style::default().fg(Color::White))))
        .collect();

    frame.render_widget(Paragraph::new(lines), input_inner);
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
        ])
        .areas(cell);
        frame.render_widget(Paragraph::new(Span::styled(label, style)), mid);
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
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));

            // Progress
            lines.push(Line::from(Span::styled(
                format!(
                    "Progress: {} / {} tasks done",
                    epic_status.done, epic_status.total
                ),
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
                                tasks
                                    .iter()
                                    .find(|t| t.id == *task_id)
                                    .map(|_| column_abbrev(col))
                            })
                        })
                        .unwrap_or("???");

                    let task_title = column_order()
                        .iter()
                        .find_map(|col| {
                            app.board.get(&col.to_string()).and_then(|tasks| {
                                tasks
                                    .iter()
                                    .find(|t| t.id == *task_id)
                                    .map(|t| t.title.as_str())
                            })
                        })
                        .unwrap_or(task_id);

                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("  [{col_label}] "),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled(task_title.to_string(), Style::default().fg(Color::White)),
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

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
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
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));

            // ID and priority
            lines.push(Line::from(vec![
                Span::styled("ID: ", Style::default().fg(Color::DarkGray)),
                Span::styled(task.id.clone(), Style::default().fg(Color::Cyan)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Priority: ", Style::default().fg(Color::DarkGray)),
                Span::styled(task.priority.to_string(), Style::default().fg(Color::White)),
            ]));

            // Column
            let col_name = column_order()
                .iter()
                .find_map(|col| {
                    app.board.get(&col.to_string()).and_then(|tasks| {
                        tasks
                            .iter()
                            .find(|t| t.id == task.id)
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

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
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

    let lines: Vec<Line> = app
        .logs
        .iter()
        .skip(skip)
        .map(|entry| {
            Line::from(Span::styled(
                truncate(entry, inner.width as usize),
                Style::default().fg(Color::Gray),
            ))
        })
        .collect();

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
        .border_style(Style::default().fg(Color::LightRed));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  No LLM provider configured",
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Run `gitzi generate-config` to rescan,",
            Style::default().fg(Color::LightYellow),
        )),
        Line::from(Span::styled(
            "  or press 'r' to rescan from here.",
            Style::default().fg(Color::LightYellow),
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
            let color = if *running {
                Color::Green
            } else {
                Color::Yellow
            };
            lines.push(Line::from(vec![
                Span::styled(format!("  {name}"), Style::default().fg(Color::White)),
                Span::styled(format!("  ({status})"), Style::default().fg(color)),
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

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
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
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
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
    lines.push(section_header(&format!(
        "In progress ({})",
        in_progress.len()
    )));
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
    lines.push(section_header(&format!(
        "Waiting for you ({})",
        waiting.len()
    )));
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

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn section_header(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD),
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

    // Visible columns: non-buffer, skip Done
    let visible_columns: Vec<&Column> = column_order()
        .iter()
        .filter(|c| !c.is_buffer() && **c != Column::Done)
        .collect();

    // One row per column (vertical layout)
    let row_constraints: Vec<Constraint> = (0..visible_columns.len())
        .map(|_| Constraint::Length(1))
        .collect();
    let row_areas = Layout::vertical(row_constraints).split(inner);

    let label_width: usize = 5;

    for (i, col) in visible_columns.iter().enumerate() {
        let active = i == app.board_col_visible();
        let tasks = app.tasks_in_visible_column(i);

        // Build task string: comma-separated titles, selected gets ">" prefix
        let available_width = inner.width.saturating_sub(label_width as u16 + 3) as usize;
        let task_str = if tasks.is_empty() {
            String::new()
        } else {
            let parts: Vec<String> = tasks
                .iter()
                .enumerate()
                .map(|(ti, t)| {
                    let selected = active && ti == app.board_task;
                    if selected {
                        format!(">{}", t.title)
                    } else {
                        t.title.clone()
                    }
                })
                .collect();
            let joined = parts.join(", ");
            truncate(&joined, available_width)
        };

        // Label style
        let label_style = if active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(column_title_color(col))
        };

        let task_style = if active {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::Gray)
        };

        let label = format!("{:<width$}", column_abbrev(col), width = label_width);
        let line = Line::from(vec![
            Span::styled(label, label_style),
            Span::styled(" \u{2502} ", Style::default().fg(Color::DarkGray)),
            Span::styled(task_str, task_style),
        ]);

        frame.render_widget(Paragraph::new(line), row_areas[i]);
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

/// Character-level wrapping for the input field — breaks at column width
/// without caring about word boundaries.
fn wrap_text_char(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars
        .chunks(width)
        .map(|chunk| chunk.iter().collect())
        .collect()
}
