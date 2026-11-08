use crate::app::{TimelineApp, TimelineRow, ViewMode};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use savepoint_core::ChangeKind;

pub fn render_ui(f: &mut Frame, app: &TimelineApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Top status bar
            Constraint::Min(10),   // Main body: Timeline (left) + Inspector (right)
            Constraint::Length(3), // Bottom keybar
        ])
        .split(f.area());

    render_header(f, chunks[0], app);
    render_body(f, chunks[1], app);
    render_footer(f, chunks[2]);

    if let ViewMode::ConfirmingRewind(step) = app.mode {
        render_confirm_modal(f, step, app.tx.active_step(), app.has_live_changes());
    }
}

fn render_header(f: &mut Frame, area: Rect, app: &TimelineApp) {
    let ws_name = app
        .tx
        .workspace_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "workspace".to_string());

    let (worktree_badge, worktree_style) = match app.rows.first() {
        Some(TimelineRow::Live { deltas }) => (
            format!("DIRTY (+{} files)", deltas.len()),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        _ => (
            "CLEAN".to_string(),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
    };

    let header_line = Line::from(vec![
        Span::styled(
            " SAVEPOINT ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  repo: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            ws_name,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  │  branch: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            app.tx.active_branch.clone(),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  │  HEAD: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("#{:02}", app.tx.active_step()),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  │  tree: ", Style::default().fg(Color::DarkGray)),
        Span::styled(worktree_badge, worktree_style),
        Span::styled("  │  tx: ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.tx.id.to_string(), Style::default().fg(Color::DarkGray)),
    ]);

    let p = Paragraph::new(header_line).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(p, area);
}

fn render_body(f: &mut Frame, area: Rect, app: &TimelineApp) {
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(46), // Timeline list
            Constraint::Percentage(54), // Selected step inspector
        ])
        .split(area);

    render_steps_list(f, body_chunks[0], app);
    render_details(f, body_chunks[1], app);
}

fn render_steps_list(f: &mut Frame, area: Rect, app: &TimelineApp) {
    let mut items = Vec::with_capacity(app.rows.len());
    let active_step = app.tx.active_step();

    for (idx, row) in app.rows.iter().enumerate() {
        let is_selected = idx == app.selected_index;
        let cursor = if is_selected { "> " } else { "  " };
        let row_bg = if is_selected {
            Style::default().bg(Color::DarkGray)
        } else {
            Style::default()
        };

        match row {
            TimelineRow::Live { deltas } => {
                let line = Line::from(vec![
                    Span::styled(
                        cursor,
                        row_bg.fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "*   ",
                        row_bg.fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "WORKTREE ",
                        row_bg.fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "[DIRTY] ",
                        row_bg.fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("{:>2}f  ", deltas.len()), row_bg.fg(Color::Yellow)),
                    Span::styled("Uncommitted workspace changes", row_bg.fg(Color::White)),
                ]);
                items.push(ListItem::new(line));
            }
            TimelineRow::Saved(cp) => {
                let (tag, tag_style) = if cp.step_index == active_step {
                    (
                        "[HEAD]  ",
                        row_bg.fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    )
                } else if cp.step_index > active_step {
                    (
                        "[AHEAD] ",
                        row_bg.fg(Color::Magenta).add_modifier(Modifier::BOLD),
                    )
                } else {
                    ("        ", row_bg.fg(Color::DarkGray))
                };

                let step_style = if is_selected {
                    row_bg.fg(Color::White).add_modifier(Modifier::BOLD)
                } else if cp.step_index == active_step {
                    row_bg.fg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else {
                    row_bg.fg(Color::White)
                };

                let prompt_text = cp.prompt.as_deref().unwrap_or("Baseline snapshot");
                let time_str = cp.timestamp.format("%H:%M:%S").to_string();

                let line = Line::from(vec![
                    Span::styled(cursor, row_bg.fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("#{:02} ", cp.step_index), step_style),
                    Span::styled(format!("{} ", time_str), row_bg.fg(Color::DarkGray)),
                    Span::styled(tag, tag_style),
                    Span::styled(
                        format!("{:>2}f  ", cp.deltas.len()),
                        row_bg.fg(Color::DarkGray),
                    ),
                    Span::styled(prompt_text, step_style),
                ]);
                items.push(ListItem::new(line));
            }
        }
    }

    let total = app.rows.len();
    let current = if total == 0 {
        0
    } else {
        app.selected_index + 1
    };
    let title = format!(" TIMELINE [{}/{}] ", current, total);

    let list = List::new(items).block(
        Block::default()
            .title(Span::styled(
                title,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    let mut state = ListState::default().with_selected(Some(app.selected_index));
    f.render_stateful_widget(list, area, &mut state);
}

fn render_details(f: &mut Frame, area: Rect, app: &TimelineApp) {
    let mut lines = Vec::new();
    let label_style = Style::default().fg(Color::DarkGray);

    match app.selected_row() {
        Some(TimelineRow::Live { deltas }) => {
            lines.push(Line::from(vec![
                Span::styled("State     : ", label_style),
                Span::styled(
                    "WORKTREE (uncommitted changes)",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Base Step : ", label_style),
                Span::styled(
                    format!("#{:02} ({})", app.tx.active_step(), app.tx.active_branch),
                    Style::default().fg(Color::Cyan),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Action    : ", label_style),
                Span::raw("Press "),
                Span::styled(
                    "Enter",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" or "),
                Span::styled(
                    "u",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" to stash & restore HEAD"),
            ]));

            lines.push(Line::from(""));
            render_delta_section(&mut lines, deltas, app.delta_scroll);
        }
        Some(TimelineRow::Saved(cp)) => {
            let active_step = app.tx.active_step();
            let (status_badge, status_style) = if cp.step_index == active_step {
                (
                    "HEAD (active workspace base)",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
            } else if cp.step_index > active_step {
                (
                    "AHEAD (fast-forward target)",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                (
                    "HISTORY (rewind target)",
                    Style::default().fg(Color::Yellow),
                )
            };

            let parent_str = match cp.parent_step {
                Some(p) => format!("#{:02}", p),
                None => "root".to_string(),
            };
            let short_hash = cp
                .content_hash
                .as_deref()
                .map(|h| if h.len() > 12 { &h[..12] } else { h })
                .unwrap_or("-");

            lines.push(Line::from(vec![
                Span::styled("Step      : ", label_style),
                Span::styled(
                    format!("#{:02}  ", cp.step_index),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(status_badge, status_style),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Branch    : ", label_style),
                Span::styled(cp.branch.clone(), Style::default().fg(Color::Green)),
                Span::styled("  │  parent: ", label_style),
                Span::styled(parent_str, Style::default().fg(Color::White)),
                Span::styled("  │  hash: ", label_style),
                Span::styled(short_hash.to_string(), Style::default().fg(Color::DarkGray)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Timestamp : ", label_style),
                Span::styled(
                    cp.timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
                    Style::default().fg(Color::White),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Message   : ", label_style),
                Span::styled(
                    cp.prompt.as_deref().unwrap_or("Baseline snapshot"),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));

            lines.push(Line::from(""));
            render_delta_section(&mut lines, &cp.deltas, app.delta_scroll);
        }
        None => {
            lines.push(Line::from(Span::styled(
                "No checkpoint selected.",
                label_style,
            )));
        }
    }

    let p = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .title(Span::styled(
                " INSPECTOR ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    f.render_widget(p, area);
}

fn render_delta_section<'a>(
    lines: &mut Vec<Line<'a>>,
    deltas: &'a [savepoint_core::FileDelta],
    scroll: u16,
) {
    let mut created = 0usize;
    let mut modified = 0usize;
    let mut deleted = 0usize;
    for d in deltas {
        match d.kind {
            ChangeKind::Created => created += 1,
            ChangeKind::Modified => modified += 1,
            ChangeKind::Deleted => deleted += 1,
        }
    }

    let scroll_hint = if scroll > 0 {
        format!("  [scroll +{}]", scroll)
    } else {
        String::new()
    };

    lines.push(Line::from(vec![
        Span::styled(
            format!("CHANGES ({} files)", deltas.len()),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("   +{}", created),
            Style::default().fg(Color::Green),
        ),
        Span::styled(
            format!("  ~{}", modified),
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(format!("  -{}", deleted), Style::default().fg(Color::Red)),
        Span::styled(scroll_hint, Style::default().fg(Color::Cyan)),
    ]));

    if deltas.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No file modifications at this checkpoint.",
            Style::default().fg(Color::DarkGray),
        )));
        return;
    }

    let skip = (scroll as usize).min(deltas.len().saturating_sub(1));
    for delta in deltas.iter().skip(skip) {
        let (code, color) = match delta.kind {
            ChangeKind::Created => ("A", Color::Green),
            ChangeKind::Modified => ("M", Color::Yellow),
            ChangeKind::Deleted => ("D", Color::Red),
        };

        let size_info = format_size_delta(delta.old_size, delta.new_size);

        lines.push(Line::from(vec![
            Span::styled(
                format!("  {}  ", code),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                delta.relative_path.display().to_string(),
                Style::default().fg(Color::White),
            ),
            Span::styled(size_info, Style::default().fg(Color::DarkGray)),
        ]));
    }
}

fn format_size_delta(old_size: Option<u64>, new_size: Option<u64>) -> String {
    match (old_size, new_size) {
        (None, Some(n)) => format!("  ({})", format_bytes(n)),
        (Some(o), Some(n)) if o != n => format!("  ({} -> {})", format_bytes(o), format_bytes(n)),
        (Some(o), Some(_)) => format!("  ({})", format_bytes(o)),
        (Some(o), None) => format!("  (-{})", format_bytes(o)),
        (None, None) => String::new(),
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn render_confirm_modal(f: &mut Frame, target_step: usize, active_step: usize, has_live: bool) {
    let area = centered_rect(60, 30, f.area());
    f.render_widget(Clear, area);

    let (title, color, action_verb) = if target_step > active_step {
        (" CONFIRM FAST-FORWARD ", Color::Magenta, "Fast-forward")
    } else {
        (" CONFIRM RESTORE ", Color::Yellow, "Restore")
    };

    let safety_note = if has_live {
        "Uncommitted worktree changes will be auto-saved as a checkpoint before switching."
    } else {
        "All checkpoints remain preserved in the transaction timeline."
    };

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw(format!("  {} workspace to ", action_verb)),
            Span::styled(
                format!("Step #{:02}", target_step),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("?"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", safety_note),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                " Enter / y ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Confirm      "),
            Span::styled(
                " Esc / n ",
                Style::default()
                    .fg(Color::White)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Cancel"),
        ]),
    ];

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color));

    let p = Paragraph::new(text).block(block);
    f.render_widget(p, area);
}

fn render_footer(f: &mut Frame, area: Rect) {
    let key_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let sep_style = Style::default().fg(Color::DarkGray);

    let footer_text = Line::from(vec![
        Span::styled(" j/k ", key_style),
        Span::raw("move"),
        Span::styled("  │  ", sep_style),
        Span::styled("g/G ", key_style),
        Span::raw("top/bottom"),
        Span::styled("  │  ", sep_style),
        Span::styled("J/K ", key_style),
        Span::raw("scroll files"),
        Span::styled("  │  ", sep_style),
        Span::styled(
            "Enter/r ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("restore"),
        Span::styled("  │  ", sep_style),
        Span::styled(
            "u ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("undo"),
        Span::styled("  │  ", sep_style),
        Span::styled(
            "c ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("commit"),
        Span::styled("  │  ", sep_style),
        Span::styled(
            "q/Esc ",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Span::raw("quit"),
    ]);

    let footer = Paragraph::new(footer_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    f.render_widget(footer, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
