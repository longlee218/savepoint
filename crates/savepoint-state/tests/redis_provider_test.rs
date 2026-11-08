use redis::Commands;
use savepoint_state::config::{ProviderKind, ProviderSpec};
use savepoint_state::provider::StateProvider;
use savepoint_state::redis::RedisProvider;
use std::env;
use std::fs;
use tempfile::tempdir;

fn get_redis_urls() -> Vec<String> {
    env::var("SP_TEST_REDIS_URLS")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn make_redis_spec(name: &str, url: &str, include: Vec<String>, exclude: Vec<String>) -> ProviderSpec {
    ProviderSpec {
        name: name.to_string(),
        kind: ProviderKind::Redis,
        url: Some(url.to_string()),
        path: None,
        include,
        exclude,
        terminate_connections: true,
        consistent: true,
        lock_timeout_secs: 10,
        maintenance_db: None,
    }
}

#[test]
fn test_redis_all_core_data_types_roundtrip() {
    let urls = get_redis_urls();
    if urls.is_empty() {
        println!("SKIPPED: SP_TEST_REDIS_URLS not set");
        return;
    }

    for (idx, url) in urls.iter().enumerate() {
        // Use database 3 for this test
        let db_url = format!("{}/3", url.rsplit_once('/').map(|(b, _)| b).unwrap_or(url));
        let client = redis::Client::open(db_url.as_str()).unwrap();
        let mut conn = client.get_connection().unwrap();
        let _: () = redis::cmd("FLUSHDB").query(&mut conn).unwrap();

        // 1. Setup string, list, set, zset, hash, streams, binary key
        let _: () = conn.set("str_key", "hello redis").unwrap();
        let _: () = conn.rpush("list_key", &["item1", "item2", "item3"]).unwrap();
        let _: () = conn.sadd("set_key", &["alpha", "beta", "gamma"]).unwrap();
        let _: () = conn.zadd("zset_key", "first", 10).unwrap();
        let _: () = conn.zadd("zset_key", "second", 20).unwrap();
        let _: () = conn.hset_multiple("hash_key", &[("field1", "val1"), ("field2", "val2")]).unwrap();
        
        // TTL key (5 minutes)
        let _: () = conn.set_ex("expiring_key", "will_expire", 300).unwrap();

        // Binary key and binary value (all bytes 0..=255)
        let bin_key = b"bin\x00key\r\n";
        let bin_val: Vec<u8> = (0u8..=255).collect();
        let _: () = conn.set(bin_key, bin_val.as_slice()).unwrap();

        let dir = tempdir().unwrap();
        let snap_dir = dir.path().join("cp_redis");
        fs::create_dir_all(&snap_dir).unwrap();

        let spec = make_redis_spec(&format!("redis_app_{}", idx), &db_url, vec![], vec![]);
        let provider = RedisProvider::new(spec);
        provider.preflight().unwrap();

        // 2. Snapshot
        let manifest = provider.snapshot(&snap_dir).unwrap();
        assert_eq!(manifest.kind, ProviderKind::Redis);
        assert_eq!(manifest.metadata["key_count"], 7);
        provider.verify(&snap_dir).unwrap();

        // 3. Mutate live DB (delete keys, change values, add new keys)
        let _: () = conn.del("str_key").unwrap();
        let _: () = conn.rpush("list_key", "unexpected_item").unwrap();
        let _: () = conn.set("intruder_key", "should_be_deleted_on_restore").unwrap();

        // 4. Restore
        provider.restore(&snap_dir).unwrap();

        // 5. Verify restored state
        let restored_str: String = conn.get("str_key").unwrap();
        assert_eq!(restored_str, "hello redis");

        let list_items: Vec<String> = conn.lrange("list_key", 0, -1).unwrap();
        assert_eq!(list_items, vec!["item1", "item2", "item3"]);

        let z_card: usize = conn.zcard("zset_key").unwrap();
        assert_eq!(z_card, 2);

        let restored_bin: Vec<u8> = conn.get(bin_key).unwrap();
        assert_eq!(restored_bin, bin_val);

        // Intruder key must have been deleted
        let exists: bool = conn.exists("intruder_key").unwrap();
        assert!(!exists, "New keys created after checkpoint must be deleted on restore");

        // Expiring key must still have TTL > 0
        let pttl: i64 = redis::cmd("PTTL").arg("expiring_key").query(&mut conn).unwrap();
        assert!(pttl > 0 && pttl <= 300_000, "TTL must be preserved: got {}", pttl);

        provider.discard(&snap_dir).unwrap();
    }
}

#[test]
fn test_redis_include_exclude_scoping() {
    let urls = get_redis_urls();
    if urls.is_empty() {
        println!("SKIPPED: SP_TEST_REDIS_URLS not set");
        return;
    }

    let base_url = &urls[0];
    let db_url = format!("{}/4", base_url.rsplit_once('/').map(|(b, _)| b).unwrap_or(base_url));
    let client = redis::Client::open(db_url.as_str()).unwrap();
    let mut conn = client.get_connection().unwrap();
    let _: () = redis::cmd("FLUSHDB").query(&mut conn).unwrap();

    let _: () = conn.set("app:session:1", "session1").unwrap();
    let _: () = conn.set("app:cache:user1", "user1_cache").unwrap();
    let _: () = conn.set("bull:job:999", "ghost_job").unwrap();

    let dir = tempdir().unwrap();
    let snap_dir = dir.path().join("cp_scope");
    fs::create_dir_all(&snap_dir).unwrap();

    // Include app:* but exclude bull:*
    let spec = make_redis_spec(
        "redis_scoped",
        &db_url,
        vec!["app:*".to_string()],
        vec!["bull:*".to_string()],
    );
    let provider = RedisProvider::new(spec);
    let manifest = provider.snapshot(&snap_dir).unwrap();
    assert_eq!(manifest.metadata["key_count"], 2, "Only app:* keys should be captured");

    // Mutate
    let _: () = conn.set("app:session:1", "mutated").unwrap();
    let _: () = conn.set("bull:job:999", "bull_job_updated").unwrap();

    provider.restore(&snap_dir).unwrap();

    // app:session:1 should be restored to session1
    let s: String = conn.get("app:session:1").unwrap();
    assert_eq!(s, "session1");

    // bull:job:999 was out of scope, so it should stay untouched ("bull_job_updated")
    let b: String = conn.get("bull:job:999").unwrap();
    assert_eq!(b, "bull_job_updated", "Excluded keys must NOT be overwritten on restore");

    provider.discard(&snap_dir).unwrap();
}
