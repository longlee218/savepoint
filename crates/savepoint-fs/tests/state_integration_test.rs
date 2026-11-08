use rusqlite::Connection;
use savepoint_fs::ShadowEngine;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_shadow_engine_atomic_file_and_database_rewind() {
    let dir = tempdir().unwrap();
    let ws = dir.path();
    let db_path = ws.join("app.db");

    // 1. Create .sp-state.yaml configuring SQLite state capture
    fs::write(
        ws.join(".sp-state.yaml"),
        "providers:\n  - name: my_sqlite\n    kind: sqlite\n    path: app.db\n",
    )
    .unwrap();

    // 2. Initialize database and code file
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE customers (id INT PRIMARY KEY, name TEXT);
             INSERT INTO customers VALUES (1, 'Alice');",
        )
        .unwrap();
    }
    fs::write(ws.join("src.rs"), "fn main() { println!(\"v1\"); }").unwrap();

    // 3. Begin transaction (captures step 0 baseline)
    let mut tx = ShadowEngine::begin_transaction(ws).unwrap();

    // 4. Checkpoint #1
    let cp1 = ShadowEngine::checkpoint(&mut tx, Some("Checkpoint 1"), None).unwrap();
    assert_eq!(cp1.step_index, 1);

    // 5. Agent modifies both code file AND database in Step 2
    fs::write(ws.join("src.rs"), "fn main() { println!(\"v2_broken\"); }").unwrap();
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "INSERT INTO customers VALUES (2, 'Bob');
             CREATE TABLE corrupted_data (bad INT);",
        )
        .unwrap();
    }

    // Checkpoint #2
    let cp2 = ShadowEngine::checkpoint(&mut tx, Some("Checkpoint 2 (broken)"), None).unwrap();
    assert_eq!(cp2.step_index, 2);

    // 6. Rewind back to Checkpoint #1
    ShadowEngine::rewind(&mut tx, 1).unwrap();

    // 7. Verify atomic rollback:
    // Code file restored to v1:
    let code = fs::read_to_string(ws.join("src.rs")).unwrap();
    assert_eq!(code, "fn main() { println!(\"v1\"); }");

    // Database restored to Checkpoint 1 (only Alice, no Bob, no corrupted_data table!):
    {
        let conn = Connection::open(&db_path).unwrap();
        let count: i64 = conn.query_row("SELECT count(*) FROM customers", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1, "Customers table must be reverted to 1 customer");

        let name: String = conn.query_row("SELECT name FROM customers WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(name, "Alice");

        assert!(conn.execute_batch("SELECT * FROM corrupted_data").is_err(), "Table corrupted_data must not exist");
    }
}
