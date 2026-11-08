use savepoint_core::{Result, SavepointError, Transaction};
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCommitResult {
    pub commit_hash: String,
    pub commit_message: String,
    pub steps_squashed: usize,
    pub files_changed: usize,
}

pub fn squash_transaction_to_git(
    tx: &mut Transaction,
    custom_message: Option<&str>,
    up_to_step: Option<usize>,
) -> Result<GitCommitResult> {
    let ws = tx.workspace_root.clone();

    // 1. Verify workspace is inside a git repository
    let is_git = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(&ws)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !is_git {
        return Err(SavepointError::Other(format!(
            "Workspace {:?} is not a valid git repository",
            ws
        )));
    }

    // 2. If target step specified, rewind first if needed
    if let Some(target) = up_to_step {
        if target != tx.active_step() {
            crate::engine::ShadowEngine::rewind(tx, target)?;
        }
    }

    // 3. Count squashed steps and changed files
    let squashed_steps: Vec<_> = tx
        .checkpoints
        .iter()
        .filter(|cp| cp.step_index > 0 && cp.step_index <= tx.active_step())
        .collect();

    let steps_count = squashed_steps.len();
    let total_deltas: usize = squashed_steps.iter().map(|cp| cp.deltas.len()).sum();

    // 4. Generate commit message if not provided
    let commit_message = match custom_message {
        Some(msg) => msg.to_string(),
        None => {
            let mut lines = Vec::new();
            lines.push(format!(
                "savepoint: squash transaction {} ({} steps)",
                tx.id, steps_count
            ));
            lines.push("".to_string());
            for cp in &squashed_steps {
                let desc = cp.prompt.as_deref().unwrap_or("Checkpoint");
                lines.push(format!(
                    "- Step #{:02}: {} ({} files)",
                    cp.step_index,
                    desc,
                    cp.deltas.len()
                ));
            }
            lines.join("\n")
        }
    };

    // 5. Run git add -A
    let add_output = Command::new("git")
        .args(["add", "-A"])
        .current_dir(&ws)
        .output()
        .map_err(|e| SavepointError::Other(format!("Failed to run git add: {}", e)))?;

    if !add_output.status.success() {
        return Err(SavepointError::Other(format!(
            "git add failed: {}",
            String::from_utf8_lossy(&add_output.stderr)
        )));
    }

    // 6. Run git commit -m <message>
    let commit_output = Command::new("git")
        .args(["commit", "-m", &commit_message])
        .current_dir(&ws)
        .output()
        .map_err(|e| SavepointError::Other(format!("Failed to run git commit: {}", e)))?;

    if !commit_output.status.success() {
        let stderr = String::from_utf8_lossy(&commit_output.stderr);
        let stdout = String::from_utf8_lossy(&commit_output.stdout);
        return Err(SavepointError::Other(format!(
            "git commit failed: {}\n{}",
            stderr, stdout
        )));
    }

    // 7. Get commit hash
    let rev_output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(&ws)
        .output()
        .map_err(|e| SavepointError::Other(format!("Failed to run git rev-parse: {}", e)))?;

    let commit_hash = String::from_utf8_lossy(&rev_output.stdout)
        .trim()
        .to_string();

    // 8. Commit Savepoint transaction so working tree and transaction pointer are finalized
    crate::engine::ShadowEngine::commit(tx)?;

    Ok(GitCommitResult {
        commit_hash,
        commit_message,
        steps_squashed: steps_count,
        files_changed: total_deltas,
    })
}
