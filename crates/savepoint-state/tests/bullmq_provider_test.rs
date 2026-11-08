use savepoint_state::config::{ProviderKind, ProviderSpec};
use savepoint_state::provider::StateProvider;
use savepoint_state::redis::RedisProvider;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

fn get_bullmq_redis_url() -> String {
    env::var("SP_TEST_BULLMQ_REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:56379/5".to_string())
}

fn node_harness(args: &[&str]) -> String {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let harness_path = manifest_dir.join("tests/bullmq/harness.mjs");
    let output = Command::new("node")
        .arg(&harness_path)
        .args(args)
        .env("SP_TEST_BULLMQ_REDIS_URL", get_bullmq_redis_url())
        .output()
        .expect("Failed to execute node harness. Ensure node is installed.");

    if !output.status.success() {
        panic!(
            "Node harness failed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn flush_redis_db(url: &str) {
    if let Ok(client) = redis::Client::open(url) {
        if let Ok(mut conn) = client.get_connection() {
            let _: () = redis::cmd("FLUSHDB").query(&mut conn).unwrap();
        }
    }
}

#[test]
fn test_bullmq_ghost_jobs_dropped_on_rewind() {
    let redis_url = get_bullmq_redis_url();
    flush_redis_db(&redis_url);

    // 1. Pre-checkpoint: Add legitimate job 1
    node_harness(&["add", "reports_queue", "legit_report", "{\"report_id\":101}"]);

    let counts_pre = node_harness(&["counts", "reports_queue"]);
    assert!(counts_pre.contains("\"waiting\":1"), "Must have 1 waiting job before checkpoint");

    let dir = tempdir().unwrap();
    let snap_dir = dir.path().join("cp1");
    fs::create_dir_all(&snap_dir).unwrap();

    let spec = ProviderSpec {
        name: "bullmq_redis".to_string(),
        kind: ProviderKind::Redis,
        url: Some(redis_url.clone()),
        path: None,
        include: vec![],
        exclude: vec![],
        terminate_connections: true,
        consistent: true,
        lock_timeout_secs: 10,
        maintenance_db: None,
    };

    let provider = RedisProvider::new(spec);
    // 2. Snapshot CP1
    provider.snapshot(&snap_dir).unwrap();

    // 3. Post-checkpoint: Agent queues 2 speculative jobs (1 immediate, 1 delayed)
    node_harness(&["add", "reports_queue", "ghost_job_instant", "{\"ghost\":true}"]);
    node_harness(&["add", "reports_queue", "ghost_job_delayed", "{\"ghost\":true}", "60000"]);

    let counts_post = node_harness(&["counts", "reports_queue"]);
    assert!(counts_post.contains("\"waiting\":2"), "Should have 2 waiting jobs");
    assert!(counts_post.contains("\"delayed\":1"), "Should have 1 delayed job");

    // 4. Agent fails & rewinds back to CP1
    provider.restore(&snap_dir).unwrap();

    // Verify queue counts immediately after rewind
    let counts_reverted = node_harness(&["counts", "reports_queue"]);
    assert!(counts_reverted.contains("\"waiting\":1"), "Ghost waiting jobs must be eradicated");
    assert!(counts_reverted.contains("\"delayed\":0"), "Ghost delayed jobs must be eradicated");

    // 5. Worker consumes the queue
    let results_file = dir.path().join("worker_results.json");
    node_harness(&[
        "drain",
        "reports_queue",
        results_file.to_str().unwrap(),
        "2000",
    ]);

    let results_json = fs::read_to_string(&results_file).unwrap();
    let processed: Vec<serde_json::Value> = serde_json::from_str(&results_json).unwrap();

    // Assert: ONLY legit_report executed!
    assert_eq!(processed.len(), 1, "Only pre-checkpoint job must be executed");
    assert_eq!(processed[0]["name"], "legit_report");

    provider.discard(&snap_dir).unwrap();
}

#[test]
fn test_bullmq_exclude_leaves_queue_intact() {
    let redis_url = get_bullmq_redis_url();
    flush_redis_db(&redis_url);

    // Add job 1
    node_harness(&["add", "audit_queue", "audit_1", "{}"]);

    let dir = tempdir().unwrap();
    let snap_dir = dir.path().join("cp_exclude");
    fs::create_dir_all(&snap_dir).unwrap();

    // Exclude bullmq queue keys
    let spec = ProviderSpec {
        name: "bullmq_excluded".to_string(),
        kind: ProviderKind::Redis,
        url: Some(redis_url),
        path: None,
        include: vec![],
        exclude: vec!["bull:*".to_string()],
        terminate_connections: true,
        consistent: true,
        lock_timeout_secs: 10,
        maintenance_db: None,
    };

    let provider = RedisProvider::new(spec);
    provider.snapshot(&snap_dir).unwrap();

    // Add job 2 post-checkpoint
    node_harness(&["add", "audit_queue", "audit_2", "{}"]);

    // Rewind
    provider.restore(&snap_dir).unwrap();

    // Because bull:* was excluded, audit_2 must STILL exist in the queue
    let counts = node_harness(&["counts", "audit_queue"]);
    assert!(
        counts.contains("\"waiting\":2"),
        "When excluded from rewind, queue jobs must remain untouched"
    );

    provider.discard(&snap_dir).unwrap();
}
