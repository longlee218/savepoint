use mysql::prelude::*;
use mysql::{Conn, Opts};
use savepoint_state::config::{ProviderKind, ProviderSpec};
use savepoint_state::mysql::MysqlProvider;
use savepoint_state::provider::StateProvider;
use std::env;
use std::fs;
use tempfile::tempdir;

fn get_mysql_urls() -> Vec<String> {
    env::var("SP_TEST_MYSQL_URLS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn make_mysql_spec(name: &str, url: &str) -> ProviderSpec {
    ProviderSpec {
        name: name.to_string(),
        kind: ProviderKind::Mysql,
        url: Some(url.to_string()),
        path: None,
        include: vec![],
        exclude: vec![],
        terminate_connections: true,
        consistent: true,
        lock_timeout_secs: 15,
        maintenance_db: None,
    }
}

fn create_fresh_mysql_db(url: &str, db_name: &str) -> (String, Opts) {
    let opts = Opts::from_url(url).unwrap();
    let root_opts = Opts::from_url(&format!(
        "mysql://{}:{}@{}:{}/mysql",
        opts.get_user().unwrap_or("root"),
        opts.get_pass().unwrap_or(""),
        opts.get_ip_or_hostname(),
        opts.get_tcp_port()
    ))
    .unwrap();

    let mut conn = Conn::new(root_opts).unwrap();
    let quoted = format!("`{}`", db_name.replace('`', "``"));
    let _ = conn.query_drop(format!("DROP DATABASE IF EXISTS {};", quoted));
    conn.query_drop(format!("CREATE DATABASE {} CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;", quoted)).unwrap();

    let db_url = format!(
        "mysql://{}:{}@{}:{}/{}",
        opts.get_user().unwrap_or("root"),
        opts.get_pass().unwrap_or(""),
        opts.get_ip_or_hostname(),
        opts.get_tcp_port(),
        db_name
    );
    (db_url, Opts::from_url(url).unwrap())
}

#[test]
fn test_mysql_and_mariadb_full_roundtrip() {
    let urls = get_mysql_urls();
    if urls.is_empty() {
        println!("SKIPPED: SP_TEST_MYSQL_URLS not set");
        return;
    }

    for (idx, base_url) in urls.iter().enumerate() {
        let db_name = format!("sp_test_roundtrip_{}", idx);
        let (db_url, _) = create_fresh_mysql_db(base_url, &db_name);
        let dir = tempdir().unwrap();
        let snap_dir = dir.path().join("cp_mysql");
        fs::create_dir_all(&snap_dir).unwrap();

        // 1. Setup schema with tables, FKs, JSON, triggers, auto_increment, emoji
        {
            let mut conn = Conn::new(Opts::from_url(&db_url).unwrap()).unwrap();
            conn.query_drop(
                "CREATE TABLE categories (
                     id INT AUTO_INCREMENT PRIMARY KEY,
                     title VARCHAR(100) NOT NULL
                 ) ENGINE=InnoDB;",
            )
            .unwrap();
            conn.query_drop(
                "CREATE TABLE products (
                     id INT AUTO_INCREMENT PRIMARY KEY,
                     cat_id INT,
                     sku VARCHAR(50) NOT NULL,
                     details JSON,
                     rating INT DEFAULT 5,
                     created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                     FOREIGN KEY (cat_id) REFERENCES categories(id) ON DELETE CASCADE
                 ) ENGINE=InnoDB;",
            )
            .unwrap();
            conn.query_drop(
                "CREATE TRIGGER trg_prod_upper BEFORE INSERT ON products
                 FOR EACH ROW
                 SET NEW.sku = UPPER(NEW.sku);",
            )
            .unwrap();

            conn.query_drop("INSERT INTO categories (title) VALUES ('Electronics ⚡️'), ('Books 📚');").unwrap();
            conn.query_drop(
                "INSERT INTO products (cat_id, sku, details) VALUES 
                 (1, 'mbp16', '{\"ram\": 64, \"color\": \"space black\"}'),
                 (2, 'rust_book', '{\"pages\": 500}');",
            )
            .unwrap();
        }

        let spec = make_mysql_spec(&format!("mysql_app_{}", idx), &db_url);
        let provider = MysqlProvider::new(spec);
        provider.preflight().unwrap();

        // 2. Snapshot
        let manifest = provider.snapshot(&snap_dir).unwrap();
        assert_eq!(manifest.kind, ProviderKind::Mysql);
        provider.verify(&snap_dir).unwrap();

        // 3. Mutate: add table, alter table, delete rows
        {
            let mut conn = Conn::new(Opts::from_url(&db_url).unwrap()).unwrap();
            conn.query_drop("DELETE FROM categories WHERE id = 1;").unwrap(); // cascades to product 1
            conn.query_drop("INSERT INTO categories (title) VALUES ('New Cat');").unwrap();
            conn.query_drop("ALTER TABLE products ADD COLUMN discount INT DEFAULT 0;").unwrap();
        }

        // 4. Restore
        provider.restore(&snap_dir).unwrap();

        // 5. Verify restored state
        {
            let mut conn = Conn::new(Opts::from_url(&db_url).unwrap()).unwrap();
            let cat_titles: Vec<String> = conn.query_map("SELECT title FROM categories ORDER BY id", |t: String| t).unwrap();
            assert_eq!(cat_titles, vec!["Electronics ⚡️", "Books 📚"]);

            let prod_skus: Vec<String> = conn.query_map("SELECT sku FROM products ORDER BY id", |s: String| s).unwrap();
            assert_eq!(prod_skus, vec!["MBP16", "RUST_BOOK"]);

            // Ensure discount column is gone
            assert!(conn.query_drop("SELECT discount FROM products").is_err());
        }

        // 6. Discard
        provider.discard(&snap_dir).unwrap();
    }
}

#[test]
fn test_mysql_auto_increment_and_view_preservation() {
    let urls = get_mysql_urls();
    if urls.is_empty() {
        println!("SKIPPED: SP_TEST_MYSQL_URLS not set");
        return;
    }

    for (idx, base_url) in urls.iter().enumerate() {
        let db_name = format!("sp_test_view_{}", idx);
        let (db_url, _) = create_fresh_mysql_db(base_url, &db_name);
        let dir = tempdir().unwrap();
        let snap_dir = dir.path().join("cp_view");
        fs::create_dir_all(&snap_dir).unwrap();

        {
            let mut conn = Conn::new(Opts::from_url(&db_url).unwrap()).unwrap();
            conn.query_drop("CREATE TABLE items (id INT AUTO_INCREMENT PRIMARY KEY, v INT);").unwrap();
            conn.query_drop("INSERT INTO items (v) VALUES (10), (20), (30);").unwrap();
            conn.query_drop("CREATE VIEW items_view AS SELECT id, v * 2 AS doubled FROM items;").unwrap();
        }

        let spec = make_mysql_spec(&format!("mysql_view_{}", idx), &db_url);
        let provider = MysqlProvider::new(spec);
        provider.snapshot(&snap_dir).unwrap();

        // Mutate live db
        {
            let mut conn = Conn::new(Opts::from_url(&db_url).unwrap()).unwrap();
            conn.query_drop("DROP VIEW items_view;").unwrap();
            conn.query_drop("INSERT INTO items (v) VALUES (40);").unwrap();
        }

        provider.restore(&snap_dir).unwrap();

        {
            let mut conn = Conn::new(Opts::from_url(&db_url).unwrap()).unwrap();
            // Verify view restored
            let doubled: Vec<i32> = conn.query_map("SELECT doubled FROM items_view ORDER BY id", |d: i32| d).unwrap();
            assert_eq!(doubled, vec![20, 40, 60]);

            // Insert new row, verify auto_increment resumes correctly
            conn.query_drop("INSERT INTO items (v) VALUES (99);").unwrap();
            let new_id: i32 = conn.query_first("SELECT id FROM items WHERE v = 99").unwrap().unwrap();
            assert_eq!(new_id, 4, "Auto-increment must resume after restored row count");
        }

        provider.discard(&snap_dir).unwrap();
    }
}
