use rusqlite::Connection;
use savepoint_state::config::{ProviderKind, ProviderSpec};
use savepoint_state::provider::StateProvider;
use savepoint_state::sqlite::SqliteProvider;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

fn make_spec(name: &str, path: PathBuf) -> ProviderSpec {
    ProviderSpec {
        name: name.to_string(),
        kind: ProviderKind::Sqlite,
        url: None,
        path: Some(path),
        include: vec![],
        exclude: vec![],
        terminate_connections: true,
        consistent: true,
        lock_timeout_secs: 30,
        maintenance_db: None,
    }
}

#[test]
fn test_sqlite_round_trip_and_schema_migration() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("app.db");
    let snap_dir = dir.path().join("snapshots").join("cp1");
    fs::create_dir_all(&snap_dir).unwrap();

    // 1. Initial database with table and 2 rows
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
             INSERT INTO users (name) VALUES ('Alice'), ('Bob');",
        )
        .unwrap();
    }

    let spec = make_spec("app_db", db_path.clone());
    let provider = SqliteProvider::new(spec, dir.path().to_path_buf());
    provider.preflight().unwrap();

    // 2. Snapshot
    let manifest = provider.snapshot(&snap_dir).unwrap();
    assert_eq!(manifest.provider_name, "app_db");
    provider.verify(&snap_dir).unwrap();

    // 3. Agent mutates schema and data (migration + new inserts)
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "ALTER TABLE users ADD COLUMN email TEXT;
             UPDATE users SET email = name || '@test.com';
             CREATE TABLE posts (id INTEGER PRIMARY KEY, title TEXT);
             INSERT INTO posts (title) VALUES ('Post 1');",
        )
        .unwrap();
    }

    // 4. Restore
    provider.restore(&snap_dir).unwrap();

    // 5. Verify restored state
    {
        let conn = Connection::open(&db_path).unwrap();
        let mut stmt = conn.prepare("SELECT name FROM users ORDER BY id").unwrap();
        let names: Vec<String> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(names, vec!["Alice", "Bob"]);

        // Verify altered column is gone and posts table is gone
        assert!(conn.query_row("SELECT email FROM users", [], |_| Ok(())).is_err());
        assert!(conn.execute_batch("SELECT * FROM posts").is_err());
    }
}

#[test]
fn test_sqlite_uncheckpointed_wal_frames_captured() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("wal_test.db");
    let snap_dir = dir.path().join("snapshots").join("cp_wal");
    fs::create_dir_all(&snap_dir).unwrap();

    // Open connection in WAL mode, insert data without checkpointing
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE items (id INTEGER PRIMARY KEY, val TEXT);
         INSERT INTO items (val) VALUES ('item1');",
    )
    .unwrap();

    let spec = make_spec("wal_db", db_path.clone());
    let provider = SqliteProvider::new(spec, dir.path().to_path_buf());

    // Snapshot while connection is open
    provider.snapshot(&snap_dir).unwrap();

    // Add more rows into live db
    conn.execute("INSERT INTO items (val) VALUES ('item2')", []).unwrap();
    let count: i64 = conn.query_row("SELECT count(*) FROM items", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 2);

    drop(conn);

    // Restore
    provider.restore(&snap_dir).unwrap();

    // Verify
    let conn2 = Connection::open(&db_path).unwrap();
    let count_restored: i64 = conn2.query_row("SELECT count(*) FROM items", [], |r| r.get(0)).unwrap();
    assert_eq!(count_restored, 1);
}

#[test]
fn test_sqlite_consistent_snapshot_under_concurrent_writer() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("concurrent.db");
    let snap_dir = dir.path().join("snapshots").join("cp_concurrent");
    fs::create_dir_all(&snap_dir).unwrap();

    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE batches (id INTEGER PRIMARY KEY, batch_id INTEGER);",
        )
        .unwrap();
    }

    let running = Arc::new(AtomicBool::new(true));
    let r_clone = running.clone();
    let p_clone = db_path.clone();

    // Thread writing rows in groups of 10 inside a single transaction
    let writer_handle = thread::spawn(move || {
        let mut conn = Connection::open(&p_clone).unwrap();
        conn.busy_timeout(Duration::from_secs(5)).unwrap();
        let mut batch = 1;
        while r_clone.load(Ordering::SeqCst) {
            let tx = conn.transaction().unwrap();
            for _ in 0..10 {
                tx.execute("INSERT INTO batches (batch_id) VALUES (?1)", [batch]).unwrap();
            }
            tx.commit().unwrap();
            batch += 1;
            thread::sleep(Duration::from_millis(5));
        }
    });

    thread::sleep(Duration::from_millis(30));

    let spec = make_spec("concurrent_db", db_path.clone());
    let provider = SqliteProvider::new(spec, dir.path().to_path_buf());
    let manifest = provider.snapshot(&snap_dir).unwrap();
    assert_eq!(manifest.provider_name, "concurrent_db");

    running.store(false, Ordering::SeqCst);
    writer_handle.join().unwrap();

    // Verify snapshot file has rows that are multiples of 10
    let snapshot_file = snap_dir.join("database.sqlite");
    let snap_conn = Connection::open(snapshot_file).unwrap();
    let count: i64 = snap_conn.query_row("SELECT count(*) FROM batches", [], |r| r.get(0)).unwrap();
    assert!(count > 0, "Snapshot must have captured some rows");
    assert_eq!(count % 10, 0, "Snapshot must be transactionally consistent (count multiple of 10)");
}

#[test]
fn test_sqlite_absent_at_checkpoint_deleted_on_restore() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("not_yet_created.db");
    let snap_dir = dir.path().join("snapshots").join("cp_absent");
    fs::create_dir_all(&snap_dir).unwrap();

    // Database does not exist yet at checkpoint time
    let spec = make_spec("absent_db", db_path.clone());
    let provider = SqliteProvider::new(spec, dir.path().to_path_buf());
    let manifest = provider.snapshot(&snap_dir).unwrap();
    assert_eq!(manifest.metadata["exists"], false);

    // Later, agent creates the database
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE dummy (x INT);").unwrap();
    }
    assert!(db_path.exists());

    // Restore to checkpoint when DB didn't exist
    provider.restore(&snap_dir).unwrap();
    assert!(!db_path.exists(), "DB created after checkpoint must be deleted on restore");
}

#[test]
fn test_sqlite_corrupt_live_file_recovered() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("recover.db");
    let snap_dir = dir.path().join("snapshots").join("cp_recover");
    fs::create_dir_all(&snap_dir).unwrap();

    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE secrets (data TEXT);
             INSERT INTO secrets VALUES ('safe_data');",
        )
        .unwrap();
    }

    let spec = make_spec("recover_db", db_path.clone());
    let provider = SqliteProvider::new(spec, dir.path().to_path_buf());
    provider.snapshot(&snap_dir).unwrap();

    // Corrupt the live database file with garbage
    fs::write(&db_path, b"CORRUPTED TRASH DATA").unwrap();

    // Restore should succeed even if live file is unparseable SQLite
    provider.restore(&snap_dir).unwrap();

    let conn = Connection::open(&db_path).unwrap();
    let val: String = conn.query_row("SELECT data FROM secrets", [], |r| r.get(0)).unwrap();
    assert_eq!(val, "safe_data");
}
