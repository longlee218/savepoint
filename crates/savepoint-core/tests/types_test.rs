use savepoint_core::{
    ChangeKind, Checkpoint, FileDelta, Transaction, TransactionId, TransactionStatus,
};
use std::path::PathBuf;

#[test]
fn test_transaction_serialization_roundtrip() {
    let tx_id = TransactionId::new();
    assert!(tx_id.as_str().starts_with("tx_"));

    let ws = PathBuf::from("/tmp/test_ws");
    let shadow = PathBuf::from("/tmp/test_shadow");
    let mut tx = Transaction::new(tx_id.clone(), ws.clone(), shadow.clone());

    assert_eq!(tx.status, TransactionStatus::Active);
    assert_eq!(tx.current_step(), 0);

    let delta = FileDelta {
        relative_path: PathBuf::from("src/main.rs"),
        kind: ChangeKind::Modified,
        old_size: Some(100),
        new_size: Some(250),
    };

    let cp = Checkpoint {
        step_index: 1,
        timestamp: chrono::Utc::now(),
        snapshot_path: shadow.join("cp_1"),
        prompt: Some("Refactor authentication".to_string()),
        summary: Some("Updated JWT handler".to_string()),
        deltas: vec![delta],
        branch: "main".to_string(),
        parent_step: Some(0),
        content_hash: None,
    };

    tx.record_checkpoint_in_branch(cp);
    assert_eq!(tx.current_step(), 1);
    assert_eq!(tx.active_branch, "main");

    // Test Forking branch
    tx.fork_branch("experiment").expect("Must fork branch");
    assert_eq!(tx.active_branch, "experiment");
    assert_eq!(tx.branch_names(), vec!["experiment", "main"]);

    // Test JSON Serialization
    let json = serde_json::to_string_pretty(&tx).expect("Must serialize transaction");
    let deserialized: Transaction = serde_json::from_str(&json).expect("Must deserialize transaction");

    assert_eq!(deserialized.id, tx_id);
    assert_eq!(deserialized.checkpoints.len(), 1);
    assert_eq!(deserialized.checkpoints[0].deltas[0].kind, ChangeKind::Modified);
    assert_eq!(deserialized.active_branch, "experiment");
    assert_eq!(deserialized.branches.get("experiment"), Some(&1));
}
