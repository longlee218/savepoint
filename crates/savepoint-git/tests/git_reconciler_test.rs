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
fn test_git_split_brain_reconciles_via_soft_reset() {
    let dir = tempdir().unwrap();
    let repo_dir = dir.path();

    run_git(repo_dir, &["init"]);
    run_git(repo_dir, &["config", "user.email", "test@test.com"]);
    run_git(repo_dir, &["config", "user.name", "Test"]);

    fs::write(repo_dir.join("main.rs"), "fn main() { println!(\"v1\"); }").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "Initial commit"]);

    // 1. Capture GitWitness at Checkpoint 1
    let witness_1 = GitWitness::capture(repo_dir).unwrap();
    assert!(witness_1.head_commit.is_some());

    // 2. Agent modifies file and makes intermediate commit (Checkpoint 2)
    fs::write(repo_dir.join("main.rs"), "fn main() { println!(\"v2 with commit\"); }").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "Agent intermediate commit"]);

    let witness_2 = GitWitness::capture(repo_dir).unwrap();
    assert_ne!(witness_1.head_commit, witness_2.head_commit);

    // 3. Reconcile back to Checkpoint 1 using Auto Soft-Reset
    let reconciler = GitReconciler::new();
    let res = reconciler
        .reconcile(repo_dir, &witness_1, ReconcileStrategy::AutoSoftReset)
        .unwrap();
    assert!(res.divergence_detected);

    // 4. Verify HEAD has returned to Initial commit without losing unstaged work
    let current_witness = GitWitness::capture(repo_dir).unwrap();
    assert_eq!(current_witness.head_commit, witness_1.head_commit);
}

#[test]
fn test_git_reconciler_fork_ephemeral_branch_preserves_abandoned_commit() {
    let dir = tempdir().unwrap();
    let repo_dir = dir.path();

    run_git(repo_dir, &["init"]);
    run_git(repo_dir, &["config", "user.email", "test@test.com"]);
    run_git(repo_dir, &["config", "user.name", "Test"]);

    fs::write(repo_dir.join("main.rs"), "v1").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "C1"]);

    let witness_1 = GitWitness::capture(repo_dir).unwrap();

    fs::write(repo_dir.join("main.rs"), "v2").unwrap();
    run_git(repo_dir, &["add", "."]);
    run_git(repo_dir, &["commit", "-m", "C2"]);
    let witness_2 = GitWitness::capture(repo_dir).unwrap();
    let abandoned_commit = witness_2.head_commit.clone().unwrap();

    let reconciler = GitReconciler::new();
    let res = reconciler
        .reconcile(
            repo_dir,
            &witness_1,
            ReconcileStrategy::ForkEphemeralBranch {
                tag: "tx_orphan_99".to_string(),
            },
        )
        .unwrap();

    assert!(res.divergence_detected);
    assert!(res.ephemeral_branch.is_some());

    // Verify orphaned branch exists and points to C2
    let branch_commit = run_git(
        repo_dir,
        &["rev-parse", "refs/heads/savepoint-orphaned-tx_orphan_99"],
    );
    assert_eq!(branch_commit, abandoned_commit);

    // Verify HEAD is at C1
    let current_witness = GitWitness::capture(repo_dir).unwrap();
    assert_eq!(current_witness.head_commit, witness_1.head_commit);
}
