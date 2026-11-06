use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};
use savepoint_core::ChangeKind;
use crate::app::{TimelineApp, TimelineRow, ViewMode};

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

    // If confirmation modal is open, render it centered on top
    if let ViewMode::ConfirmingRewind(step) = app.mode {
        render_confirm_modal(f, step, app.tx.active_step());
    }
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

    for (idx, row) in app.rows.iter().enumerate() {
        let is_selected = idx == app.selected_index;
        let icon = if is_selected { "👉 " } else { "   " };

        match row {
            TimelineRow::Live { deltas } => {
                let style = if is_selected {
                    Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                };

                let line = Line::from(vec![
                    Span::styled(format!("{}⚡ [HIỆN TẠI - Chưa lưu] ", icon), style),
                    Span::styled(format!("({} files) ", deltas.len()), Style::default().fg(Color::Red)),
                ]);
                items.push(ListItem::new(line));
            }
            TimelineRow::Saved(cp) => {
                let style = if is_selected {
                    Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let active_step = app.tx.active_step();
                let (badge, badge_style) = if cp.step_index == active_step {
                    (" [ĐANG Ở ĐÂY] ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
                } else if cp.step_index > active_step {
                    (" [Tương lai ⏩] ", Style::default().fg(Color::Magenta))
                } else {
                    (" [Quá khứ ⏪] ", Style::default().fg(Color::DarkGray))
                };

                let step_title = format!("{}Bước #{} ", icon, cp.step_index);
                let prompt_desc = cp.prompt.as_deref().unwrap_or("Mốc khởi đầu");

                let line = Line::from(vec![
                    Span::styled(step_title, style),
                    Span::styled(badge, badge_style),
                    Span::styled(format!("({} files) ", cp.deltas.len()), Style::default().fg(Color::DarkGray)),
                    Span::styled(prompt_desc, style),
                ]);
                items.push(ListItem::new(line));
            }
        }
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

    match app.selected_row() {
        Some(TimelineRow::Live { deltas }) => {
            lines.push(Line::from(vec![
                Span::styled("Trạng thái: ", Style::default().fg(Color::DarkGray)),
                Span::styled("Thay đổi trực tiếp trong thư mục (Chưa lưu thành mốc)", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            ]));

            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    "Các file vừa bị tác động gần đây nhất:",
                    Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                ),
            ]));

            for delta in deltas {
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

            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("💡 Mẹo: ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw("Để hủy các thay đổi này và quay về mốc trước, hãy bấm phím "),
                Span::styled("↓", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::raw(" để chọn bước trước đó rồi bấm "),
                Span::styled("[Enter]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                Span::raw("!"),
            ]));
        }
        Some(TimelineRow::Saved(cp)) => {
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
                    "  (Bước khởi đầu - mốc an toàn nguyên bản)",
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

            let active_step = app.tx.active_step();
            lines.push(Line::from(""));
            if cp.step_index == active_step {
                lines.push(Line::from(vec![
                    Span::styled("✔ ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::styled("Thư mục dự án đang ở chính xác mốc này.", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                ]));
            } else if cp.step_index > active_step {
                lines.push(Line::from(vec![
                    Span::styled("💡 Bấm phím ", Style::default().fg(Color::DarkGray)),
                    Span::styled("[r]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::raw(" hoặc "),
                    Span::styled("[Enter]", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                    Span::styled(
                        format!(" để TUA TỚI (Redo ⏩) Bước #{}", cp.step_index),
                        Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                    ),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled("💡 Bấm phím ", Style::default().fg(Color::DarkGray)),
                    Span::styled("[r]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::raw(" hoặc "),
                    Span::styled("[Enter]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::styled(
                        format!(" để TUA NGƯỢC (Undo ⏪) về Bước #{}", cp.step_index),
                        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                    ),
                ]));
            }
        }
        None => {
            lines.push(Line::from(Span::raw("Chưa chọn bước nào")));
        }
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

fn render_confirm_modal(f: &mut Frame, target_step: usize, active_step: usize) {
    let area = centered_rect(68, 32, f.area());
    f.render_widget(Clear, area);

    let (title, color, prompt_action, note) = if target_step > active_step {
        (
            "  ⏩  XÁC NHẬN TUA TỚI TƯƠNG LAI (REDO / FAST-FORWARD)  ",
            Color::Magenta,
            "TUA TỚI (Redo)",
            "  (Savepoint sẽ khôi phục lại mã nguồn của mốc tương lai này trong 1ms!)",
        )
    } else {
        (
            "  ⏪  XÁC NHẬN TUA NGƯỢC THỜI GIAN (UNDO / REWIND)  ",
            Color::Yellow,
            "TUA NGƯỢC (Undo)",
            "  (Mọi thay đổi chưa lưu sẽ được tự động sao lưu an toàn trước khi tua!)",
        )
    };

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                title,
                Style::default().fg(Color::Black).bg(color).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw(format!("  Bạn có chắc chắn muốn {} toàn bộ dự án về ", prompt_action)),
            Span::styled(format!("Bước #{}", target_step), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw("?"),
        ]),
        Line::from(""),
        Line::from(Span::styled(note, Style::default().fg(Color::Green))),
        Line::from(""),
        Line::from(vec![
            Span::styled("  👉 [y / Enter] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw("Đồng ý chuyển mốc        "),
            Span::styled("  [n / Esc] ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            Span::raw("Hủy bỏ"),
        ]),
    ];

    let block = Block::default()
        .title(" Điều Hướng Thời Gian ")
        .borders(Borders::ALL)
        .style(Style::default().fg(color));

    let p = Paragraph::new(text).block(block);
    f.render_widget(p, area);
}

fn render_footer(f: &mut Frame, area: Rect) {
    let footer_text = Line::from(vec![
        Span::styled(" [↑ / ↓] ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw("Chọn bước   "),
        Span::styled(" [r / Enter] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::raw("Tua thời gian (Undo/Redo)   "),
        Span::styled(" [c] ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::raw("Chốt lưu (Commit)   "),
        Span::styled(" [q / Esc] ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Span::raw("Đóng menu"),
    ]);

    let footer = Paragraph::new(footer_text)
        .block(Block::default().borders(Borders::ALL).style(Style::default().fg(Color::DarkGray)));

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
