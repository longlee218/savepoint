use ratatui::{backend::TestBackend, Terminal};
use savepoint_core::{ChangeKind, Checkpoint, FileDelta, Transaction, TransactionId};
use savepoint_tui::ui::render_ui;
use savepoint_tui::{TimelineAction, TimelineApp};
use std::path::PathBuf;

fn sample_transaction(steps: usize) -> Transaction {
    let mut tx = Transaction::new(
        TransactionId::new(),
        PathBuf::from("/tmp/savepoint-demo"),
        PathBuf::from("/tmp/shadow"),
    );

    for i in 0..steps {
        tx.checkpoints.push(Checkpoint {
            step_index: i,
            timestamp: chrono::Utc::now(),
            snapshot_path: PathBuf::from(format!("/tmp/cp_{}", i)),
            prompt: Some(format!("Step {} implementation", i)),
            summary: None,
            deltas: vec![FileDelta {
                relative_path: PathBuf::from(format!("src/module_{}.rs", i)),
                kind: if i == 0 {
                    ChangeKind::Created
                } else {
                    ChangeKind::Modified
                },
                old_size: if i == 0 { None } else { Some(128) },
                new_size: Some(256),
            }],
            branch: "main".to_string(),
            parent_step: if i > 0 { Some(i - 1) } else { None },
            content_hash: Some("abcdef0123456789abcdef0123456789".to_string()),
        });
    }
    tx.current_step_index = Some(steps.saturating_sub(1));
    tx
}

#[test]
fn test_app_navigation_and_confirmation() {
    let tx = sample_transaction(3);

    let mut app = TimelineApp::new(tx, Vec::new());
    assert_eq!(app.selected_index, 0);

    // Navigate down to step 1 (row 1)
    app.next();
    assert_eq!(app.selected_index, 1);

    // Jump to bottom (last) and top (first)
    app.last();
    assert_eq!(app.selected_index, 2);
    app.first();
    assert_eq!(app.selected_index, 0);

    // Move back to row 1 and test detail scrolling reset on row change
    app.next();
    app.scroll_details_down();
    assert!(app.delta_scroll > 0);
    app.scroll_details_up();
    assert_eq!(app.delta_scroll, 0);
    app.scroll_details_down();
    app.previous();
    assert_eq!(app.delta_scroll, 0);

    // Navigate down to step 1 (row 1) and request Rewind confirmation
    app.next();
    app.request_rewind_confirmation();
    assert!(!app.should_quit);
    assert_eq!(app.action, None);

    // If user cancels confirmation
    app.cancel_confirmation();
    assert!(!app.should_quit);

    // Request again and confirm
    app.request_rewind_confirmation();
    app.confirm_rewind();
    assert!(app.should_quit);
    assert_eq!(app.action, Some(TimelineAction::Rewind(1)));
}

#[test]
fn test_quick_undo_request() {
    let tx = sample_transaction(3);
    let mut app = TimelineApp::new(tx, Vec::new());

    // Active step is #2; quick undo should target #1
    app.request_undo();
    app.confirm_rewind();
    assert!(app.should_quit);
    assert_eq!(app.action, Some(TimelineAction::Rewind(1)));
}

#[test]
fn test_ui_renders_clean_english_without_vietnamese_or_emoji_slop() {
    let tx = sample_transaction(4);
    let live_deltas = vec![FileDelta {
        relative_path: PathBuf::from("src/dirty.rs"),
        kind: ChangeKind::Modified,
        old_size: Some(100),
        new_size: Some(220),
    }];
    let app = TimelineApp::new(tx, live_deltas);

    let backend = TestBackend::new(110, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| render_ui(f, &app)).unwrap();

    let buffer = terminal.backend().buffer();
    let mut rendered = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            rendered.push_str(buffer[(x, y)].symbol());
        }
        rendered.push('\n');
    }

    // Must contain clean English headers & labels
    assert!(rendered.contains("SAVEPOINT"), "Missing SAVEPOINT header");
    assert!(rendered.contains("TIMELINE"), "Missing TIMELINE pane title");
    assert!(
        rendered.contains("INSPECTOR"),
        "Missing INSPECTOR pane title"
    );
    assert!(rendered.contains("WORKTREE"), "Missing WORKTREE row");
    assert!(rendered.contains("[HEAD]"), "Missing [HEAD] badge");

    // Must NOT contain Vietnamese words or emoji slop
    for forbidden in [
        "Dự án",
        "Mã phiên",
        "Lịch Sử",
        "Chi Tiết",
        "ĐANG Ở ĐÂY",
        "Tương lai",
        "Quá khứ",
        "Bước",
        "⏱",
        "👉",
        "⚡",
        "📜",
        "🔍",
        "💡",
        "⏩",
        "⏪",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "Rendered UI still contains forbidden string: {}",
            forbidden
        );
    }
}
