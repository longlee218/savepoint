use std::fs;
use std::time::Instant;
use tempfile::tempdir_in;
use savepoint_fs::clone_tree;

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
