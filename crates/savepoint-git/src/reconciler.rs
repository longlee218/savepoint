use crate::error::{GitError, Result};
use crate::witness::GitWitness;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileStrategy {
    AutoSoftReset,
    ForkEphemeralBranch { tag: String },
    Decoupled,
}

#[derive(Debug, Clone)]
pub struct ReconcileResult {
    pub divergence_detected: bool,
    pub previous_head: Option<String>,
    pub reconciled_head: Option<String>,
    pub ephemeral_branch: Option<String>,
}

#[derive(Default)]
pub struct GitReconciler;

impl GitReconciler {
    pub fn new() -> Self {
        Self
    }

    pub fn reconcile(
        &self,
        workspace_root: &Path,
        target_witness: &GitWitness,
        strategy: ReconcileStrategy,
    ) -> Result<ReconcileResult> {
        let current_witness = GitWitness::capture(workspace_root)?;
        if !current_witness.is_git_repo || !target_witness.is_git_repo {
            return Ok(ReconcileResult {
                divergence_detected: false,
                previous_head: current_witness.head_commit,
                reconciled_head: target_witness.head_commit.clone(),
                ephemeral_branch: None,
            });
        }

        let Some(target_head) = &target_witness.head_commit else {
            return Ok(ReconcileResult {
                divergence_detected: false,
                previous_head: current_witness.head_commit,
                reconciled_head: None,
                ephemeral_branch: None,
            });
        };

        if current_witness.head_commit.as_ref() == Some(target_head) {
            return Ok(ReconcileResult {
                divergence_detected: false,
                previous_head: current_witness.head_commit,
                reconciled_head: Some(target_head.clone()),
                ephemeral_branch: None,
            });
        }

        // Divergence detected: HEAD moved since target checkpoint!
        let prev_head = current_witness.head_commit.clone();

        match strategy {
            ReconcileStrategy::Decoupled => Ok(ReconcileResult {
                divergence_detected: true,
                previous_head: prev_head,
                reconciled_head: current_witness.head_commit,
                ephemeral_branch: None,
            }),
            ReconcileStrategy::ForkEphemeralBranch { tag } => {
                let branch_name = format!("savepoint-orphaned-{}", tag);
                // Create branch pointing to current HEAD before resetting
                let out = Command::new("git")
                    .args(["branch", "-f", &branch_name])
                    .current_dir(workspace_root)
                    .output()
                    .map_err(|e| GitError::io(workspace_root, e))?;

                if !out.status.success() {
                    return Err(GitError::CommandFailed {
                        cmd: format!("git branch -f {}", branch_name),
                        code: out.status.code(),
                        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
                    });
                }

                // Now execute soft reset to target HEAD
                self.soft_reset(workspace_root, target_head)?;

                Ok(ReconcileResult {
                    divergence_detected: true,
                    previous_head: prev_head,
                    reconciled_head: Some(target_head.clone()),
                    ephemeral_branch: Some(branch_name),
                })
            }
            ReconcileStrategy::AutoSoftReset => {
                self.soft_reset(workspace_root, target_head)?;

                Ok(ReconcileResult {
                    divergence_detected: true,
                    previous_head: prev_head,
                    reconciled_head: Some(target_head.clone()),
                    ephemeral_branch: None,
                })
            }
        }
    }

    fn soft_reset(&self, workspace_root: &Path, commit_sha: &str) -> Result<()> {
        let out = Command::new("git")
            .args(["reset", "--soft", commit_sha])
            .current_dir(workspace_root)
            .output()
            .map_err(|e| GitError::io(workspace_root, e))?;

        if !out.status.success() {
            return Err(GitError::CommandFailed {
                cmd: format!("git reset --soft {}", commit_sha),
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            });
        }
        Ok(())
    }
}
