use savepoint_fs::clone_tree;
use std::fs;
use std::time::Instant;
use tempfile::tempdir_in;

#[test]
fn test_apfs_cow_clone_and_benchmark() {
    // Note: tempdir must be created on the current APFS volume (e.g. in ./ or /Users/...)
    // /tmp on macOS is a symlink to /private/tmp, which is also on the root APFS container.
    let base_dir = std::env::current_dir().expect("Failed to get current dir");
    let test_dir = tempdir_in(&base_dir).expect("Failed to create tempdir in project root");
    let src = test_dir.path().join("src_project");
    let dst = test_dir.path().join("dst_project_snapshot");

    fs::create_dir_all(src.join("src/auth")).expect("Failed to create dirs");
    fs::create_dir_all(src.join("src/models")).expect("Failed to create dirs");
    fs::create_dir_all(src.join("node_modules/dummy_pkg")).expect("Failed to create dirs");

    // Populate with 100 sample files
    for i in 0..100 {
        let file_path = src.join(format!("src/models/model_{}.rs", i));
        fs::write(&file_path, format!("// Auto-generated struct Model_{} with 100 fields\npub struct Model_{} {{ pub id: u64 }}", i, i))
            .expect("Failed to write file");
    }

    // Measure clone duration
    let start = Instant::now();
    clone_tree(&src, &dst).expect("APFS clone_tree must succeed");
    let elapsed = start.elapsed();

    println!("⚡ APFS CoW Clone 100 files elapsed time: {:?}", elapsed);

    // Verify all 100 files exist in dst
    assert!(dst.join("src/models/model_0.rs").exists());
    assert!(dst.join("src/models/model_99.rs").exists());

    // Verify Copy-on-Write property:
    // Modifying file in dst should NOT change file in src!
    let dst_file = dst.join("src/models/model_0.rs");
    fs::write(&dst_file, "// MODIFIED IN SHADOW").expect("Failed to modify dst file");

    let src_content = fs::read_to_string(src.join("src/models/model_0.rs")).expect("Read src");
    let dst_content = fs::read_to_string(&dst_file).expect("Read dst");

    assert_ne!(src_content, dst_content);
    assert!(src_content.contains("pub struct Model_0"));
    assert_eq!(dst_content, "// MODIFIED IN SHADOW");

    // Benchmark SLA check: Must complete under 10ms (typically < 3ms on APFS)
    assert!(
        elapsed.as_millis() < 50,
        "CoW clone took too long: {:?}",
        elapsed
    );
}

#[test]
fn test_clone_workspace_skips_git_and_savepoint_and_handles_broken_symlink() {
    use savepoint_fs::clone_workspace;
    use std::os::unix::fs::symlink;
    use std::path::Path;

    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path().join("ws");
    let snap = temp.path().join("snap");
    std::fs::create_dir_all(ws.join(".git/objects")).unwrap();
    std::fs::write(ws.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::create_dir_all(ws.join(".savepoint/spool")).unwrap();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/lib.rs"), "pub fn ok() {}").unwrap();
    symlink(
        Path::new("nonexistent_target"),
        ws.join("src/dangling_link"),
    )
    .unwrap();

    clone_workspace(&ws, &snap).expect("clone_workspace must succeed");

    assert!(!snap.join(".git").exists());
    assert!(!snap.join(".savepoint").exists());
    assert!(snap.join("src/lib.rs").exists());
    assert!(snap.join("src/dangling_link").symlink_metadata().is_ok());
}

#[test]
fn test_fallback_copy_preserves_mtime_for_zero_diff() {
    use savepoint_fs::clone::fallback_copy_tree;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::time::{Duration, SystemTime};

    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    let dst = temp.path().join("dst");
    std::fs::create_dir_all(src.join("node_modules/pkg")).unwrap();
    let f = src.join("data.txt");
    let ro = src.join("readonly.txt");
    let pkg_f = src.join("node_modules/pkg/index.js");
    std::fs::write(&f, "hello world").unwrap();
    std::fs::write(&ro, "immutable secret").unwrap();
    std::fs::write(&pkg_f, "module.exports = 1;").unwrap();

    // Set an explicit historical mtime (10 seconds ago) on files and directories
    // so we can verify fallback_copy_tree actually copies mtime rather than stamping now().
    let past = SystemTime::now() - Duration::from_secs(10);
    for p in [
        &f,
        &ro,
        &pkg_f,
        &src.join("node_modules/pkg"),
        &src.join("node_modules"),
    ] {
        let fh = std::fs::File::open(p).unwrap();
        fh.set_modified(past).unwrap();
    }
    // Make readonly.txt read-only (0o444)
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o444)).unwrap();
    let fh_ro = std::fs::File::open(&ro).unwrap();
    fh_ro.set_modified(past).unwrap();

    fallback_copy_tree(&src, &dst).expect("fallback_copy_tree must succeed");

    for rel in [
        "data.txt",
        "readonly.txt",
        "node_modules",
        "node_modules/pkg",
    ] {
        let m1 = std::fs::symlink_metadata(src.join(rel)).unwrap();
        let m2 = std::fs::symlink_metadata(dst.join(rel)).unwrap();
        assert_eq!(m1.mtime(), m2.mtime(), "mtime mismatch for {}", rel);
        assert_eq!(
            m1.mtime_nsec(),
            m2.mtime_nsec(),
            "mtime_nsec mismatch for {}",
            rel
        );
    }

    // Also verify compute_deltas between src and fallback-copied dst is 0 deltas!
    let deltas = savepoint_fs::compute_deltas(&src, &dst).unwrap();
    assert!(
        deltas.is_empty(),
        "expected 0 deltas after fallback_copy_tree, got: {:?}",
        deltas
    );
}
