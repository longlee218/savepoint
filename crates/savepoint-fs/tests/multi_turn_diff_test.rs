use savepoint_core::ChangeKind;
use savepoint_fs::ShadowEngine;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_multi_turn_diff_between_steps() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();

    let mut tx = ShadowEngine::begin_transaction(&ws).unwrap();

    // Step 1: add file_a
    fs::write(ws.join("file_a.txt"), "A1").unwrap();
    ShadowEngine::checkpoint(&mut tx, Some("Add file_a"), None).unwrap();

    // Step 2: edit file_a, add file_b
    fs::write(ws.join("file_a.txt"), "A2 modified").unwrap();
    fs::write(ws.join("file_b.txt"), "B1").unwrap();
    ShadowEngine::checkpoint(&mut tx, Some("Edit a, add b"), None).unwrap();

    // Step 3: delete file_a, add file_c
    fs::remove_file(ws.join("file_a.txt")).unwrap();
    fs::write(ws.join("file_c.txt"), "C1").unwrap();
    ShadowEngine::checkpoint(&mut tx, Some("Remove a, add c"), None).unwrap();

    // 1. Diff from Step 1 to Step 2
    let diff_1_2 = ShadowEngine::diff_checkpoints(&tx, 1, 2).unwrap();
    assert_eq!(diff_1_2.len(), 2);
    let mod_a = diff_1_2
        .iter()
        .find(|d| d.relative_path == std::path::Path::new("file_a.txt"))
        .unwrap();
    assert_eq!(mod_a.kind, ChangeKind::Modified);
    let new_b = diff_1_2
        .iter()
        .find(|d| d.relative_path == std::path::Path::new("file_b.txt"))
        .unwrap();
    assert_eq!(new_b.kind, ChangeKind::Created);

    // 2. Diff from Step 0 to Step 3
    let diff_0_3 = ShadowEngine::diff_checkpoints(&tx, 0, 3).unwrap();
    // In step 3: file_b and file_c exist; file_a is gone
    let has_b = diff_0_3
        .iter()
        .any(|d| d.relative_path == std::path::Path::new("file_b.txt"));
    let has_c = diff_0_3
        .iter()
        .any(|d| d.relative_path == std::path::Path::new("file_c.txt"));
    assert!(has_b);
    assert!(has_c);

    // 3. Diff from Step 2 to Live
    // Modify file_c in live
    fs::write(ws.join("file_c.txt"), "C1 live changes").unwrap();
    let diff_live = ShadowEngine::diff_checkpoint_to_live(&tx, 2).unwrap();
    assert!(!diff_live.is_empty());
}
