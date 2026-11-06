use savepoint_core::{Checkpoint, Transaction, TransactionId};
use savepoint_tui::{TimelineAction, TimelineApp};
use std::path::PathBuf;

#[test]
fn test_app_navigation_and_confirmation() {
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
    assert_eq!(app.selected_index, 2);

    // Navigate to step 1
    app.previous();
    assert_eq!(app.selected_index, 1);

    // Request Rewind confirmation
    app.request_rewind_confirmation();
    // It should NOT quit yet! It should be in confirmation mode
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
