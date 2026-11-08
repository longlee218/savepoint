use savepoint_state::config::{ProviderKind, ProviderSpec};
use savepoint_state::glob::glob_match;
use savepoint_state::records::{read_records, RecordWriter};
use savepoint_state::StateProvider;
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn test_state_provider_kind_parsing() {
    let kinds = vec![
        ("sqlite", ProviderKind::Sqlite),
        ("postgres", ProviderKind::Postgres),
        ("mysql", ProviderKind::Mysql),
        ("redis", ProviderKind::Redis),
        ("memcached", ProviderKind::Memcached),
    ];
    for (name, expected) in kinds {
        let parsed: ProviderKind = serde_yaml::from_str(name).unwrap();
        assert_eq!(parsed, expected);
    }
}

#[test]
fn test_glob_pattern_matching_edge_cases() {
    assert!(glob_match(b"users:*:profile", b"users:123:profile"));
    assert!(glob_match(b"users:*:profile", b"users:alice_smith:profile"));
    assert!(!glob_match(b"users:*:profile", b"users:123:settings"));
    assert!(!glob_match(b"users:*:profile", b"orders:123:profile"));

    // Prefix match
    assert!(glob_match(b"cache:*", b"cache:session:1"));
    assert!(!glob_match(b"cache:*", b"other:session:1"));

    // Exact match
    assert!(glob_match(b"config:app_mode", b"config:app_mode"));
    assert!(!glob_match(b"config:app_mode", b"config:app_mode_v2"));
}

#[test]
fn test_record_writer_and_reader_roundtrip() {
    let dir = tempdir().unwrap();
    let record_file = dir.path().join("records.spstate");

    let mut writer = RecordWriter::new();
    // Write empty payload record
    writer.push(&[b"key_empty", b""]);

    // Write large binary payload record
    let large_binary = vec![0xEFu8; 64 * 1024];
    writer.push(&[b"key_binary", &large_binary, b"3600"]);

    writer.finish(&record_file).unwrap();

    // Read back
    let records = read_records(&record_file).unwrap();
    assert_eq!(records.len(), 2);

    assert_eq!(records[0][0], b"key_empty");
    assert!(records[0][1].is_empty());

    assert_eq!(records[1][0], b"key_binary");
    assert_eq!(records[1][1], large_binary);
    assert_eq!(records[1][2], b"3600");
}

#[test]
fn test_sqlite_preflight_and_snapshot_when_db_absent() {
    let dir = tempdir().unwrap();
    let absent_path = dir.path().join("absent.db");

    let spec = ProviderSpec {
        name: "test_sqlite".to_string(),
        kind: ProviderKind::Sqlite,
        url: None,
        path: Some(absent_path.clone()),
        include: Vec::new(),
        exclude: Vec::new(),
        terminate_connections: true,
        consistent: true,
        lock_timeout_secs: 10,
        maintenance_db: None,
    };

    let provider = savepoint_state::sqlite::SqliteProvider::new(spec, PathBuf::from(dir.path()));
    assert!(provider.preflight().is_ok());

    let snap_dir = dir.path().join("snap");
    let manifest = provider.snapshot(&snap_dir).unwrap();
    assert_eq!(manifest.metadata["exists"], false);
}
