use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitWitness {
    pub is_git_repo: bool,
    pub head_commit: Option<String>,
    pub current_branch: Option<String>,
    pub is_detached: bool,
    pub dirty_files_count: usize,
}

impl GitWitness {
    pub fn capture(workspace_root: &Path) -> Result<Self> {
        let dot_git = workspace_root.join(".git");
        if !dot_git.exists() {
            return Ok(Self {
                is_git_repo: false,
                head_commit: None,
                current_branch: None,
                is_detached: false,
                dirty_files_count: 0,
            });
        }

        let is_repo = Command::new("git")
            .args(["rev-parse", "--is-inside-work-tree"])
            .current_dir(workspace_root)
            .output();

        let Ok(out) = is_repo else {
            return Ok(Self {
                is_git_repo: false,
                head_commit: None,
                current_branch: None,
                is_detached: false,
                dirty_files_count: 0,
            });
        };

        if !out.status.success() {
            return Ok(Self {
                is_git_repo: false,
                head_commit: None,
                current_branch: None,
                is_detached: false,
                dirty_files_count: 0,
            });
        }

        // HEAD commit
        let head_commit = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(workspace_root)
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                } else {
                    None
                }
            });

        // Branch name
        let branch_out = Command::new("git")
            .args(["symbolic-ref", "--short", "HEAD"])
            .current_dir(workspace_root)
            .output();

        let (current_branch, is_detached) = match branch_out {
            Ok(o) if o.status.success() => {
                let name = String::from_utf8_lossy(&o.stdout).trim().to_string();
                (Some(name), false)
            }
            _ => (None, true),
        };

        // Status porcelain count
        let status_out = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(workspace_root)
            .output();

        let dirty_files_count = match status_out {
            Ok(o) if o.status.success() => {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .count()
            }
            _ => 0,
        };

        Ok(Self {
            is_git_repo: true,
            head_commit,
            current_branch,
            is_detached,
            dirty_files_count,
        })
    }
}
