use savepoint_git::{GitReconciler, GitWitness, ReconcileStrategy};
use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn run_git(repo: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git must execute");
    if !out.status.success() {
        panic!(
            "git {:?} failed: stderr={}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn test_witness_non_git_directory() {
    let dir = tempdir().unwrap();
    let witness = GitWitness::capture(dir.path()).unwrap();
    assert!(!witness.is_git_repo);
    assert!(witness.head_commit.is_none());
    assert!(witness.current_branch.is_none());
    assert_eq!(witness.dirty_files_count, 0);
}

#[test]
fn test_witness_unborn_head_empty_repo() {
    let dir = tempdir().unwrap();
    run_git(dir.path(), &["init"]);

    let witness = GitWitness::capture(dir.path()).unwrap();
    assert!(witness.is_git_repo);
    assert!(witness.head_commit.is_none());
    assert_eq!(witness.dirty_files_count, 0);
}

#[test]
fn test_witness_detects_dirty_files() {
    let dir = tempdir().unwrap();
    let repo_dir = dir.path();
    run_git(repo_dir, &["init"]);
    run_git(repo_dir, &["config", "user.email", "test@test.com"]);
    run_git(repo_dir, &["config", "user.name", "Test"]);

    fs::write(repo_dir.join("tracked.txt"), "hello").unwrap();
    run_git(repo_dir, &["add", "tracked.txt"]);
    run_git(repo_dir, &["commit", "-m", "Initial"]);

    // Add untracked and modify tracked
    fs::write(repo_dir.join("untracked.txt"), "ghost").unwrap();
    fs::write(repo_dir.join("tracked.txt"), "hello modified").unwrap();

    let witness = GitWitness::capture(repo_dir).unwrap();
    assert!(witness.is_git_repo);
    assert!(witness.head_commit.is_some());
    assert!(witness.dirty_files_count >= 2);
}

#[test]
fn test_witness_detached_head_detection() {
    let dir = tempdir().unwrap();
    let repo_dir = dir.path();
    run_git(repo_dir, &["init"]);
    run_git(repo_dir, &["config", "user.email", "test@test.com"]);
    run_git(repo_dir, &["config", "user.name", "Test"]);

    fs::write(repo_dir.join("file.txt"), "v1").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "v1"]);

    // Checkout detach
    run_git(repo_dir, &["checkout", "--detach", "HEAD"]);

    let witness = GitWitness::capture(repo_dir).unwrap();
    assert!(witness.is_detached);
    assert!(witness.head_commit.is_some());
}

#[test]
fn test_reconciler_no_divergence_when_head_matches() {
    let dir = tempdir().unwrap();
    let repo_dir = dir.path();
    run_git(repo_dir, &["init"]);
    run_git(repo_dir, &["config", "user.email", "test@test.com"]);
    run_git(repo_dir, &["config", "user.name", "Test"]);

    fs::write(repo_dir.join("file.txt"), "v1").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "v1"]);

    let witness = GitWitness::capture(repo_dir).unwrap();
    let reconciler = GitReconciler::new();
    let res = reconciler
        .reconcile(repo_dir, &witness, ReconcileStrategy::AutoSoftReset)
        .unwrap();

    assert!(!res.divergence_detected);
    assert!(res.ephemeral_branch.is_none());
}

#[test]
fn test_reconciler_ignores_non_git_repo() {
    let dir = tempdir().unwrap();
    let witness = GitWitness::capture(dir.path()).unwrap();
    let reconciler = GitReconciler::new();
    let res = reconciler
        .reconcile(dir.path(), &witness, ReconcileStrategy::AutoSoftReset)
        .unwrap();

    assert!(!res.divergence_detected);
}
