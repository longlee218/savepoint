use std::fs;
use std::path::{Path, PathBuf};
use chrono::Utc;
use savepoint_core::{
    ChangeKind, Checkpoint, FileDelta, Result, SavepointError, Transaction, TransactionId, TransactionStatus,
};
use crate::clone::clone_tree;
use crate::diff::compute_deltas;

pub struct ShadowEngine;

impl ShadowEngine {
    /// Resolves the storage root for Savepoint data for a specific workspace.
    /// By default stores in `~/.savepoint/workspaces/<ws_hash>/` to keep the user
    /// repository 100% clean and prevent recursive APFS clone errors.
    pub fn resolve_storage_dir(workspace_root: &Path) -> PathBuf {
        let canonical = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());

        // Use custom root if SAVEPOINT_DIR is set (useful for tests)
        if let Ok(custom_dir) = std::env::var("SAVEPOINT_DIR") {
            let hash = format!("{:x}", md5_or_simple_hash(&canonical.to_string_lossy()));
            return PathBuf::from(custom_dir).join("workspaces").join(hash);
        }

        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let hash = format!("{:x}", md5_or_simple_hash(&canonical.to_string_lossy()));
        PathBuf::from(home)
            .join(".savepoint")
            .join("workspaces")
            .join(hash)
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

        // Snapshot 0: baseline pristine state
        let cp0_path = snapshots_dir.join("cp_0");
        clone_tree(&ws, &cp0_path)?;

        let initial_cp = Checkpoint {
            step_index: 0,
            timestamp: Utc::now(),
            snapshot_path: cp0_path,
            prompt: Some("Baseline pristine workspace".to_string()),
            summary: None,
            deltas: Vec::new(),
        };

        let mut tx = Transaction::new(tx_id, ws, tx_dir);
        tx.checkpoints.push(initial_cp);
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

        let next_step = tx.current_step();
        let snapshots_dir = tx.shadow_root.join("snapshots");
        let current_cp_path = snapshots_dir.join(format!("cp_{}", next_step));

        // Take snapshot of current workspace state
        clone_tree(&tx.workspace_root, &current_cp_path)?;

        // Compute deltas from the checkpoint the workspace was based on
        let base_cp = tx.active_checkpoint().or_else(|| tx.checkpoints.last()).ok_or_else(|| {
            SavepointError::Other("Transaction has no previous checkpoint".to_string())
        })?;
        let deltas = compute_deltas(&base_cp.snapshot_path, &current_cp_path)?;

        let cp = Checkpoint {
            step_index: next_step,
            timestamp: Utc::now(),
            snapshot_path: current_cp_path,
            prompt: prompt.map(|s| s.to_string()),
            summary: summary.map(|s| s.to_string()),
            deltas,
        };

        tx.checkpoints.push(cp.clone());
        tx.current_step_index = Some(next_step);
        Self::save_metadata(tx)?;

        Ok(cp)
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

        // 2. Perform delta rollback / fast-forward to target_step:
        let deltas = compute_deltas(&target_cp.snapshot_path, &tx.workspace_root)?;

        for delta in deltas {
            let target_file_in_ws = tx.workspace_root.join(&delta.relative_path);
            let snapshot_file = target_cp.snapshot_path.join(&delta.relative_path);

            match delta.kind {
                ChangeKind::Created => {
                    // File exists in workspace but not in target snapshot -> delete it
                    if target_file_in_ws.exists() {
                        if target_file_in_ws.is_dir() {
                            let _ = fs::remove_dir_all(&target_file_in_ws);
                        } else {
                            let _ = fs::remove_file(&target_file_in_ws);
                        }
                    }
                }
                ChangeKind::Modified | ChangeKind::Deleted => {
                    // File was modified or deleted -> restore exact copy from snapshot
                    if let Some(parent) = target_file_in_ws.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    if snapshot_file.exists() {
                        fs::copy(&snapshot_file, &target_file_in_ws).map_err(|e| SavepointError::Io {
                            path: target_file_in_ws.clone(),
                            source: e,
                        })?;
                    }
                }
            }
        }

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

fn md5_or_simple_hash(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}
