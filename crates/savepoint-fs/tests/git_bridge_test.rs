use savepoint_fs::ShadowEngine;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn test_git_bridge_squash_to_git() {
    let dir = tempdir().unwrap();
    let ws = dir.path().canonicalize().unwrap();

    // 1. Initialize git repo
    let status = Command::new("git")
        .args(["init"])
        .current_dir(&ws)
        .status()
        .unwrap();
    assert!(status.success());

    // Configure user name and email so git commit works in tests
    let _ = Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(&ws)
        .status();
    let _ = Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(&ws)
        .status();

    // Baseline commit in git
    fs::write(ws.join("README.md"), "# Project\n").unwrap();
    let _ = Command::new("git").args(["add", "README.md"]).current_dir(&ws).status();
    let _ = Command::new("git").args(["commit", "-m", "initial commit"]).current_dir(&ws).status();

    // 2. Start Savepoint transaction
    let mut tx = ShadowEngine::begin_transaction(&ws).unwrap();

    // Step 1: create file1
    fs::write(ws.join("file1.rs"), "fn foo() {}").unwrap();
    ShadowEngine::checkpoint(&mut tx, Some("Add file1"), None).unwrap();

    // Step 2: create file2
    fs::write(ws.join("file2.rs"), "fn bar() {}").unwrap();
    ShadowEngine::checkpoint(&mut tx, Some("Add file2"), None).unwrap();

    // 3. Squash transaction to git
    let git_res = ShadowEngine::squash_to_git(&mut tx, Some("feat: add file1 and file2"), None).unwrap();
    assert!(!git_res.commit_hash.is_empty());
    assert_eq!(git_res.steps_squashed, 2);

    // Verify git log has this commit
    let output = Command::new("git")
        .args(["log", "-1", "--pretty=%s"])
        .current_dir(&ws)
        .output()
        .unwrap();
    let commit_subject = String::from_utf8_lossy(&output.stdout);
    assert_eq!(commit_subject.trim(), "feat: add file1 and file2");

    // Verify Savepoint transaction is now Committed
    assert_eq!(tx.status, savepoint_core::TransactionStatus::Committed);
}
