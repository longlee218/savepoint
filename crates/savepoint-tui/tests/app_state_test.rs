use savepoint_core::{Checkpoint, Transaction, TransactionId};
use savepoint_tui::{TimelineAction, TimelineApp};
use std::path::PathBuf;

#[test]
fn test_app_navigation_and_actions() {
    let mut tx = Transaction::new(
        TransactionId::new(),
        PathBuf::from("/tmp/ws"),
        PathBuf::from("/tmp/shadow"),
    );

    for i in 0..3 {
        tx.checkpoints.push(Checkpoint {
            step_index: i,
            timestamp: chrono::Utc::now(),
            snapshot_path: PathBuf::from(format!("/tmp/cp_{}", i)),
            prompt: Some(format!("Step {}", i)),
            summary: None,
            deltas: Vec::new(),
        });
    }

    let mut app = TimelineApp::new(tx);
    // Initially selects latest checkpoint (index 2)
    assert_eq!(app.selected_index, 2);

    // Navigate up
    app.previous();
    assert_eq!(app.selected_index, 1);
    app.previous();
    assert_eq!(app.selected_index, 0);
    // Bounds check
    app.previous();
    assert_eq!(app.selected_index, 0);

    // Navigate down
    app.next();
    assert_eq!(app.selected_index, 1);

    // Request Rewind
    app.request_rewind();
    assert!(app.should_quit);
    assert_eq!(app.action, Some(TimelineAction::Rewind(1)));
}
