use savepoint_core::ChangeKind;
use savepoint_fs::clone_tree;
use savepoint_fs::diff::compute_deltas;
use std::fs;
use tempfile::tempdir_in;

#[test]
fn test_directory_diff_detection() {
    let base_dir = std::env::current_dir().expect("Failed to get current dir");
    let test_dir = tempdir_in(&base_dir).expect("Failed to create tempdir");
    let original = test_dir.path().join("original");
    let working = test_dir.path().join("working");

    // 1. Create original structure
    fs::create_dir_all(original.join("src")).unwrap();
    fs::write(original.join("src/keep.rs"), "fn keep() {}").unwrap();
    fs::write(original.join("src/modify.rs"), "fn old() {}").unwrap();
    fs::write(original.join("src/delete.rs"), "fn to_be_deleted() {}").unwrap();

    // 2. Clone to working using CoW
    clone_tree(&original, &working).unwrap();

    // 3. Apply changes in working directory:
    // - Modify
    fs::write(
        working.join("src/modify.rs"),
        "fn new_version() { println!(\"updated\"); }",
    )
    .unwrap();
    // - Delete
    fs::remove_file(working.join("src/delete.rs")).unwrap();
    // - Create
    fs::write(working.join("src/create.rs"), "fn newly_created() {}").unwrap();

    // 4. Compute deltas
    let deltas = compute_deltas(&original, &working).expect("Failed to compute deltas");

    println!("Detected Deltas: {:#?}", deltas);

    assert_eq!(deltas.len(), 3);

    let created = deltas
        .iter()
        .find(|d| d.kind == ChangeKind::Created)
        .expect("Missing Created");
    assert_eq!(created.relative_path, std::path::Path::new("src/create.rs"));

    let modified = deltas
        .iter()
        .find(|d| d.kind == ChangeKind::Modified)
        .expect("Missing Modified");
    assert_eq!(
        modified.relative_path,
        std::path::Path::new("src/modify.rs")
    );

    let deleted = deltas
        .iter()
        .find(|d| d.kind == ChangeKind::Deleted)
        .expect("Missing Deleted");
    assert_eq!(deleted.relative_path, std::path::Path::new("src/delete.rs"));
}

#[test]
fn test_two_stage_stat_fast_path_and_same_size_content_diff() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("base");
    let curr = temp.path().join("curr");
    std::fs::create_dir_all(&base).unwrap();

    let untouched_base = base.join("untouched.txt");
    let rewritten_base = base.join("rewritten.txt");
    std::fs::write(&untouched_base, "aaaa").unwrap();
    std::fs::write(&rewritten_base, "1111").unwrap();
    std::os::unix::fs::symlink(
        std::path::Path::new("missing_target"),
        base.join("broken_link"),
    )
    .unwrap();

    savepoint_fs::clone_workspace(&base, &curr).unwrap();

    // Make untouched file unreadable (0o000) while preserving its mtime from clone
    let orig_mtime = untouched_base.metadata().unwrap().modified().unwrap();
    let untouched_curr = curr.join("untouched.txt");
    std::fs::set_permissions(&untouched_curr, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert_eq!(
        untouched_curr.metadata().unwrap().modified().unwrap(),
        orig_mtime
    );

    // Overwrite rewritten.txt with same byte length (4 bytes) and advance mtime
    let rewritten_curr = curr.join("rewritten.txt");
    std::fs::write(&rewritten_curr, "2222").unwrap();
    let f = std::fs::File::options()
        .write(true)
        .open(&rewritten_curr)
        .unwrap();
    f.set_modified(orig_mtime + Duration::from_secs(5)).unwrap();

    let deltas = savepoint_fs::compute_deltas(&base, &curr)
        .expect("diff must succeed without opening untouched.txt");

    // Restore permissions so tempdir cleanup succeeds
    std::fs::set_permissions(&untouched_curr, std::fs::Permissions::from_mode(0o644)).unwrap();

    assert_eq!(deltas.len(), 1);
    assert_eq!(
        deltas[0].relative_path,
        std::path::PathBuf::from("rewritten.txt")
    );
    assert_eq!(deltas[0].kind, savepoint_core::ChangeKind::Modified);
}

#[test]
fn test_heavy_dirs_collapsed_and_build_caches_ignored() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("base");
    let curr = temp.path().join("curr");
    std::fs::create_dir_all(&base).unwrap();
    savepoint_fs::clone_workspace(&base, &curr).unwrap();

    // Populate target/ with build artifacts -> should be ignored in compute_deltas
    std::fs::create_dir_all(curr.join("target/debug/deps")).unwrap();
    std::fs::write(curr.join("target/debug/deps/libfoo.rlib"), "binary").unwrap();

    // Populate legitimate nested src/target/mod.rs -> MUST NOT be ignored!
    std::fs::create_dir_all(curr.join("src/target")).unwrap();
    std::fs::write(curr.join("src/target/mod.rs"), "pub mod arch;").unwrap();

    // Populate node_modules/ with multiple nested files -> should collapse to 1 "node_modules" delta
    std::fs::create_dir_all(curr.join("node_modules/pkg_a")).unwrap();
    std::fs::write(
        curr.join("node_modules/pkg_a/index.js"),
        "module.exports = 1;",
    )
    .unwrap();
    std::fs::create_dir_all(curr.join("node_modules/pkg_b")).unwrap();
    std::fs::write(
        curr.join("node_modules/pkg_b/index.js"),
        "module.exports = 2;",
    )
    .unwrap();

    let deltas = savepoint_fs::compute_deltas(&base, &curr).unwrap();
    assert_eq!(
        deltas.len(),
        2,
        "Expected node_modules and src/target/mod.rs deltas, got: {:?}",
        deltas
    );
    assert!(deltas
        .iter()
        .any(|d| d.relative_path == std::path::Path::new("node_modules")
            && d.kind == savepoint_core::ChangeKind::Created));
    assert!(deltas.iter().any(
        |d| d.relative_path == std::path::Path::new("src/target/mod.rs")
            && d.kind == savepoint_core::ChangeKind::Created
    ));
}

#[test]
fn test_node_modules_unchanged_produces_zero_deltas() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("base");
    let curr = temp.path().join("curr");
    std::fs::create_dir_all(base.join("node_modules/pkg_a")).unwrap();
    std::fs::write(base.join("node_modules/pkg_a/index.js"), "module.exports = 1;").unwrap();
    std::fs::write(base.join("package.json"), "{}").unwrap();

    savepoint_fs::clone_workspace(&base, &curr).unwrap();

    let deltas = savepoint_fs::compute_deltas(&base, &curr).unwrap();
    assert_eq!(
        deltas.len(),
        0,
        "Untouched node_modules must not produce any diff: {:?}",
        deltas
    );
}

#[test]
fn test_gitignore_respected_by_clone_and_diff() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path().join("ws");
    let snap = temp.path().join("snap");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::create_dir_all(ws.join(".claude/worktrees")).unwrap();
    std::fs::create_dir_all(ws.join("node_modules/dep")).unwrap();
    std::fs::create_dir_all(ws.join("dist")).unwrap();

    std::fs::write(ws.join(".gitignore"), "node_modules\n.claude/\ndist/\n").unwrap();
    std::fs::write(ws.join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(ws.join("node_modules/dep/index.js"), "1").unwrap();
    std::fs::write(ws.join(".claude/worktrees/config.json"), "{}").unwrap();
    std::fs::write(ws.join("dist/bundle.js"), "1").unwrap();

    savepoint_fs::clone_workspace(&ws, &snap).unwrap();

    // Verify snapshot skipped gitignored entries and .claude
    assert!(snap.join("src/main.rs").exists());
    assert!(!snap.join("node_modules").exists());
    assert!(!snap.join(".claude").exists());
    assert!(!snap.join("dist").exists());

    // Verify compute_deltas also ignores them
    let deltas = savepoint_fs::compute_deltas(&snap, &ws).unwrap();
    assert_eq!(deltas.len(), 0, "No changes detected on ignored paths");
}
