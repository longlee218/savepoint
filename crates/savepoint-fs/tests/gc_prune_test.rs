use savepoint_fs::{SavepointIgnore, ShadowEngine};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_is_ignored_unconditionally_skips_heavy_dependencies_and_agent_internals() {
    let ws = tempdir().unwrap();
    let ignore = SavepointIgnore::for_workspace(ws.path());

    // Without any .gitignore, internal agent directories and heavy build caches must be ignored unconditionally!
    assert!(ignore.is_ignored(&ws.path().join(".claude"), true));
    assert!(ignore.is_ignored(&ws.path().join(".git"), true));
    assert!(ignore.is_ignored(&ws.path().join(".savepoint"), true));
    assert!(ignore.is_ignored(&ws.path().join(".turbo"), true));
    assert!(ignore.is_ignored(&ws.path().join("dist"), true));
    assert!(ignore.is_ignored(&ws.path().join("build"), true));
    assert!(ignore.is_ignored(&ws.path().join(".next"), true));
    assert!(ignore.is_ignored(&ws.path().join(".output"), true));

    // Regular source files and folders must NOT be ignored
    assert!(!ignore.is_ignored(&ws.path().join("src"), true));
    assert!(!ignore.is_ignored(&ws.path().join("src/main.rs"), false));
}

#[test]
fn test_temp_workspace_isolated_from_home_savepoint() {
    let ws = tempdir().unwrap();
    let storage = ShadowEngine::resolve_storage_dir(ws.path());

    // Must be isolated inside a temp spool, never writing to ~/.savepoint
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() {
        let home_savepoint = std::path::PathBuf::from(home).join(".savepoint");
        assert!(
            !storage.starts_with(&home_savepoint),
            "Temp workspace storage {:?} must NOT be inside {:?}",
            storage,
            home_savepoint
        );
    }
}

#[test]
fn test_prune_old_transactions_and_auto_retention() {
    let ws = tempdir().unwrap();
    fs::write(ws.path().join("file.txt"), "v1").unwrap();

    // Create 4 transactions, committing each
    for i in 0..4 {
        fs::write(ws.path().join("file.txt"), format!("v{}", i)).unwrap();
        let mut tx = ShadowEngine::begin_transaction(ws.path()).unwrap();
        ShadowEngine::commit(&mut tx).unwrap();
    }

    // Now begin a 5th active transaction
    let _active_tx = ShadowEngine::begin_transaction(ws.path()).unwrap();

    let ws_storage = ShadowEngine::resolve_storage_dir(ws.path());
    let tx_root = ws_storage.join("transactions");
    let tx_dirs: Vec<_> = fs::read_dir(&tx_root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();

    // Active transaction + at most 2 historical committed transactions = at most 3 directories!
    assert!(
        tx_dirs.len() <= 3,
        "Auto-retention must bound historical transactions! Found: {:?}",
        tx_dirs
    );

    // Explicitly prune down to 0 historical transactions (active only)
    let pruned = ShadowEngine::prune_old_transactions(ws.path(), 0).unwrap();
    assert!(pruned > 0);

    let tx_dirs_after: Vec<_> = fs::read_dir(&tx_root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    // Exactly 1 (the active transaction)
    assert_eq!(tx_dirs_after.len(), 1);
}

#[test]
fn test_gc_orphaned_workspaces_sweeps_deleted_repos() {
    let temp_spool = tempdir().unwrap();
    let spool_workspaces = temp_spool.path().join("workspaces");
    let ws_orphan_dir = spool_workspaces.join("orphan_ws_hash");
    let tx_orphan_dir = ws_orphan_dir.join("transactions").join("tx_dead");
    fs::create_dir_all(&tx_orphan_dir).unwrap();

    // Write a meta.json pointing to a non-existent deleted workspace root
    let dead_meta = serde_json::json!({
        "id": "tx_dead",
        "workspace_root": "/tmp/non_existent_path_for_unit_test_99999",
        "status": "Committed"
    });
    fs::write(tx_orphan_dir.join("meta.json"), dead_meta.to_string()).unwrap();

    // Run GC on the test spool
    let (orphaned, _) = ShadowEngine::gc_orphaned_workspaces_in_dir(&spool_workspaces).unwrap();
    assert_eq!(orphaned, 1, "GC must sweep the orphaned workspace directory");
    assert!(!ws_orphan_dir.exists(), "Orphaned directory must be purged");
}
