use savepoint_fs::{LifecycleHooks, ShadowEngine};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_lifecycle_hooks_discovery_and_execution() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();

    // 1. Write an .sp-hooks.yaml file
    let hooks_yaml = r#"
pre_checkpoint:
  - "echo pre_cp > hook_pre_cp.txt"
post_checkpoint:
  - "echo post_cp > hook_post_cp.txt"
pre_rollback:
  - "mkdir -p .sp-tmp && echo pre_rb > .sp-tmp/hook_pre_rb.txt"
post_rollback:
  - "echo post_rb > hook_post_rb.txt"
"#;
    fs::write(ws.join(".sp-hooks.yaml"), hooks_yaml).unwrap();

    let loaded = LifecycleHooks::load(&ws);
    assert!(loaded.is_some(), "Should load .sp-hooks.yaml");
    let hooks = loaded.unwrap();
    assert_eq!(hooks.pre_checkpoint.len(), 1);
    assert_eq!(hooks.post_checkpoint.len(), 1);
    assert_eq!(hooks.pre_rollback.len(), 1);
    assert_eq!(hooks.post_rollback.len(), 1);

    // 2. Start transaction
    let mut tx = ShadowEngine::begin_transaction(&ws).unwrap();
    assert!(ws.join("hook_pre_cp.txt").exists(), "Pre-checkpoint hook should have executed on step 0");
    assert!(ws.join("hook_post_cp.txt").exists(), "Post-checkpoint hook should have executed on step 0");

    // Clean up marker files
    let _ = fs::remove_file(ws.join("hook_pre_cp.txt"));
    let _ = fs::remove_file(ws.join("hook_post_cp.txt"));

    // 3. Create a step 1
    fs::write(ws.join("hello.txt"), "hello v1").unwrap();
    let _ = ShadowEngine::checkpoint(&mut tx, Some("Step 1 edit"), None).unwrap();
    assert!(ws.join("hook_pre_cp.txt").exists());
    assert!(ws.join("hook_post_cp.txt").exists());

    // 4. Test rollback hooks
    assert!(!ws.join(".sp-tmp").join("hook_pre_rb.txt").exists());
    assert!(!ws.join("hook_post_rb.txt").exists());

    ShadowEngine::rewind(&mut tx, 0).unwrap();
    assert!(ws.join(".sp-tmp").join("hook_pre_rb.txt").exists(), "Pre-rollback hook should run");
    assert!(ws.join("hook_post_rb.txt").exists(), "Post-rollback hook should run");
}
