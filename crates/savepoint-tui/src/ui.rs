use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};
use savepoint_core::ChangeKind;
use crate::app::TimelineApp;

pub fn render_ui(f: &mut Frame, app: &TimelineApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(8),    // Body (Left: Timeline, Right: Inspector)
            Constraint::Length(3), // Footer
        ])
        .split(f.area());

    render_header(f, chunks[0], app);
    render_body(f, chunks[1], app);
    render_footer(f, chunks[2]);
}

fn render_header(f: &mut Frame, area: Rect, app: &TimelineApp) {
    let header_text = vec![
        Line::from(vec![
            Span::styled(" ⚡ SAVEPOINT ", Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw(" // Interactive Time-Travel & ACID Runtime   "),
            Span::styled("Tx: ", Style::default().fg(Color::DarkGray)),
            Span::styled(app.tx.id.as_str(), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::styled(" | Status: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:?}", app.tx.status), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
    ];

    let header_widget = Paragraph::new(header_text)
        .block(Block::default().borders(Borders::ALL).style(Style::default().fg(Color::Cyan)));

    f.render_widget(header_widget, area);
}

fn render_body(f: &mut Frame, area: Rect, app: &TimelineApp) {
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(42), // Timeline Checkpoint List
            Constraint::Percentage(58), // Inspector & File Deltas
        ])
        .split(area);

    render_timeline_list(f, body_chunks[0], app);
    render_inspector(f, body_chunks[1], app);
}

fn render_timeline_list(f: &mut Frame, area: Rect, app: &TimelineApp) {
    let mut items = Vec::new();

    for (idx, cp) in app.tx.checkpoints.iter().enumerate() {
        let is_selected = idx == app.selected_index;
        let prefix = if is_selected { "▶ " } else { "  " };

        let step_label = format!("{}Step #{} ", prefix, cp.step_index);
        let prompt_desc = cp.prompt.as_deref().unwrap_or("Baseline snapshot");

        let style = if is_selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let line = Line::from(vec![
            Span::styled(step_label, style.add_modifier(Modifier::BOLD)),
            Span::styled(format!("({} changes) ", cp.deltas.len()), Style::default().fg(Color::DarkGray)),
            Span::styled(prompt_desc, style),
        ]);

        items.push(ListItem::new(line));
    }

    let list = List::new(items).block(
        Block::default()
            .title(" ⏱ Checkpoint Timeline ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::White)),
    );

    f.render_widget(list, area);
}

fn render_inspector(f: &mut Frame, area: Rect, app: &TimelineApp) {
    let mut lines = Vec::new();

    if let Some(cp) = app.selected_checkpoint() {
        lines.push(Line::from(vec![
            Span::styled("Selected: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("Checkpoint Step #{}", cp.step_index), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled(format!("  ({})", cp.timestamp.format("%H:%M:%S UTC")), Style::default().fg(Color::DarkGray)),
        ]));

        lines.push(Line::from(vec![
            Span::styled("Prompt: ", Style::default().fg(Color::DarkGray)),
            Span::styled(cp.prompt.as_deref().unwrap_or("None"), Style::default().fg(Color::Yellow)),
        ]));

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("File Deltas (Blast Radius):", Style::default().add_modifier(Modifier::UNDERLINED | Modifier::BOLD)),
        ]));

        if cp.deltas.is_empty() {
            lines.push(Line::from(Span::styled("  (No file changes in this step - pristine baseline)", Style::default().fg(Color::DarkGray))));
        } else {
            for delta in &cp.deltas {
                let (badge, badge_color) = match delta.kind {
                    ChangeKind::Created => ("+ [Created]", Color::Green),
                    ChangeKind::Modified => ("~ [Modified]", Color::Yellow),
                    ChangeKind::Deleted => ("- [Deleted]", Color::Red),
                };

                lines.push(Line::from(vec![
                    Span::styled(format!("  {} ", badge), Style::default().fg(badge_color).add_modifier(Modifier::BOLD)),
                    Span::raw(delta.relative_path.to_string_lossy().to_string()),
                ]));
            }
        }
    } else {
        lines.push(Line::from(Span::raw("No checkpoint selected")));
    }

    let inspector = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(" 🔍 Step Inspector & Deltas ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::White)),
        );

    f.render_widget(inspector, area);
}

fn render_footer(f: &mut Frame, area: Rect) {
    let footer_text = Line::from(vec![
        Span::styled(" [↑/k / ↓/j] ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("Navigate  "),
        Span::styled(" [Enter] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::raw("Rewind to Step  "),
        Span::styled(" [c] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::raw("Commit  "),
        Span::styled(" [q / Esc] ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Span::raw("Cancel/Exit"),
    ]);

    let footer = Paragraph::new(footer_text)
        .block(Block::default().borders(Borders::ALL).style(Style::default().fg(Color::DarkGray)));

    f.render_widget(footer, area);
}
