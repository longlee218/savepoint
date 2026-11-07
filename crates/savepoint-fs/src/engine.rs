use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use chrono::Utc;
use sha2::{Digest, Sha256};
use savepoint_core::{
    ChangeKind, Checkpoint, FileDelta, Result, SavepointError, Transaction, TransactionId, TransactionStatus,
};
use crate::clone::{clone_tree, clone_workspace};
use crate::diff::compute_deltas;

pub struct ShadowEngine;

impl ShadowEngine {
    /// Resolves the storage root for Savepoint data for a specific workspace.
    /// Routes by filesystem device ID (`st_dev`) so `clonefile(2)` / `FICLONE`
    /// never fail with `EXDEV`.
    pub fn resolve_storage_dir(workspace_root: &Path) -> PathBuf {
        let canonical = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        let hash = format!("{:x}", md5_or_simple_hash(&canonical.to_string_lossy()));

        // Use custom root if SAVEPOINT_DIR is set (useful for tests)
        if let Ok(custom_dir) = std::env::var("SAVEPOINT_DIR") {
            return PathBuf::from(custom_dir).join("workspaces").join(hash);
        }

        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let home_savepoint = PathBuf::from(home).join(".savepoint");
        let _ = fs::create_dir_all(&home_savepoint);

        let ws_dev = fs::metadata(&canonical).map(|m| m.dev()).ok();
        let home_dev = fs::metadata(&home_savepoint).map(|m| m.dev()).ok();

        // Case A: Same filesystem volume as ~/.savepoint
        if ws_dev.is_some() && ws_dev == home_dev {
            return home_savepoint.join("workspaces").join(hash);
        }

        // Case B: External volume — try sibling directory <workspace_parent>/.savepoint-spool/<hash>
        if let Some(parent) = canonical.parent() {
            let sibling_spool = parent.join(".savepoint-spool");
            if fs::create_dir_all(&sibling_spool).is_ok() {
                let sibling_dev = fs::metadata(&sibling_spool).map(|m| m.dev()).ok();
                if ws_dev.is_some() && ws_dev == sibling_dev {
                    return sibling_spool.join(&hash);
                }
            }
        }

        // Case C: Fallback inside workspace <.savepoint/workspaces/<hash>> with .git/info/exclude
        ensure_git_exclude_savepoint(&canonical);
        canonical.join(".savepoint").join("workspaces").join(hash)
    }

    /// Begins a new transaction for the given `workspace_root`.
    /// Immediately creates Snapshot 0 (`cp_0`) capturing the pristine state.
    pub fn begin_transaction(workspace_root: &Path) -> Result<Transaction> {
        let ws = workspace_root.canonicalize().map_err(|e| SavepointError::Io {
            path: workspace_root.to_path_buf(),
            source: e,
        })?;

        let tx_id = TransactionId::new();
        let ws_storage = Self::resolve_storage_dir(&ws);
        let tx_dir = ws_storage.join("transactions").join(tx_id.as_str());
        let snapshots_dir = tx_dir.join("snapshots");

        fs::create_dir_all(&snapshots_dir).map_err(|e| SavepointError::Io {
            path: snapshots_dir.clone(),
            source: e,
        })?;

        // Snapshot 0: baseline pristine state (skips .git and .savepoint)
        let cp0_path = snapshots_dir.join("cp_0");
        clone_workspace(&ws, &cp0_path)?;

        let content_hash = compute_checkpoint_hash(None, "main", &[]);
        let initial_cp = Checkpoint {
            step_index: 0,
            timestamp: Utc::now(),
            snapshot_path: cp0_path,
            prompt: Some("Baseline pristine workspace".to_string()),
            summary: None,
            deltas: Vec::new(),
            branch: "main".to_string(),
            parent_step: None,
            content_hash: Some(content_hash),
        };

        let mut tx = Transaction::new(tx_id, ws, tx_dir);
        tx.record_checkpoint_in_branch(initial_cp);
        Self::save_metadata(&tx)?;

        Ok(tx)
    }

    /// Captures a new checkpoint for the current turn.
    pub fn checkpoint(
        tx: &mut Transaction,
        prompt: Option<&str>,
        summary: Option<&str>,
    ) -> Result<Checkpoint> {
        if tx.status != TransactionStatus::Active {
            return Err(SavepointError::TransactionFinished(tx.id.to_string(), tx.status));
        }

        let next_step = tx.next_step_index();
        let snapshots_dir = tx.shadow_root.join("snapshots");
        let current_cp_path = snapshots_dir.join(format!("cp_{}", next_step));

        // Take snapshot of current workspace state
        clone_workspace(&tx.workspace_root, &current_cp_path)?;

        // Compute deltas from the checkpoint the workspace was based on
        let base_cp = tx.active_checkpoint().or_else(|| tx.checkpoints.last()).ok_or_else(|| {
            SavepointError::Other("Transaction has no previous checkpoint".to_string())
        })?;
        let parent_step = Some(base_cp.step_index);
        let branch = tx.active_branch.clone();
        let deltas = compute_deltas(&base_cp.snapshot_path, &current_cp_path)?;
        let content_hash = compute_checkpoint_hash(parent_step, &branch, &deltas);

        let cp = Checkpoint {
            step_index: next_step,
            timestamp: Utc::now(),
            snapshot_path: current_cp_path,
            prompt: prompt.map(|s| s.to_string()),
            summary: summary.map(|s| s.to_string()),
            deltas,
            branch,
            parent_step,
            content_hash: Some(content_hash),
        };

        tx.record_checkpoint_in_branch(cp.clone());
        Self::save_metadata(tx)?;

        Ok(cp)
    }

    /// Creates a new branch pointing to the current active checkpoint.
    pub fn fork_branch(tx: &mut Transaction, new_branch: &str) -> Result<()> {
        if tx.status != TransactionStatus::Active {
            return Err(SavepointError::TransactionFinished(tx.id.to_string(), tx.status));
        }
        tx.fork_branch(new_branch)?;
        Self::save_metadata(tx)?;
        Ok(())
    }

    /// Switches the workspace to the head of the target branch.
    pub fn switch_branch(tx: &mut Transaction, branch_name: &str) -> Result<usize> {
        if tx.status != TransactionStatus::Active {
            return Err(SavepointError::TransactionFinished(tx.id.to_string(), tx.status));
        }
        let target_step = tx.switch_branch(branch_name)?;
        Self::rewind(tx, target_step)?;
        Ok(target_step)
    }

    /// Merges changes from source_branch into target_branch.
    pub fn merge_branch(
        tx: &mut Transaction,
        source_branch: &str,
        target_branch: &str,
    ) -> Result<Checkpoint> {
        if tx.status != TransactionStatus::Active {
            return Err(SavepointError::TransactionFinished(tx.id.to_string(), tx.status));
        }
        let src_head = tx.branch_head(source_branch).cloned().ok_or_else(|| {
            SavepointError::BranchNotFound(source_branch.to_string())
        })?;

        // Switch to target branch if not active
        if tx.active_branch != target_branch {
            Self::switch_branch(tx, target_branch)?;
        }

        // Apply diff from source snapshot onto current target workspace via CoW
        let deltas = compute_deltas(&src_head.snapshot_path, &tx.workspace_root)?;
        apply_snapshot_deltas(&src_head.snapshot_path, &tx.workspace_root, &deltas)?;

        let summary = format!("Merged branch '{}' into '{}'", source_branch, target_branch);
        Self::checkpoint(tx, Some(&summary), Some(&summary))
    }

    /// Rewinds or fast-forwards the workspace to `target_step` in milliseconds.
    /// Non-destructive: preserves all future/past checkpoints and saves live edits if present.
    pub fn rewind(tx: &mut Transaction, target_step: usize) -> Result<()> {
        if tx.status != TransactionStatus::Active {
            return Err(SavepointError::TransactionFinished(tx.id.to_string(), tx.status));
        }

        let target_cp = tx
            .checkpoints
            .iter()
            .find(|cp| cp.step_index == target_step)
            .ok_or_else(|| SavepointError::CheckpointNotFound(target_step, tx.id.to_string()))?
            .clone();

        // 1. SAFETY-NET CHECKPOINT:
        // Before jumping, check if current workspace has uncommitted changes!
        // If it does, freeze a safety checkpoint so user NEVER loses code!
        if let Ok(live_deltas) = Self::compute_live_deltas(tx) {
            if !live_deltas.is_empty() {
                let _ = Self::checkpoint(
                    tx,
                    Some(&format!("Tự động lưu: Trước khi chuyển sang Bước #{}", target_step)),
                    None,
                );
            }
        }

        // 2. Perform delta rollback / fast-forward to target_step via CoW:
        let deltas = compute_deltas(&target_cp.snapshot_path, &tx.workspace_root)?;
        apply_snapshot_deltas(&target_cp.snapshot_path, &tx.workspace_root, &deltas)?;

        // 3. DO NOT PRUNE! Update current_step_index so the entire timeline remains intact!
        tx.current_step_index = Some(target_step);
        Self::save_metadata(tx)?;

        Ok(())
    }

    /// Rewinds one step back (to the previous checkpoint before the active step).
    /// Returns the target step index that was restored, or None if already at baseline.
    pub fn rewind_previous(tx: &mut Transaction) -> Result<Option<usize>> {
        let current_step = tx.active_step();
        if current_step == 0 {
            return Ok(None);
        }

        let prev_cp = tx
            .checkpoints
            .iter()
            .filter(|cp| cp.step_index < current_step)
            .max_by_key(|cp| cp.step_index);

        if let Some(target_cp) = prev_cp {
            let target_step = target_cp.step_index;
            Self::rewind(tx, target_step)?;
            Ok(Some(target_step))
        } else {
            Ok(None)
        }
    }

    /// Computes differences between the active checkpoint and the current live workspace.
    pub fn compute_live_deltas(tx: &Transaction) -> Result<Vec<FileDelta>> {
        let active_cp = tx.active_checkpoint().ok_or_else(|| {
            SavepointError::Other("Transaction has no checkpoints".to_string())
        })?;

        compute_deltas(&active_cp.snapshot_path, &tx.workspace_root)
    }

    /// Commits the active transaction, cementing changes.
    pub fn commit(tx: &mut Transaction) -> Result<()> {
        if tx.status != TransactionStatus::Active {
            return Err(SavepointError::TransactionFinished(tx.id.to_string(), tx.status));
        }

        tx.status = TransactionStatus::Committed;
        Self::save_metadata(tx)?;

        Ok(())
    }

    /// Loads an existing active transaction from a workspace directory.
    pub fn load_active_transaction(workspace_root: &Path) -> Result<Option<Transaction>> {
        let ws = workspace_root.canonicalize().map_err(|e| SavepointError::Io {
            path: workspace_root.to_path_buf(),
            source: e,
        })?;

        let ws_storage = Self::resolve_storage_dir(&ws);

        // Check active pointer file first
        let pointer_file = ws_storage.join("active_tx_id.txt");
        if pointer_file.exists() {
            if let Ok(tx_id_str) = fs::read_to_string(&pointer_file) {
                let tx_dir = ws_storage.join("transactions").join(tx_id_str.trim());
                let meta_file = tx_dir.join("meta.json");
                if meta_file.exists() {
                    if let Ok(content) = fs::read_to_string(&meta_file) {
                        if let Ok(tx) = serde_json::from_str::<Transaction>(&content) {
                            if tx.status == TransactionStatus::Active {
                                return Ok(Some(tx));
                            }
                        }
                    }
                }
            }
        }

        let tx_base = ws_storage.join("transactions");
        if !tx_base.exists() {
            return Ok(None);
        }

        // Fallback: Find the newest active transaction sorted by created_at
        let mut active_tx: Option<Transaction> = None;
        for entry in fs::read_dir(tx_base).map_err(|e| SavepointError::Io {
            path: ws.clone(),
            source: e,
        })? {
            let entry = entry.map_err(|e| SavepointError::Other(e.to_string()))?;
            let meta_file = entry.path().join("meta.json");
            if meta_file.exists() {
                if let Ok(content) = fs::read_to_string(&meta_file) {
                    if let Ok(tx) = serde_json::from_str::<Transaction>(&content) {
                        if tx.status == TransactionStatus::Active {
                            match &active_tx {
                                Some(curr) if tx.created_at > curr.created_at => {
                                    active_tx = Some(tx);
                                }
                                None => {
                                    active_tx = Some(tx);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        Ok(active_tx)
    }

    fn save_metadata(tx: &Transaction) -> Result<()> {
        let meta_path = tx.shadow_root.join("meta.json");
        let json = serde_json::to_string_pretty(tx)?;
        fs::write(&meta_path, json).map_err(|e| SavepointError::Io {
            path: meta_path,
            source: e,
        })?;

        // Update active pointer
        if let Some(ws_storage) = tx.shadow_root.parent().and_then(|p| p.parent()) {
            if tx.status == TransactionStatus::Active {
                let _ = fs::write(ws_storage.join("active_tx_id.txt"), tx.id.as_str());
            }
        }

        Ok(())
    }
}

fn apply_snapshot_deltas(
    snapshot_path: &Path,
    workspace_root: &Path,
    deltas: &[FileDelta],
) -> Result<()> {
    for delta in deltas {
        let target_file_in_ws = workspace_root.join(&delta.relative_path);
        let snapshot_file = snapshot_path.join(&delta.relative_path);

        match delta.kind {
            ChangeKind::Created => {
                // Path exists in workspace but not in target snapshot -> remove it
                if let Ok(meta) = target_file_in_ws.symlink_metadata() {
                    if meta.is_dir() && !meta.file_type().is_symlink() {
                        let _ = fs::remove_dir_all(&target_file_in_ws);
                    } else {
                        let _ = fs::remove_file(&target_file_in_ws);
                    }
                }
            }
            ChangeKind::Modified | ChangeKind::Deleted => {
                // Path was modified or deleted -> restore via CoW clone_tree from snapshot
                if snapshot_file.symlink_metadata().is_ok() {
                    clone_tree(&snapshot_file, &target_file_in_ws)?;
                }
            }
        }

        cleanup_orphaned_sqlite_companions(&delta.relative_path, snapshot_path, workspace_root);
    }

    Ok(())
}

fn cleanup_orphaned_sqlite_companions(
    rel_path: &Path,
    snapshot_path: &Path,
    workspace_root: &Path,
) {
    let Some(name) = rel_path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let is_sqlite = name.ends_with(".db") || name.ends_with(".sqlite") || name.ends_with(".sqlite3");
    if !is_sqlite {
        return;
    }

    for suffix in ["-wal", "-shm"] {
        let companion_name = format!("{}{}", name, suffix);
        let companion_rel = match rel_path.parent() {
            Some(parent) => parent.join(companion_name),
            None => PathBuf::from(companion_name),
        };
        let snap_companion = snapshot_path.join(&companion_rel);
        let ws_companion = workspace_root.join(&companion_rel);

        if snap_companion.symlink_metadata().is_err() && ws_companion.symlink_metadata().is_ok() {
            let _ = fs::remove_file(&ws_companion);
        }
    }
}

fn compute_checkpoint_hash(
    parent_step: Option<usize>,
    branch: &str,
    deltas: &[FileDelta],
) -> String {
    let mut hasher = Sha256::new();
    match parent_step {
        Some(p) => hasher.update(format!("parent:{}\n", p).as_bytes()),
        None => hasher.update(b"parent:root\n"),
    }
    hasher.update(format!("branch:{}\n", branch).as_bytes());
    for d in deltas {
        hasher.update(
            format!(
                "delta:{}:{:?}:{:?}:{:?}\n",
                d.relative_path.to_string_lossy(),
                d.kind,
                d.old_size,
                d.new_size
            )
            .as_bytes(),
        );
    }
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{:02x}", b)).collect()
}

fn ensure_git_exclude_savepoint(workspace_root: &Path) {
    let exclude_path = workspace_root.join(".git").join("info").join("exclude");
    if let Some(parent) = exclude_path.parent() {
        if parent.exists() {
            let existing = fs::read_to_string(&exclude_path).unwrap_or_default();
            if !existing.lines().any(|line| line.trim() == ".savepoint/" || line.trim() == ".savepoint") {
                let mut updated = existing;
                if !updated.ends_with('\n') && !updated.is_empty() {
                    updated.push('\n');
                }
                updated.push_str(".savepoint/\n");
                let _ = fs::write(&exclude_path, updated);
            }
        }
    }
}

fn md5_or_simple_hash(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}
