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
            Constraint::Min(10),   // Main Body (Left: Steps list, Right: Step Details)
            Constraint::Length(3), // Action bar
        ])
        .split(f.area());

    render_header(f, chunks[0], app);
    render_body(f, chunks[1], app);
    render_footer(f, chunks[2]);
}

fn render_header(f: &mut Frame, area: Rect, app: &TimelineApp) {
    let ws_name = app
        .tx
        .workspace_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "Workspace".to_string());

    let header_line = Line::from(vec![
        Span::styled(" ⏱ TIME MACHINE ", Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(format!("  Dự án: {}  •  Mã phiên: {}  ", ws_name, app.tx.id)),
    ]);

    let p = Paragraph::new(header_line).block(
        Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::Cyan)),
    );
    f.render_widget(p, area);
}

fn render_body(f: &mut Frame, area: Rect, app: &TimelineApp) {
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(45), // Danh sách các mốc
            Constraint::Percentage(55), // Chi tiết mốc đang chọn
        ])
        .split(area);

    render_steps_list(f, body_chunks[0], app);
    render_details(f, body_chunks[1], app);
}

fn render_steps_list(f: &mut Frame, area: Rect, app: &TimelineApp) {
    let mut items = Vec::new();
    let total_checkpoints = app.tx.checkpoints.len();

    // Render from newest (top) to oldest (bottom) or step order
    for (idx, cp) in app.tx.checkpoints.iter().enumerate() {
        let is_selected = idx == app.selected_index;
        let is_latest = idx == total_checkpoints.saturating_sub(1);

        let icon = if is_selected {
            "👉 "
        } else if is_latest {
            "● "
        } else {
            "○ "
        };

        let badge = if is_latest {
            " [Đang ở đây] "
        } else {
            " "
        };

        let step_title = format!("{}Bước #{}{}", icon, cp.step_index, badge);
        let prompt_desc = cp.prompt.as_deref().unwrap_or("Mốc khởi đầu");

        let style = if is_selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else if is_latest {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::White)
        };

        let line = Line::from(vec![
            Span::styled(step_title, style),
            Span::styled(format!("({} files) ", cp.deltas.len()), Style::default().fg(Color::DarkGray)),
            Span::styled(prompt_desc, style),
        ]);

        items.push(ListItem::new(line));
    }

    let list = List::new(items).block(
        Block::default()
            .title(" 📜 Lịch Sử Các Bước (Dùng ↑ / ↓ để chọn) ")
            .borders(Borders::ALL)
            .style(Style::default().fg(Color::White)),
    );

    f.render_widget(list, area);
}

fn render_details(f: &mut Frame, area: Rect, app: &TimelineApp) {
    let mut lines = Vec::new();

    if let Some(cp) = app.selected_checkpoint() {
        lines.push(Line::from(vec![
            Span::styled("Mốc thời gian: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("Bước #{} (Lúc {})", cp.step_index, cp.timestamp.format("%H:%M:%S")),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
        ]));

        lines.push(Line::from(vec![
            Span::styled("Mô tả: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                cp.prompt.as_deref().unwrap_or("Trạng thái ban đầu"),
                Style::default().fg(Color::Yellow),
            ),
        ]));

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                "Các file thay đổi tại bước này:",
                Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ),
        ]));

        if cp.deltas.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (Bước khởi đầu - chưa có file nào bị thay đổi)",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            for delta in &cp.deltas {
                let (badge, color, action_text) = match delta.kind {
                    ChangeKind::Created => ("+ [TẠO MỚI]", Color::Green, "Tạo mới"),
                    ChangeKind::Modified => ("~ [SỬA ĐỔI]", Color::Yellow, "Sửa nội dung"),
                    ChangeKind::Deleted => ("- [ĐÃ XÓA]", Color::Red, "Bị xóa mất"),
                };

                lines.push(Line::from(vec![
                    Span::styled(format!("  {} ", badge), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                    Span::raw(format!("{}  ({})", delta.relative_path.display(), action_text)),
                ]));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("💡 Bấm phím ", Style::default().fg(Color::DarkGray)),
            Span::styled("[Enter]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(" để đưa toàn bộ dự án quay về Bước #{}", cp.step_index),
                Style::default().fg(Color::Green),
            ),
        ]));
    } else {
        lines.push(Line::from(Span::raw("Chưa chọn bước nào")));
    }

    let p = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(" 🔍 Chi Tiết Bước Đang Chọn ")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::White)),
        );

    f.render_widget(p, area);
}

fn render_footer(f: &mut Frame, area: Rect) {
    let footer_text = Line::from(vec![
        Span::styled(" [↑ / ↓] ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("Chọn bước   "),
        Span::styled(" [Enter] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::raw("Quay về bước này   "),
        Span::styled(" [c] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::raw("Chốt lưu (Commit)   "),
        Span::styled(" [q / Esc] ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Span::raw("Đóng menu"),
    ]);

    let footer = Paragraph::new(footer_text)
        .block(Block::default().borders(Borders::ALL).style(Style::default().fg(Color::DarkGray)));

    f.render_widget(footer, area);
}
