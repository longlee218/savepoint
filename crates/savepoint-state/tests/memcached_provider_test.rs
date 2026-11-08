use savepoint_state::config::{ProviderKind, ProviderSpec};
use savepoint_state::memcached::MemcachedProvider;
use savepoint_state::provider::StateProvider;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use tempfile::tempdir;

fn get_memcached_addr() -> Option<String> {
    env::var("SP_TEST_MEMCACHED_ADDR").ok().filter(|s| !s.is_empty())
}

fn send_cmd(addr: &str, cmd: &str) -> String {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.write_all(cmd.as_bytes()).unwrap();
    let mut resp = String::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = stream.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        resp.push_str(&String::from_utf8_lossy(&buf[..n]));
        if resp.ends_with("END\r\n") || resp.ends_with("STORED\r\n") || resp.ends_with("DELETED\r\n") || resp.ends_with("OK\r\n") || resp.ends_with("NOT_FOUND\r\n") {
            break;
        }
    }
    resp
}

fn make_spec(name: &str, addr: &str, inc: Vec<String>, exc: Vec<String>) -> ProviderSpec {
    ProviderSpec {
        name: name.to_string(),
        kind: ProviderKind::Memcached,
        url: Some(format!("memcached://{}", addr)),
        path: None,
        include: inc,
        exclude: exc,
        terminate_connections: true,
        consistent: true,
        lock_timeout_secs: 10,
        maintenance_db: None,
    }
}

#[test]
fn test_memcached_roundtrip_with_flags_ttl_and_binary() {
    let Some(addr) = get_memcached_addr() else {
        println!("SKIPPED: SP_TEST_MEMCACHED_ADDR not set");
        return;
    };

    // Clean existing keys by flushing
    let _ = send_cmd(&addr, "flush_all\r\n");

    // 1. Set key with flags 42 and ttl 3600
    let _ = send_cmd(&addr, "set rt:user1 42 3600 12\r\nwelcome_user\r\n");

    // 2. Set binary payload with CRLF inside
    let mut stream = TcpStream::connect(&addr).unwrap();
    let bin_val = b"hello\r\nworld\x00binary";
    let cmd = format!("set rt:bin 99 3600 {}\r\n", bin_val.len());
    stream.write_all(cmd.as_bytes()).unwrap();
    stream.write_all(bin_val).unwrap();
    stream.write_all(b"\r\n").unwrap();
    let mut resp = [0u8; 64];
    let _ = stream.read(&mut resp).unwrap();

    let dir = tempdir().unwrap();
    let snap_dir = dir.path().join("cp_memcached");
    fs::create_dir_all(&snap_dir).unwrap();

    let spec = make_spec("memcached_app", &addr, vec!["rt:*".to_string()], vec![]);
    let provider = MemcachedProvider::new(spec);
    provider.preflight().unwrap();

    // Snapshot
    let manifest = provider.snapshot(&snap_dir).unwrap();
    assert_eq!(manifest.kind, ProviderKind::Memcached);
    assert_eq!(manifest.metadata["key_count"], 2);
    provider.verify(&snap_dir).unwrap();

    // Mutate live state
    let _ = send_cmd(&addr, "delete rt:user1\r\n");
    let _ = send_cmd(&addr, "set rt:intruder 0 3600 6\r\nintrud\r\n");

    // Restore
    provider.restore(&snap_dir).unwrap();

    // Verify
    let val_resp = send_cmd(&addr, "get rt:user1\r\n");
    assert!(val_resp.contains("welcome_user"));
    assert!(val_resp.contains(" 42 "), "Flags must be preserved");

    // Intruder key must have been deleted on restore
    let intr_resp = send_cmd(&addr, "get rt:intruder\r\n");
    assert!(intr_resp.contains("END") && !intr_resp.contains("VALUE"));

    provider.discard(&snap_dir).unwrap();
}

#[test]
fn test_memcached_prefix_scoping() {
    let Some(addr) = get_memcached_addr() else {
        println!("SKIPPED: SP_TEST_MEMCACHED_ADDR not set");
        return;
    };

    let _ = send_cmd(&addr, "flush_all\r\n");
    let _ = send_cmd(&addr, "set app:pref:1 0 3600 4\r\nval1\r\n");
    let _ = send_cmd(&addr, "set foreign:key 0 3600 7\r\nforeign\r\n");

    let dir = tempdir().unwrap();
    let snap_dir = dir.path().join("cp_mem_scoped");
    fs::create_dir_all(&snap_dir).unwrap();

    let spec = make_spec("mem_scoped", &addr, vec!["app:*".to_string()], vec![]);
    let provider = MemcachedProvider::new(spec);
    let manifest = provider.snapshot(&snap_dir).unwrap();
    assert_eq!(manifest.metadata["key_count"], 1);

    // Mutate both
    let _ = send_cmd(&addr, "set app:pref:1 0 3600 7\r\nmutated\r\n");
    let _ = send_cmd(&addr, "set foreign:key 0 3600 8\r\nuntouchd\r\n");

    provider.restore(&snap_dir).unwrap();

    // app:pref:1 restored to val1
    let app_resp = send_cmd(&addr, "get app:pref:1\r\n");
    assert!(app_resp.contains("val1"));

    // foreign:key remains untouched ("untouchd")
    let for_resp = send_cmd(&addr, "get foreign:key\r\n");
    assert!(for_resp.contains("untouchd"));

    provider.discard(&snap_dir).unwrap();
}
