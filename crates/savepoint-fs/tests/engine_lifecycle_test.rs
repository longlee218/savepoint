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
