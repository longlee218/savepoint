use chrono::Utc;
use savepoint_core::error::SavepointError;
use savepoint_core::types::{
    ChangeKind, Checkpoint, FileDelta, Transaction, TransactionId, TransactionStatus,
};
use std::collections::HashSet;
use std::path::PathBuf;

#[test]
fn test_transaction_id_generation_and_formatting() {
    let id1 = TransactionId::new();
    let id2 = TransactionId::default();
    assert_ne!(id1, id2);
    assert!(id1.as_str().starts_with("tx_"));
    assert_eq!(format!("{}", id1), id1.as_str());

    let custom = TransactionId::from_string("tx_custom_123");
    assert_eq!(custom.as_str(), "tx_custom_123");

    let mut set = HashSet::new();
    set.insert(id1.clone());
    assert!(set.contains(&id1));
}

#[test]
fn test_transaction_status_and_change_kind_roundtrip() {
    let statuses = vec![
        TransactionStatus::Active,
        TransactionStatus::Committed,
        TransactionStatus::RolledBack,
        TransactionStatus::Aborted,
    ];
    for status in statuses {
        let json = serde_json::to_string(&status).unwrap();
        let parsed: TransactionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, parsed);
    }

    let kinds = vec![
        ChangeKind::Created,
        ChangeKind::Modified,
        ChangeKind::Deleted,
    ];
    for kind in kinds {
        let json = serde_json::to_string(&kind).unwrap();
        let parsed: ChangeKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, parsed);
    }
}

#[test]
fn test_file_delta_properties() {
    let delta = FileDelta {
        relative_path: PathBuf::from("src/main.rs"),
        kind: ChangeKind::Modified,
        old_size: Some(1024),
        new_size: Some(2048),
    };
    let json = serde_json::to_string(&delta).unwrap();
    let parsed: FileDelta = serde_json::from_str(&json).unwrap();
    assert_eq!(delta, parsed);
}

#[test]
fn test_transaction_branch_lifecycle() {
    let mut tx = Transaction::new(
        TransactionId::from_string("tx_test"),
        PathBuf::from("/workspace"),
        PathBuf::from("/shadow"),
    );
    assert_eq!(tx.active_branch, "main");
    assert_eq!(tx.active_step(), 0);
    assert_eq!(tx.next_step_index(), 0);
    assert!(tx.active_checkpoint().is_none());
    assert!(tx.latest_checkpoint().is_none());

    // Record step 0 on main
    let cp0 = Checkpoint {
        step_index: 0,
        timestamp: Utc::now(),
        snapshot_path: PathBuf::from("/shadow/0"),
        prompt: Some("Init".to_string()),
        summary: None,
        deltas: Vec::new(),
        branch: "main".to_string(),
        parent_step: None,
        content_hash: None,
    };
    tx.record_checkpoint_in_branch(cp0);
    assert_eq!(tx.active_step(), 0);
    assert_eq!(tx.next_step_index(), 1);

    // Fork feature branch
    tx.fork_branch("feature_auth").unwrap();
    assert_eq!(tx.active_branch, "feature_auth");
    assert_eq!(tx.active_step(), 0);

    // Forking duplicate branch should error
    let err = tx.fork_branch("feature_auth").unwrap_err();
    match err {
        SavepointError::BranchAlreadyExists(b) => assert_eq!(b, "feature_auth"),
        _ => panic!("Expected BranchAlreadyExists"),
    }

    // Record step 1 on feature_auth
    let cp1 = Checkpoint {
        step_index: 1,
        timestamp: Utc::now(),
        snapshot_path: PathBuf::from("/shadow/1"),
        prompt: Some("Add JWT".to_string()),
        summary: None,
        deltas: Vec::new(),
        branch: "feature_auth".to_string(),
        parent_step: Some(0),
        content_hash: None,
    };
    tx.record_checkpoint_in_branch(cp1);
    assert_eq!(tx.active_step(), 1);
    assert_eq!(tx.next_step_index(), 2);

    // Switch back to main
    let main_step = tx.switch_branch("main").unwrap();
    assert_eq!(main_step, 0);
    assert_eq!(tx.active_step(), 0);

    // Switch to non-existent branch should error
    let err = tx.switch_branch("ghost").unwrap_err();
    match err {
        SavepointError::BranchNotFound(b) => assert_eq!(b, "ghost"),
        _ => panic!("Expected BranchNotFound"),
    }

    // List branches
    let branches = tx.branch_names();
    assert_eq!(branches, vec!["feature_auth", "main"]);

    // Head check
    let head = tx.branch_head("feature_auth").unwrap();
    assert_eq!(head.step_index, 1);
}

#[test]
fn test_savepoint_error_display_coverage() {
    let io_err = SavepointError::io(
        PathBuf::from("/tmp/missing"),
        std::io::Error::new(std::io::ErrorKind::NotFound, "not found"),
    );
    assert!(format!("{}", io_err).contains("/tmp/missing"));

    let errs = vec![
        SavepointError::CloneFileFailed {
            src: PathBuf::from("/a"),
            dst: PathBuf::from("/b"),
            errno: 17,
        },
        SavepointError::TransactionNotFound("tx_1".to_string()),
        SavepointError::TransactionFinished("tx_1".to_string(), TransactionStatus::Committed),
        SavepointError::CheckpointNotFound(5, "tx_1".to_string()),
        SavepointError::BranchNotFound("br_1".to_string()),
        SavepointError::BranchAlreadyExists("br_2".to_string()),
        SavepointError::SpeculationFailed("timeout".to_string()),
        SavepointError::InvalidWorkspace(PathBuf::from("/inv")),
        SavepointError::Other("custom".to_string()),
    ];

    for err in errs {
        let msg = format!("{}", err);
        assert!(!msg.is_empty());
    }
}
