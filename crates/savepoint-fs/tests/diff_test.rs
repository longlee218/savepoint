use std::fs;
use tempfile::tempdir_in;
use savepoint_core::ChangeKind;
use savepoint_fs::clone_tree;
use savepoint_fs::diff::compute_deltas;

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
    fs::write(working.join("src/modify.rs"), "fn new_version() { println!(\"updated\"); }").unwrap();
    // - Delete
    fs::remove_file(working.join("src/delete.rs")).unwrap();
    // - Create
    fs::write(working.join("src/create.rs"), "fn newly_created() {}").unwrap();

    // 4. Compute deltas
    let deltas = compute_deltas(&original, &working).expect("Failed to compute deltas");

    println!("Detected Deltas: {:#?}", deltas);

    assert_eq!(deltas.len(), 3);

    let created = deltas.iter().find(|d| d.kind == ChangeKind::Created).expect("Missing Created");
    assert_eq!(created.relative_path, std::path::Path::new("src/create.rs"));

    let modified = deltas.iter().find(|d| d.kind == ChangeKind::Modified).expect("Missing Modified");
    assert_eq!(modified.relative_path, std::path::Path::new("src/modify.rs"));

    let deleted = deltas.iter().find(|d| d.kind == ChangeKind::Deleted).expect("Missing Deleted");
    assert_eq!(deleted.relative_path, std::path::Path::new("src/delete.rs"));
}
