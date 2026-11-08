use postgres::{Client, NoTls};
use savepoint_state::config::{ProviderKind, ProviderSpec};
use savepoint_state::provider::StateProvider;
use savepoint_state::postgres::PostgresProvider;
use std::env;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

fn get_pg_url() -> Option<String> {
    env::var("SP_TEST_PG_URL").ok().filter(|s| !s.is_empty())
}

fn make_pg_spec(name: &str, url: &str, terminate: bool) -> ProviderSpec {
    ProviderSpec {
        name: name.to_string(),
        kind: ProviderKind::Postgres,
        url: Some(url.to_string()),
        path: None,
        include: vec![],
        exclude: vec![],
        terminate_connections: terminate,
        consistent: true,
        lock_timeout_secs: 10,
        maintenance_db: Some("postgres".to_string()),
    }
}

fn create_fresh_db(base_url: &str, db_name: &str) -> String {
    let mut root_client = Client::connect(base_url, NoTls).unwrap();
    let quoted_name = format!("\"{}\"", db_name.replace('"', "\"\""));
    let _ = root_client.execute(
        &format!("DROP DATABASE IF EXISTS {} WITH (FORCE);", quoted_name),
        &[],
    );
    root_client
        .execute(&format!("CREATE DATABASE {};", quoted_name), &[])
        .unwrap();

    let mut parts: Vec<&str> = base_url.split('/').collect();
    parts.pop();
    format!("{}/{}", parts.join("/"), db_name)
}

#[test]
fn test_postgres_full_object_round_trip() {
    let Some(base_url) = get_pg_url() else {
        println!("SKIPPED: SP_TEST_PG_URL not set");
        return;
    };

    let db_url = create_fresh_db(&base_url, "sp_test_roundtrip");
    let dir = tempdir().unwrap();
    let snap_dir = dir.path().join("cp1");
    fs::create_dir_all(&snap_dir).unwrap();

    // 1. Setup schema with tables, FKs, sequences, views, functions, triggers, enums
    {
        let mut client = Client::connect(&db_url, NoTls).unwrap();
        client
            .batch_execute(
                "CREATE TYPE user_role AS ENUM ('admin', 'member', 'guest');
                 CREATE TABLE departments (
                     id SERIAL PRIMARY KEY,
                     name TEXT NOT NULL
                 );
                 CREATE TABLE employees (
                     id SERIAL PRIMARY KEY,
                     dept_id INT REFERENCES departments(id),
                     name TEXT NOT NULL,
                     role user_role DEFAULT 'member',
                     created_at TIMESTAMPTZ DEFAULT NOW()
                 );
                 CREATE VIEW dept_summary AS 
                     SELECT d.name, count(e.id) as emp_count 
                     FROM departments d LEFT JOIN employees e ON d.id = e.dept_id 
                     GROUP BY d.name;
                 CREATE OR REPLACE FUNCTION audit_emp() RETURNS TRIGGER AS $$
                 BEGIN
                     NEW.name = UPPER(NEW.name);
                     RETURN NEW;
                 END;
                 $$ LANGUAGE plpgsql;
                 CREATE TRIGGER trg_upper_name BEFORE INSERT ON employees
                     FOR EACH ROW EXECUTE FUNCTION audit_emp();

                 INSERT INTO departments (name) VALUES ('Engineering'), ('Design');
                 INSERT INTO employees (dept_id, name, role) VALUES (1, 'Alice', 'admin');",
            )
            .unwrap();
    }

    let spec = make_pg_spec("pg_app", &db_url, true);
    let provider = PostgresProvider::new(spec);
    provider.preflight().unwrap();

    // 2. Snapshot
    let manifest = provider.snapshot(&snap_dir).unwrap();
    assert_eq!(manifest.provider_name, "pg_app");
    provider.verify(&snap_dir).unwrap();

    // 3. Mutate: insert, delete, alter schema
    {
        let mut client = Client::connect(&db_url, NoTls).unwrap();
        client
            .batch_execute(
                "INSERT INTO departments (name) VALUES ('Sales');
                 INSERT INTO employees (dept_id, name, role) VALUES (2, 'Bob', 'guest');
                 ALTER TABLE employees ADD COLUMN phone TEXT;
                 DROP VIEW dept_summary;",
            )
            .unwrap();
    }

    // 4. Restore
    provider.restore(&snap_dir).unwrap();

    // 5. Verify restored state
    {
        let mut client = Client::connect(&db_url, NoTls).unwrap();
        let rows = client
            .query("SELECT name, role::text FROM employees ORDER BY id", &[])
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<_, String>(0), "ALICE"); // trigger worked
        assert_eq!(rows[0].get::<_, String>(1), "admin");

        // View must be back
        let view_rows = client
            .query("SELECT emp_count FROM dept_summary WHERE name = 'Engineering'", &[])
            .unwrap();
        assert_eq!(view_rows[0].get::<_, i64>(0), 1);

        // Altered column must be gone
        assert!(client.query("SELECT phone FROM employees", &[]).is_err());
    }

    // Clean up
    provider.discard(&snap_dir).unwrap();
}

#[test]
fn test_postgres_terminate_connections_handling() {
    let Some(base_url) = get_pg_url() else {
        println!("SKIPPED: SP_TEST_PG_URL not set");
        return;
    };

    let db_url = create_fresh_db(&base_url, "sp_test_term");
    let dir = tempdir().unwrap();
    let snap_dir = dir.path().join("cp_term");
    fs::create_dir_all(&snap_dir).unwrap();

    {
        let mut client = Client::connect(&db_url, NoTls).unwrap();
        client.batch_execute("CREATE TABLE t (x INT); INSERT INTO t VALUES (1);").unwrap();
    }

    // Keep an active client connection open
    let app_client = Client::connect(&db_url, NoTls).unwrap();

    // With terminate_connections = false, snapshot must fail cleanly if sessions exist
    let spec_no_term = make_pg_spec("pg_no_term", &db_url, false);
    let prov_no_term = PostgresProvider::new(spec_no_term);
    let err = prov_no_term.snapshot(&snap_dir).unwrap_err();
    assert!(err.to_string().contains("active connection") || err.to_string().contains("terminate"));

    // With terminate_connections = true, snapshot must succeed
    let spec_term = make_pg_spec("pg_term", &db_url, true);
    let prov_term = PostgresProvider::new(spec_term);
    prov_term.snapshot(&snap_dir).unwrap();
    prov_term.verify(&snap_dir).unwrap();

    drop(app_client);
    prov_term.discard(&snap_dir).unwrap();
}

#[test]
fn test_postgres_concurrent_reconnect_storm() {
    let Some(base_url) = get_pg_url() else {
        println!("SKIPPED: SP_TEST_PG_URL not set");
        return;
    };

    let db_url = create_fresh_db(&base_url, "sp_test_reconnect");
    let dir = tempdir().unwrap();
    let snap_dir = dir.path().join("cp_reconn");
    fs::create_dir_all(&snap_dir).unwrap();

    {
        let mut client = Client::connect(&db_url, NoTls).unwrap();
        client.batch_execute("CREATE TABLE counter (v INT); INSERT INTO counter VALUES (10);").unwrap();
    }

    let running = Arc::new(AtomicBool::new(true));
    let r_clone = running.clone();
    let u_clone = db_url.clone();

    // Reconnect storm: constantly opens connections and executes queries
    let storm_handle = thread::spawn(move || {
        while r_clone.load(Ordering::SeqCst) {
            if let Ok(mut c) = Client::connect(&u_clone, NoTls) {
                let _ = c.execute("SELECT 1;", &[]);
            }
            thread::sleep(Duration::from_millis(10));
        }
    });

    let spec = make_pg_spec("pg_storm", &db_url, true);
    let provider = PostgresProvider::new(spec);
    // Snapshot should retry and overcome reconnect storm
    provider.snapshot(&snap_dir).unwrap();

    running.store(false, Ordering::SeqCst);
    storm_handle.join().unwrap();

    provider.discard(&snap_dir).unwrap();
}

#[test]
fn test_postgres_discard_cleans_snapshot_database() {
    let Some(base_url) = get_pg_url() else {
        println!("SKIPPED: SP_TEST_PG_URL not set");
        return;
    };

    let db_url = create_fresh_db(&base_url, "sp_test_discard");
    let dir = tempdir().unwrap();
    let snap_dir = dir.path().join("cp_disc");
    fs::create_dir_all(&snap_dir).unwrap();

    {
        let mut client = Client::connect(&db_url, NoTls).unwrap();
        client.batch_execute("CREATE TABLE val (v TEXT); INSERT INTO val VALUES ('hello');").unwrap();
    }

    let spec = make_pg_spec("pg_disc", &db_url, true);
    let provider = PostgresProvider::new(spec);
    let manifest = provider.snapshot(&snap_dir).unwrap();
    let snap_db = manifest.metadata["snapshot_db"].as_str().unwrap();

    // Verify snapshot database exists in Postgres
    let mut root_client = Client::connect(&base_url, NoTls).unwrap();
    let exists: bool = root_client
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1);",
            &[&snap_db],
        )
        .unwrap()
        .get(0);
    assert!(exists, "Snapshot database must exist after snapshot()");

    // Call discard
    provider.discard(&snap_dir).unwrap();

    let exists_after: bool = root_client
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1);",
            &[&snap_db],
        )
        .unwrap()
        .get(0);
    assert!(!exists_after, "Snapshot database must be dropped after discard()");
}
