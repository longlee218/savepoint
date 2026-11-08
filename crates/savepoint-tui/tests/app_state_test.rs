use ratatui::{backend::TestBackend, Terminal};
use savepoint_core::{ChangeKind, Checkpoint, FileDelta, Transaction, TransactionId};
use savepoint_tui::app::AppTab;
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
fn test_tab_navigation_and_cycling() {
    let tx = sample_transaction(2);
    let mut app = TimelineApp::new(tx, Vec::new());

    assert_eq!(app.active_tab, AppTab::Timeline);

    // Test next_tab cycle through all 7 tabs
    app.next_tab();
    assert_eq!(app.active_tab, AppTab::DagGraph);
    app.next_tab();
    assert_eq!(app.active_tab, AppTab::StateProviders);
    app.next_tab();
    assert_eq!(app.active_tab, AppTab::AgentLeases);
    app.next_tab();
    assert_eq!(app.active_tab, AppTab::EgressSagas);
    app.next_tab();
    assert_eq!(app.active_tab, AppTab::CasStorage);
    app.next_tab();
    assert_eq!(app.active_tab, AppTab::Benchmarks);
    app.next_tab();
    assert_eq!(app.active_tab, AppTab::Timeline); // Cycles back!

    // Test prev_tab cycle
    app.prev_tab();
    assert_eq!(app.active_tab, AppTab::Benchmarks);
    app.prev_tab();
    assert_eq!(app.active_tab, AppTab::CasStorage);

    // Test direct set_tab
    app.set_tab(AppTab::StateProviders);
    assert_eq!(app.active_tab, AppTab::StateProviders);
}

#[test]
fn test_all_seven_tabs_render_properly() {
    let tx = sample_transaction(3);
    let mut app = TimelineApp::new(tx, Vec::new());

    let backend = TestBackend::new(120, 35);
    let mut terminal = Terminal::new(backend).unwrap();

    // Check Tab 1: Timeline
    app.set_tab(AppTab::Timeline);
    terminal.draw(|f| render_ui(f, &app)).unwrap();
    let mut text = String::new();
    let buf = terminal.backend().buffer();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            text.push_str(buf[(x, y)].symbol());
        }
        text.push('\n');
    }
    assert!(text.contains("TIMELINE"));
    assert!(text.contains("INSPECTOR"));

    // Check Tab 2: DAG Graph
    app.set_tab(AppTab::DagGraph);
    terminal.draw(|f| render_ui(f, &app)).unwrap();
    let mut text = String::new();
    let buf = terminal.backend().buffer();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            text.push_str(buf[(x, y)].symbol());
        }
        text.push('\n');
    }
    assert!(text.contains("DAG TOPOLOGY"));
    assert!(text.contains("GRAPH ARCHITECTURE"));

    // Check Tab 3: State Providers
    app.set_tab(AppTab::StateProviders);
    terminal.draw(|f| render_ui(f, &app)).unwrap();
    let mut text = String::new();
    let buf = terminal.backend().buffer();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            text.push_str(buf[(x, y)].symbol());
        }
        text.push('\n');
    }
    assert!(text.contains("EXTERNAL STATE HYPERVISOR"));
    assert!(text.contains("PostgreSQL"));

    // Check Tab 4: Agent Leases
    app.set_tab(AppTab::AgentLeases);
    terminal.draw(|f| render_ui(f, &app)).unwrap();
    let mut text = String::new();
    let buf = terminal.backend().buffer();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            text.push_str(buf[(x, y)].symbol());
        }
        text.push('\n');
    }
    assert!(text.contains("ACTIVE MULTI-AGENT VIRTUAL LEASES"));
    assert!(text.contains("port-offset"));

    // Check Tab 5: Egress & Sagas
    app.set_tab(AppTab::EgressSagas);
    terminal.draw(|f| render_ui(f, &app)).unwrap();
    let mut text = String::new();
    let buf = terminal.backend().buffer();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            text.push_str(buf[(x, y)].symbol());
        }
        text.push('\n');
    }
    assert!(text.contains("NETWORK HYPERVISOR & EGRESS CONTROL"));
    assert!(text.contains("SAGA COMPENSATION REGISTRY"));

    // Check Tab 6: Storage & CAS
    app.set_tab(AppTab::CasStorage);
    terminal.draw(|f| render_ui(f, &app)).unwrap();
    let mut text = String::new();
    let buf = terminal.backend().buffer();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            text.push_str(buf[(x, y)].symbol());
        }
        text.push('\n');
    }
    assert!(text.contains("STORAGE ARCHITECTURE"));
    assert!(text.contains("FastCDC"));

    // Check Tab 7: Benchmarks
    app.set_tab(AppTab::Benchmarks);
    terminal.draw(|f| render_ui(f, &app)).unwrap();
    let mut text = String::new();
    let buf = terminal.backend().buffer();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            text.push_str(buf[(x, y)].symbol());
        }
        text.push('\n');
    }
    assert!(text.contains("LIVE BENCHMARK METRICS"));
    assert!(text.contains("APFS CoW Workspace Clone"));
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

    let backend = TestBackend::new(120, 35);
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
