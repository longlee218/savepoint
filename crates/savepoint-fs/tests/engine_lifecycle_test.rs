use std::fs;
use tempfile::tempdir_in;
use savepoint_core::TransactionStatus;
use savepoint_fs::engine::ShadowEngine;

#[test]
fn test_engine_transaction_lifecycle() {
    let base_dir = std::env::current_dir().expect("Failed to get current dir");
    let test_dir = tempdir_in(&base_dir).expect("Failed to create tempdir");
    let ws = test_dir.path().join("my_workspace");

    // 1. Setup workspace with initial files
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src/main.rs"), "fn main() { println!(\"v1\"); }").unwrap();
    fs::write(ws.join("src/lib.rs"), "pub fn add(a: i32, b: i32) -> i32 { a + b }").unwrap();

    // 2. Begin transaction
    let mut tx = ShadowEngine::begin_transaction(&ws).expect("Begin transaction failed");
    assert_eq!(tx.status, TransactionStatus::Active);
    assert_eq!(tx.current_step(), 1); // Step 0 is initial base checkpoint

    // 3. Turn 1 (AI modifies src/main.rs and creates src/utils.rs)
    fs::write(ws.join("src/main.rs"), "fn main() { println!(\"v2 - Turn 1\"); }").unwrap();
    fs::write(ws.join("src/utils.rs"), "pub fn helper() {}").unwrap();

    let cp1 = ShadowEngine::checkpoint(&mut tx, Some("Turn 1: update main, add utils"), None)
        .expect("Checkpoint 1 failed");
    assert_eq!(cp1.step_index, 1);
    assert_eq!(cp1.deltas.len(), 2);

    // 4. Turn 2 (AI makes a mistake: corrupts lib.rs and deletes utils.rs)
    fs::write(ws.join("src/lib.rs"), "// CORRUPTED CODE").unwrap();
    fs::remove_file(ws.join("src/utils.rs")).unwrap();

    let cp2 = ShadowEngine::checkpoint(&mut tx, Some("Turn 2: bad edits"), None)
        .expect("Checkpoint 2 failed");
    assert_eq!(cp2.step_index, 2);

    // Verify workspace is currently corrupted
    assert_eq!(fs::read_to_string(ws.join("src/lib.rs")).unwrap(), "// CORRUPTED CODE");
    assert!(!ws.join("src/utils.rs").exists());

    // 5. REWIND to Turn 1!
    let rewound_step = ShadowEngine::rewind_previous(&mut tx).expect("Rewind previous failed");
    assert_eq!(rewound_step, Some(1));

    // Verify Turn 1 state is perfectly restored!
    assert_eq!(
        fs::read_to_string(ws.join("src/main.rs")).unwrap(),
        "fn main() { println!(\"v2 - Turn 1\"); }"
    );
    assert_eq!(
        fs::read_to_string(ws.join("src/lib.rs")).unwrap(),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }"
    );
    assert!(ws.join("src/utils.rs").exists());

    // 5.1 REDO / FAST-FORWARD to Turn 2!
    ShadowEngine::rewind(&mut tx, 2).expect("Fast-forward to Turn 2 failed");
    assert_eq!(fs::read_to_string(ws.join("src/lib.rs")).unwrap(), "// CORRUPTED CODE");
    assert!(!ws.join("src/utils.rs").exists());

    // 5.2 REWIND back to Turn 1 again!
    ShadowEngine::rewind(&mut tx, 1).expect("Rewind to Turn 1 failed");
    assert_eq!(fs::read_to_string(ws.join("src/lib.rs")).unwrap(), "pub fn add(a: i32, b: i32) -> i32 { a + b }");
    assert!(ws.join("src/utils.rs").exists());

    // 6. COMMIT
    ShadowEngine::commit(&mut tx).expect("Commit failed");
    assert_eq!(tx.status, TransactionStatus::Committed);
}

#[test]
fn test_rewind_cleans_up_orphaned_sqlite_wal_and_shm_and_populates_content_hash() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();

    let db_path = ws.join("app.db");
    fs::write(&db_path, "sqlite-v1-clean").unwrap();

    let mut tx = ShadowEngine::begin_transaction(ws).unwrap();
    assert!(tx.checkpoints[0].content_hash.as_deref().unwrap().len() == 64);

    // Simulate dirty WAL writes during agent execution
    fs::write(&db_path, "sqlite-v2-dirty").unwrap();
    fs::write(ws.join("app.db-wal"), "dirty-wal-frames").unwrap();
    fs::write(ws.join("app.db-shm"), "dirty-shm-index").unwrap();

    let cp1 = ShadowEngine::checkpoint(&mut tx, Some("dirty db state"), None).unwrap();
    assert_eq!(cp1.content_hash.as_deref().unwrap().len(), 64);
    assert_ne!(tx.checkpoints[0].content_hash, cp1.content_hash);

    // Rewind back to Step 0 (where app.db-wal and app.db-shm did not exist)
    ShadowEngine::rewind(&mut tx, 0).unwrap();

    assert_eq!(fs::read_to_string(&db_path).unwrap(), "sqlite-v1-clean");
    assert!(!ws.join("app.db-wal").exists(), "orphaned -wal must be removed");
    assert!(!ws.join("app.db-shm").exists(), "orphaned -shm must be removed");
}

#[test]
fn test_heavy_node_modules_collapsed_in_deltas_and_restored_on_rewind() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();

    fs::create_dir_all(ws.join("node_modules/pkg_v1")).unwrap();
    fs::write(ws.join("node_modules/pkg_v1/index.js"), "v1").unwrap();

    let mut tx = ShadowEngine::begin_transaction(ws).unwrap();

    // Agent installs pkg_v2 and removes pkg_v1
    fs::remove_dir_all(ws.join("node_modules/pkg_v1")).unwrap();
    fs::create_dir_all(ws.join("node_modules/pkg_v2")).unwrap();
    fs::write(ws.join("node_modules/pkg_v2/index.js"), "v2").unwrap();

    let cp1 = ShadowEngine::checkpoint(&mut tx, Some("upgrade deps"), None).unwrap();
    assert_eq!(cp1.deltas.len(), 1);
    assert_eq!(cp1.deltas[0].relative_path, std::path::PathBuf::from("node_modules"));

    // Rewind to Step 0 -> pkg_v1 must be restored and pkg_v2 gone!
    ShadowEngine::rewind(&mut tx, 0).unwrap();
    assert!(ws.join("node_modules/pkg_v1/index.js").exists());
    assert!(!ws.join("node_modules/pkg_v2").exists());
}
