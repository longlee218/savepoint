use crate::clone::{clone_tree, clone_workspace};
use crate::diff::compute_deltas;
use chrono::Utc;
use savepoint_core::{
    ChangeKind, Checkpoint, FileDelta, Result, SavepointError, Transaction, TransactionId,
    TransactionStatus,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub struct ShadowEngine;

impl ShadowEngine {
    /// Resolves the storage root for Savepoint data for a specific workspace.
    /// Routes by filesystem device ID (`st_dev`) so `clonefile(2)` / `FICLONE`
    /// never fail with `EXDEV`.
    pub fn resolve_storage_dir(workspace_root: &Path) -> PathBuf {
        let canonical = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        let hash = workspace_id_hash(&canonical.to_string_lossy());

        // Use custom root if SAVEPOINT_DIR is set (useful for tests)
        if let Ok(custom_dir) = std::env::var("SAVEPOINT_DIR") {
            return PathBuf::from(custom_dir).join("workspaces").join(hash);
        }

        // If workspace is inside OS temp directory (e.g. unit tests or ephemeral runs),
        // isolate storage inside a dedicated temp store so ~/.savepoint is never polluted!
        let temp_base = std::env::temp_dir();
        if canonical.starts_with(&temp_base)
            || canonical.starts_with("/private/var/folders")
            || canonical.starts_with("/var/folders")
            || canonical.starts_with("/tmp")
            || canonical
                .components()
                .any(|c| c.as_os_str().to_string_lossy().starts_with(".tmp"))
        {
            let test_savepoint = temp_base.join("savepoint-temp-spool").join("workspaces").join(&hash);
            let _ = fs::create_dir_all(&test_savepoint);
            return test_savepoint;
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
        Self::ensure_git_exclude_savepoint(&canonical);
        canonical.join(".savepoint").join("workspaces").join(hash)
    }

    /// Begins a new transaction for the given `workspace_root`.
    /// Immediately creates Snapshot 0 (`cp_0`) capturing the pristine state.
    pub fn begin_transaction(workspace_root: &Path) -> Result<Transaction> {
        let ws = workspace_root
            .canonicalize()
            .map_err(|e| SavepointError::Io {
                path: workspace_root.to_path_buf(),
                source: e,
            })?;

        let tx_id = TransactionId::new();
        let ws_storage = Self::resolve_storage_dir(&ws);

        // Automatically prune old finished transactions in this workspace (retention: 2)
        let _ = Self::prune_old_transactions(&ws, 2);

        let tx_dir = ws_storage.join("transactions").join(tx_id.as_str());
        let snapshots_dir = tx_dir.join("snapshots");

        fs::create_dir_all(&snapshots_dir).map_err(|e| SavepointError::Io {
            path: snapshots_dir.clone(),
            source: e,
        })?;

        let hooks = crate::hooks::LifecycleHooks::load(&ws);
        if let Some(h) = &hooks {
            let _ = crate::hooks::LifecycleHooks::run_hooks(
                &ws,
                "pre_checkpoint",
                &h.pre_checkpoint,
                &[("SAVEPOINT_TX_ID", tx_id.as_str()), ("SAVEPOINT_STEP", "0")],
            );
        }

        // Snapshot 0: baseline pristine state (skips .git and .savepoint)
        let cp0_path = snapshots_dir.join("cp_0");
        clone_workspace(&ws, &cp0_path)?;

        let cp0_state_dir = tx_dir.join("state").join("cp_0");
        if let Ok(Some(state_mgr)) = savepoint_state::StateManager::load(&ws) {
            state_mgr
                .preflight_all()
                .map_err(|e| SavepointError::Other(e.to_string()))?;
            state_mgr
                .snapshot_all(&cp0_state_dir)
                .map_err(|e| SavepointError::Other(e.to_string()))?;
        }

        if let Some(h) = &hooks {
            let _ = crate::hooks::LifecycleHooks::run_hooks(
                &ws,
                "post_checkpoint",
                &h.post_checkpoint,
                &[("SAVEPOINT_TX_ID", tx_id.as_str()), ("SAVEPOINT_STEP", "0")],
            );
        }

        let branch = Self::detect_git_branch(&ws).unwrap_or_else(|| "main".to_string());
        let content_hash = compute_checkpoint_hash(None, &branch, &[]);
        let initial_cp = Checkpoint {
            step_index: 0,
            timestamp: Utc::now(),
            snapshot_path: cp0_path,
            prompt: Some("Baseline pristine workspace".to_string()),
            summary: None,
            deltas: Vec::new(),
            branch: branch.clone(),
            parent_step: None,
            content_hash: Some(content_hash),
        };

        let mut tx = Transaction::with_branch(tx_id, ws, tx_dir, branch);
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
            return Err(SavepointError::TransactionFinished(
                tx.id.to_string(),
                tx.status,
            ));
        }

        let next_step = tx.next_step_index();
        let snapshots_dir = tx.shadow_root.join("snapshots");
        let current_cp_path = snapshots_dir.join(format!("cp_{}", next_step));

        let hooks = crate::hooks::LifecycleHooks::load(&tx.workspace_root);
        let next_step_str = next_step.to_string();
        if let Some(h) = &hooks {
            let _ = crate::hooks::LifecycleHooks::run_hooks(
                &tx.workspace_root,
                "pre_checkpoint",
                &h.pre_checkpoint,
                &[
                    ("SAVEPOINT_TX_ID", tx.id.as_str()),
                    ("SAVEPOINT_STEP", &next_step_str),
                ],
            );
        }

        // Take snapshot of current workspace state
        clone_workspace(&tx.workspace_root, &current_cp_path)?;

        let state_dir = tx
            .shadow_root
            .join("state")
            .join(format!("cp_{}", next_step));
        if let Ok(Some(state_mgr)) = savepoint_state::StateManager::load(&tx.workspace_root) {
            state_mgr
                .snapshot_all(&state_dir)
                .map_err(|e| SavepointError::Other(e.to_string()))?;
        }

        // Compute deltas from the checkpoint the workspace was based on
        let base_cp = tx
            .active_checkpoint()
            .or_else(|| tx.checkpoints.last())
            .ok_or_else(|| {
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

        if let Some(h) = &hooks {
            let _ = crate::hooks::LifecycleHooks::run_hooks(
                &tx.workspace_root,
                "post_checkpoint",
                &h.post_checkpoint,
                &[
                    ("SAVEPOINT_TX_ID", tx.id.as_str()),
                    ("SAVEPOINT_STEP", &next_step_str),
                ],
            );
        }

        Ok(cp)
    }

    /// Creates a new branch pointing to the current active checkpoint.
    pub fn fork_branch(tx: &mut Transaction, new_branch: &str) -> Result<()> {
        if tx.status != TransactionStatus::Active {
            return Err(SavepointError::TransactionFinished(
                tx.id.to_string(),
                tx.status,
            ));
        }
        tx.fork_branch(new_branch)?;
        Self::save_metadata(tx)?;
        Ok(())
    }

    /// Switches the workspace to the head of the target branch.
    pub fn switch_branch(tx: &mut Transaction, branch_name: &str) -> Result<usize> {
        if tx.status != TransactionStatus::Active {
            return Err(SavepointError::TransactionFinished(
                tx.id.to_string(),
                tx.status,
            ));
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
            return Err(SavepointError::TransactionFinished(
                tx.id.to_string(),
                tx.status,
            ));
        }
        let src_head = tx
            .branch_head(source_branch)
            .cloned()
            .ok_or_else(|| SavepointError::BranchNotFound(source_branch.to_string()))?;

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
            return Err(SavepointError::TransactionFinished(
                tx.id.to_string(),
                tx.status,
            ));
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
        let live_deltas = Self::compute_live_deltas(tx)?;
        if !live_deltas.is_empty() {
            Self::checkpoint(
                tx,
                Some(&format!(
                    "Auto-save before switching to Step #{}",
                    target_step
                )),
                None,
            )?;
        }

        let hooks = crate::hooks::LifecycleHooks::load(&tx.workspace_root);
        let target_step_str = target_step.to_string();
        if let Some(h) = &hooks {
            let _ = crate::hooks::LifecycleHooks::run_hooks(
                &tx.workspace_root,
                "pre_rollback",
                &h.pre_rollback,
                &[
                    ("SAVEPOINT_TX_ID", tx.id.as_str()),
                    ("SAVEPOINT_TARGET_STEP", &target_step_str),
                ],
            );
        }

        // 2. Perform delta rollback / fast-forward to target_step via CoW:
        let deltas = compute_deltas(&target_cp.snapshot_path, &tx.workspace_root)?;
        apply_snapshot_deltas(&target_cp.snapshot_path, &tx.workspace_root, &deltas)?;

        let target_state_dir = tx
            .shadow_root
            .join("state")
            .join(format!("cp_{}", target_step));
        if target_state_dir.exists() {
            if let Ok(Some(state_mgr)) = savepoint_state::StateManager::load(&tx.workspace_root) {
                state_mgr
                    .restore_all(&target_state_dir)
                    .map_err(|e| SavepointError::Other(e.to_string()))?;
            }
        }

        // 3. DO NOT PRUNE! Update current_step_index so the entire timeline remains intact!
        tx.current_step_index = Some(target_step);
        Self::save_metadata(tx)?;

        if let Some(h) = &hooks {
            let _ = crate::hooks::LifecycleHooks::run_hooks(
                &tx.workspace_root,
                "post_rollback",
                &h.post_rollback,
                &[
                    ("SAVEPOINT_TX_ID", tx.id.as_str()),
                    ("SAVEPOINT_TARGET_STEP", &target_step_str),
                ],
            );
        }

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
        let active_cp = tx
            .active_checkpoint()
            .ok_or_else(|| SavepointError::Other("Transaction has no checkpoints".to_string()))?;

        compute_deltas(&active_cp.snapshot_path, &tx.workspace_root)
    }

    /// Computes differences between two arbitrary checkpoints in the transaction.
    pub fn diff_checkpoints(
        tx: &Transaction,
        from_step: usize,
        to_step: usize,
    ) -> Result<Vec<FileDelta>> {
        let cp_from = tx
            .checkpoints
            .iter()
            .find(|c| c.step_index == from_step)
            .ok_or_else(|| SavepointError::CheckpointNotFound(from_step, tx.id.to_string()))?;
        let cp_to = tx
            .checkpoints
            .iter()
            .find(|c| c.step_index == to_step)
            .ok_or_else(|| SavepointError::CheckpointNotFound(to_step, tx.id.to_string()))?;

        compute_deltas(&cp_from.snapshot_path, &cp_to.snapshot_path)
    }

    /// Computes differences between a specific checkpoint and the current live workspace.
    pub fn diff_checkpoint_to_live(tx: &Transaction, from_step: usize) -> Result<Vec<FileDelta>> {
        let cp_from = tx
            .checkpoints
            .iter()
            .find(|c| c.step_index == from_step)
            .ok_or_else(|| SavepointError::CheckpointNotFound(from_step, tx.id.to_string()))?;

        compute_deltas(&cp_from.snapshot_path, &tx.workspace_root)
    }

    /// Squashes the active transaction into a Git commit in the workspace repository.
    pub fn squash_to_git(
        tx: &mut Transaction,
        message: Option<&str>,
        up_to_step: Option<usize>,
    ) -> Result<crate::git_bridge::GitCommitResult> {
        crate::git_bridge::squash_transaction_to_git(tx, message, up_to_step)
    }

    /// Commits the active transaction, cementing changes.
    pub fn commit(tx: &mut Transaction) -> Result<()> {
        if tx.status != TransactionStatus::Active {
            return Err(SavepointError::TransactionFinished(
                tx.id.to_string(),
                tx.status,
            ));
        }

        tx.status = TransactionStatus::Committed;
        Self::save_metadata(tx)?;

        // Automatically prune historical finished transactions in this workspace (retention: 2)
        let _ = Self::prune_old_transactions(&tx.workspace_root, 2);

        Ok(())
    }

    /// Prunes old completed/committed transactions in the workspace storage,
    /// keeping the currently active transaction plus at most `keep_last` recent transactions.
    pub fn prune_old_transactions(workspace_root: &Path, keep_last: usize) -> Result<usize> {
        let ws = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        let ws_storage = Self::resolve_storage_dir(&ws);
        let tx_root = ws_storage.join("transactions");
        if !tx_root.is_dir() {
            return Ok(0);
        }

        let active_id = fs::read_to_string(ws_storage.join("active_tx_id.txt"))
            .unwrap_or_default()
            .trim()
            .to_string();

        let mut entries = Vec::new();
        if let Ok(rd) = fs::read_dir(&tx_root) {
            for entry in rd.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let id = entry.file_name().to_string_lossy().to_string();
                if id == active_id {
                    continue; // Never prune currently active transaction
                }

                let meta_path = path.join("meta.json");
                let mtime = fs::metadata(&meta_path)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                entries.push((path, mtime));
            }
        }

        // Sort descending by modified time (newest first)
        entries.sort_by(|a, b| b.1.cmp(&a.1));

        let mut pruned = 0;
        // Keep the newest `keep_last`, remove the rest
        for (path, _) in entries.into_iter().skip(keep_last) {
            if fs::remove_dir_all(&path).is_ok() {
                pruned += 1;
            }
        }

        Ok(pruned)
    }

    /// Global Garbage Collector: scans ~/.savepoint/workspaces/ and removes:
    /// 1. Orphaned workspaces whose `workspace_root` no longer exists on disk (e.g. deleted tempdirs).
    /// 2. Inactive/committed transactions beyond `keep_last` in remaining workspaces.
    pub fn gc_orphaned_workspaces() -> Result<(usize, usize)> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let base_savepoint = if let Ok(custom) = std::env::var("SAVEPOINT_DIR") {
            PathBuf::from(custom).join("workspaces")
        } else {
            PathBuf::from(home).join(".savepoint").join("workspaces")
        };

        Self::gc_orphaned_workspaces_in_dir(&base_savepoint)
    }

    /// Garbage collects orphaned and obsolete workspaces in a specific root directory.
    pub fn gc_orphaned_workspaces_in_dir(base_savepoint: &Path) -> Result<(usize, usize)> {
        if !base_savepoint.is_dir() {
            return Ok((0, 0));
        }

        let mut orphaned_workspaces = 0;
        let mut pruned_transactions = 0;

        if let Ok(rd) = fs::read_dir(base_savepoint) {
            for entry in rd.flatten() {
                let ws_dir = entry.path();
                if !ws_dir.is_dir() {
                    continue;
                }

                // Check transactions in this workspace
                let tx_dir = ws_dir.join("transactions");
                let mut ws_root: Option<PathBuf> = None;

                if tx_dir.is_dir() {
                    if let Ok(tx_rd) = fs::read_dir(&tx_dir) {
                        for tx_entry in tx_rd.flatten() {
                            let meta_file = tx_entry.path().join("meta.json");
                            if let Ok(content) = fs::read_to_string(&meta_file) {
                                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                                    if let Some(r) = parsed.get("workspace_root").and_then(|v| v.as_str()) {
                                        ws_root = Some(PathBuf::from(r));
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }

                let is_orphaned = match &ws_root {
                    Some(root) => !root.exists(),
                    None => true, // No transactions or meta.json found
                };

                if is_orphaned {
                    if fs::remove_dir_all(&ws_dir).is_ok() {
                        orphaned_workspaces += 1;
                    }
                } else if let Some(root) = ws_root {
                    if let Ok(count) = Self::prune_old_transactions(&root, 2) {
                        pruned_transactions += count;
                    }
                }
            }
        }

        Ok((orphaned_workspaces, pruned_transactions))
    }

    /// Loads an existing active transaction from a workspace directory.
    pub fn load_active_transaction(workspace_root: &Path) -> Result<Option<Transaction>> {
        let ws = workspace_root
            .canonicalize()
            .map_err(|e| SavepointError::Io {
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
                                return Ok(Some(Self::sync_tx_branch_with_git(tx)));
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
        for entry in fs::read_dir(&tx_base).map_err(|e| SavepointError::Io {
            path: tx_base.clone(),
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

        Ok(active_tx.map(Self::sync_tx_branch_with_git))
    }

    /// Loads the active transaction if present, or falls back to the most recently created transaction
    /// (including `Committed` transactions) so `sp share` / `FlightRecorder::export_bundle` can export
    /// right after a session finishes.
    pub fn load_latest_transaction(workspace_root: &Path) -> Result<Option<Transaction>> {
        if let Some(active) = Self::load_active_transaction(workspace_root)? {
            return Ok(Some(active));
        }

        let ws = workspace_root
            .canonicalize()
            .map_err(|e| SavepointError::Io {
                path: workspace_root.to_path_buf(),
                source: e,
            })?;
        let ws_storage = Self::resolve_storage_dir(&ws);
        let tx_base = ws_storage.join("transactions");
        if !tx_base.exists() {
            return Ok(None);
        }

        let mut latest_tx: Option<Transaction> = None;
        for entry in fs::read_dir(&tx_base).map_err(|e| SavepointError::Io {
            path: tx_base.clone(),
            source: e,
        })? {
            let entry = entry.map_err(|e| SavepointError::Other(e.to_string()))?;
            let meta_file = entry.path().join("meta.json");
            if meta_file.exists() {
                if let Ok(content) = fs::read_to_string(&meta_file) {
                    if let Ok(tx) = serde_json::from_str::<Transaction>(&content) {
                        match &latest_tx {
                            Some(curr) if tx.created_at > curr.created_at => {
                                latest_tx = Some(tx);
                            }
                            None => {
                                latest_tx = Some(tx);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(latest_tx)
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
            let pointer_path = ws_storage.join("active_tx_id.txt");
            if tx.status == TransactionStatus::Active {
                let _ = fs::write(&pointer_path, tx.id.as_str());
            } else {
                let _ = fs::remove_file(&pointer_path);
            }
        }

        Ok(())
    }

    /// Ensures `.savepoint/` is listed in the repository's `.git/info/exclude`
    /// (supports both standard `.git` directories and git worktree/submodule `.git` files).
    pub fn ensure_git_exclude_savepoint(workspace_root: &Path) {
        let dot_git = workspace_root.join(".git");
        let git_dir = if dot_git.is_file() {
            fs::read_to_string(&dot_git).ok().and_then(|content| {
                content
                    .lines()
                    .find_map(|line| line.strip_prefix("gitdir:"))
                    .map(|p| {
                        let raw = PathBuf::from(p.trim());
                        if raw.is_absolute() {
                            raw
                        } else {
                            workspace_root.join(raw)
                        }
                    })
            })
        } else if dot_git.is_dir() {
            Some(dot_git)
        } else {
            None
        };

        let Some(git_dir) = git_dir else {
            return;
        };
        let exclude_path = git_dir.join("info").join("exclude");
        if let Some(parent) = exclude_path.parent() {
            if parent.exists() {
                let existing = fs::read_to_string(&exclude_path).unwrap_or_default();
                if !existing
                    .lines()
                    .any(|line| line.trim() == ".savepoint/" || line.trim() == ".savepoint")
                {
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

    /// Detects the active git branch for a workspace root.
    /// Reads `.git/HEAD` directly (fast path, works with standard repos and worktrees)
    /// and falls back to `git branch --show-current`.
    pub fn detect_git_branch(workspace_root: &Path) -> Option<String> {
        let dot_git = workspace_root.join(".git");
        let head_content = if dot_git.is_file() {
            fs::read_to_string(&dot_git).ok().and_then(|content| {
                content
                    .lines()
                    .find_map(|line| line.strip_prefix("gitdir:"))
                    .and_then(|p| {
                        let raw = PathBuf::from(p.trim());
                        let git_dir = if raw.is_absolute() {
                            raw
                        } else {
                            workspace_root.join(raw)
                        };
                        fs::read_to_string(git_dir.join("HEAD")).ok()
                    })
            })
        } else if dot_git.is_dir() {
            fs::read_to_string(dot_git.join("HEAD")).ok()
        } else {
            None
        };

        if let Some(head) = head_content {
            let trimmed = head.trim();
            if let Some(branch) = trimmed.strip_prefix("ref: refs/heads/") {
                let b = branch.trim().to_string();
                if !b.is_empty() {
                    return Some(b);
                }
            }
        }

        // Fallback to git CLI if .git format is non-standard or detached
        std::process::Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(workspace_root)
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if !s.is_empty() {
                        Some(s)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
    }

    fn sync_tx_branch_with_git(mut tx: Transaction) -> Transaction {
        if tx.active_branch == "main" && tx.branches.len() == 1 && tx.branches.contains_key("main")
        {
            if let Some(git_branch) = Self::detect_git_branch(&tx.workspace_root) {
                if git_branch != "main" {
                    let step = tx.branches.remove("main").unwrap_or(0);
                    tx.branches.insert(git_branch.clone(), step);
                    tx.active_branch = git_branch.clone();
                    for cp in &mut tx.checkpoints {
                        if cp.branch == "main" {
                            cp.branch = git_branch.clone();
                        }
                    }
                    let _ = Self::save_metadata(&tx);
                }
            }
        }
        tx
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
    let is_sqlite =
        name.ends_with(".db") || name.ends_with(".sqlite") || name.ends_with(".sqlite3");
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
    format!("{:x}", digest)
}

fn workspace_id_hash(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let digest = hasher.finalize();
    let hex = format!("{:x}", digest);
    hex[..16].to_string()
}
