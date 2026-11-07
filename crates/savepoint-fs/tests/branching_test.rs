use savepoint_core::TransactionStatus;
use savepoint_fs::ShadowEngine;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_branching_and_merging_lifecycle() {
    let ws_dir = tempdir().expect("tempdir");
    let ws_path = ws_dir.path();
    let file1 = ws_path.join("main.txt");
    fs::write(&file1, "version 1").expect("write file1");

    // 1. Begin transaction on main branch
    let mut tx = ShadowEngine::begin_transaction(ws_path).expect("begin_tx");
    assert_eq!(tx.active_branch, "main");
    assert_eq!(tx.active_step(), 0);

    // 2. Make change on main and checkpoint
    fs::write(&file1, "version 2 on main").expect("write file1 v2");
    let cp1 = ShadowEngine::checkpoint(&mut tx, Some("main edit"), None).expect("cp1");
    assert_eq!(cp1.branch, "main");
    assert_eq!(tx.active_step(), 1);

    // 3. Fork a new branch 'speculative_ai'
    ShadowEngine::fork_branch(&mut tx, "speculative_ai").expect("fork branch");
    assert_eq!(tx.active_branch, "speculative_ai");
    assert_eq!(tx.branch_names(), vec!["main", "speculative_ai"]);

    // 4. Make experimental edits in speculative_ai branch
    let exp_file = ws_path.join("experiment.rs");
    fs::write(&exp_file, "fn innovate() {}").expect("write experiment");
    let cp2 = ShadowEngine::checkpoint(&mut tx, Some("AI prototype"), None).expect("cp2");
    assert_eq!(cp2.branch, "speculative_ai");
    assert_eq!(cp2.parent_step, Some(1));

    // 5. Switch back to main branch
    ShadowEngine::switch_branch(&mut tx, "main").expect("switch to main");
    assert_eq!(tx.active_branch, "main");
    assert_eq!(tx.active_step(), 1);
    // experiment.rs should NOT exist on main!
    assert!(!exp_file.exists());
    assert_eq!(fs::read_to_string(&file1).unwrap(), "version 2 on main");

    // 6. Merge speculative_ai into main
    let merge_cp = ShadowEngine::merge_branch(&mut tx, "speculative_ai", "main").expect("merge");
    assert_eq!(merge_cp.branch, "main");
    // Now experiment.rs MUST exist on main after merge!
    assert!(exp_file.exists());
    assert_eq!(fs::read_to_string(&exp_file).unwrap(), "fn innovate() {}");

    // 7. Commit
    ShadowEngine::commit(&mut tx).expect("commit");
    assert_eq!(tx.status, TransactionStatus::Committed);
}
