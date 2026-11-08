use savepoint_core::types::ChangeKind;
use savepoint_fs::clone::clone_workspace;
use savepoint_fs::diff::compute_deltas;
use savepoint_fs::ShadowEngine;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use tempfile::tempdir;

#[test]
fn test_cow_deeply_nested_directories_and_rewind() {
    let dir = tempdir().unwrap();
    let ws = dir.path().join("workspace");
    fs::create_dir_all(&ws).unwrap();

    let deep_dir = ws.join("a/b/c/d/e/f/g/h");
    fs::create_dir_all(&deep_dir).unwrap();
    fs::write(deep_dir.join("leaf.txt"), "depth 8").unwrap();

    let mut tx = ShadowEngine::begin_transaction(&ws).unwrap();
    assert_eq!(tx.checkpoints.len(), 1);
    assert_eq!(tx.checkpoints[0].step_index, 0);

    // Modify leaf and add sibling
    fs::write(deep_dir.join("leaf.txt"), "depth 8 modified").unwrap();
    fs::write(deep_dir.join("leaf_sibling.txt"), "sibling").unwrap();

    let deltas = ShadowEngine::compute_live_deltas(&tx).unwrap();
    assert_eq!(deltas.len(), 2);

    let cp1 = ShadowEngine::checkpoint(&mut tx, Some("Mutated deep tree"), None).unwrap();
    assert_eq!(cp1.step_index, 1);

    // Rewind back to 0
    ShadowEngine::rewind(&mut tx, 0).unwrap();
    assert_eq!(fs::read_to_string(deep_dir.join("leaf.txt")).unwrap(), "depth 8");
    assert!(!deep_dir.join("leaf_sibling.txt").exists());
}

#[test]
fn test_cow_unicode_and_special_character_filenames() {
    let dir = tempdir().unwrap();
    let ws = dir.path().join("workspace");
    fs::create_dir_all(&ws).unwrap();

    let vn_file = ws.join("dữ_liệu_kiểm_thử.json");
    let emoji_file = ws.join("⚡_savepoint_🚀.rs");
    let space_file = ws.join("path with spaces and # symbols.md");

    fs::write(&vn_file, "{\"tieng\": \"viet\"}").unwrap();
    fs::write(&emoji_file, "fn fast() {}").unwrap();
    fs::write(&space_file, "# Markdown").unwrap();

    let mut tx = ShadowEngine::begin_transaction(&ws).unwrap();
    assert_eq!(tx.checkpoints[0].step_index, 0);

    // Verify clone integrity in snapshot
    let snap_path = &tx.checkpoints[0].snapshot_path;
    assert!(snap_path.join("dữ_liệu_kiểm_thử.json").exists());
    assert!(snap_path.join("⚡_savepoint_🚀.rs").exists());
    assert!(snap_path.join("path with spaces and # symbols.md").exists());

    // Mutate and restore
    fs::remove_file(&emoji_file).unwrap();
    fs::write(&vn_file, "{\"modified\": true}").unwrap();

    ShadowEngine::rewind(&mut tx, 0).unwrap();
    assert!(emoji_file.exists());
    assert_eq!(fs::read_to_string(&vn_file).unwrap(), "{\"tieng\": \"viet\"}");
}

#[test]
fn test_cow_preserves_file_permissions() {
    let dir = tempdir().unwrap();
    let ws = dir.path().join("workspace");
    fs::create_dir_all(&ws).unwrap();

    let script = ws.join("deploy.sh");
    fs::write(&script, "#!/bin/sh\necho ok\n").unwrap();
    let mut perms = fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).unwrap();

    let snap_dir = dir.path().join("snap");
    clone_workspace(&ws, &snap_dir).unwrap();

    let cloned_perms = fs::metadata(snap_dir.join("deploy.sh")).unwrap().permissions();
    assert_eq!(cloned_perms.mode() & 0o777, 0o755);
}

#[test]
fn test_cow_relative_symlink_preserved() {
    let dir = tempdir().unwrap();
    let ws = dir.path().join("workspace");
    fs::create_dir_all(&ws).unwrap();

    let target = ws.join("target.txt");
    fs::write(&target, "symlink target content").unwrap();

    let link = ws.join("link.txt");
    std::os::unix::fs::symlink("target.txt", &link).unwrap();

    let snap_dir = dir.path().join("snap");
    clone_workspace(&ws, &snap_dir).unwrap();

    let cloned_link = snap_dir.join("link.txt");
    assert!(cloned_link.is_symlink());
    assert_eq!(
        fs::read_link(&cloned_link).unwrap(),
        std::path::PathBuf::from("target.txt")
    );
}

#[test]
fn test_diff_engine_detects_all_change_kinds() {
    let dir = tempdir().unwrap();
    let ws_a = dir.path().join("snap_a");
    let ws_b = dir.path().join("snap_b");
    fs::create_dir_all(&ws_a).unwrap();
    fs::create_dir_all(&ws_b).unwrap();

    // in A and B (modified)
    fs::write(ws_a.join("modified.txt"), "version 1").unwrap();
    fs::write(ws_b.join("modified.txt"), "version 2 longer content").unwrap();

    // only in A (deleted in B)
    fs::write(ws_a.join("deleted.txt"), "gone in b").unwrap();

    // only in B (created in B)
    fs::write(ws_b.join("created.txt"), "new in b").unwrap();

    // identical file
    fs::write(ws_a.join("same.txt"), "constant").unwrap();
    fs::write(ws_b.join("same.txt"), "constant").unwrap();

    let diffs = compute_deltas(&ws_a, &ws_b).unwrap();
    assert_eq!(diffs.len(), 3);

    let mod_delta = diffs.iter().find(|d| d.relative_path.to_str() == Some("modified.txt")).unwrap();
    assert_eq!(mod_delta.kind, ChangeKind::Modified);

    let del_delta = diffs.iter().find(|d| d.relative_path.to_str() == Some("deleted.txt")).unwrap();
    assert_eq!(del_delta.kind, ChangeKind::Deleted);

    let cre_delta = diffs.iter().find(|d| d.relative_path.to_str() == Some("created.txt")).unwrap();
    assert_eq!(cre_delta.kind, ChangeKind::Created);
}
