use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;
use savepoint_core::{Result, SavepointError, Transaction};
use crate::engine::ShadowEngine;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlightBundle {
    pub format_version: String,
    pub created_at: DateTime<Utc>,
    pub scrubbed_files_count: usize,
    pub scrubbed_secrets_count: usize,
    pub transaction: Transaction,
    /// Map of step_index -> (relative_path_string -> BundleFileState)
    pub step_files: BTreeMap<usize, BTreeMap<String, BundleFileState>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BundleFileState {
    Present {
        content: String,
        is_hex: bool,
    },
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleExportStats {
    pub transaction_id: String,
    pub output_path: PathBuf,
    pub checkpoints_count: usize,
    pub tracked_files_count: usize,
    pub scrubbed_files_count: usize,
    pub scrubbed_secrets_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleReplayStats {
    pub transaction_id: String,
    pub replayed_step: usize,
    pub restored_files_count: usize,
}

pub struct FlightRecorder;

impl FlightRecorder {
    /// Exports the active or latest committed transaction for `workspace_root` into a portable `.spbundle` file,
    /// automatically scrubbing sensitive files (`.env*`, keys, certs) and inline API tokens.
    pub fn export_bundle(workspace_root: &Path, output_path: &Path) -> Result<BundleExportStats> {
        let mut tx = ShadowEngine::load_latest_transaction(workspace_root)?.ok_or_else(|| {
            SavepointError::Other(format!(
                "No Savepoint transaction found for workspace {:?}",
                workspace_root
            ))
        })?;

        let mut candidate_paths: BTreeSet<PathBuf> = BTreeSet::new();
        let mut sensitive_paths: BTreeSet<PathBuf> = BTreeSet::new();

        // 1. Collect all relative paths from deltas across all checkpoints
        for cp in &tx.checkpoints {
            for delta in &cp.deltas {
                if is_sensitive_secret_path(&delta.relative_path) {
                    sensitive_paths.insert(delta.relative_path.clone());
                } else {
                    candidate_paths.insert(delta.relative_path.clone());
                }
            }
        }

        // Also scan cp_0 snapshot for any sensitive files that were present at baseline
        // so scrubbed_files_count accurately reflects excluded baseline secret files (e.g. .env)
        if let Some(cp0) = tx.checkpoints.first() {
            if cp0.snapshot_path.exists() {
                for entry in WalkDir::new(&cp0.snapshot_path).follow_links(false).into_iter().flatten() {
                    if entry.file_type().is_file() {
                        if let Ok(rel) = entry.path().strip_prefix(&cp0.snapshot_path) {
                            if is_sensitive_secret_path(rel) {
                                sensitive_paths.insert(rel.to_path_buf());
                            }
                        }
                    }
                }
            }
        }

        let mut scrubbed_secrets_count = 0usize;

        // 2. Scrub sensitive deltas and scrub prompts/summaries in the exported Transaction metadata
        for cp in &mut tx.checkpoints {
            cp.deltas.retain(|d| !is_sensitive_secret_path(&d.relative_path));
            if let Some(ref prompt) = cp.prompt {
                let (scrubbed, count) = scrub_secrets_in_text(prompt);
                scrubbed_secrets_count += count;
                cp.prompt = Some(scrubbed);
            }
            if let Some(ref summary) = cp.summary {
                let (scrubbed, count) = scrub_secrets_in_text(summary);
                scrubbed_secrets_count += count;
                cp.summary = Some(scrubbed);
            }
        }

        // 3. Capture per-step file states for all tracked non-sensitive paths
        let mut step_files: BTreeMap<usize, BTreeMap<String, BundleFileState>> = BTreeMap::new();
        for cp in &tx.checkpoints {
            let mut files_at_step = BTreeMap::new();
            for rel_path in &candidate_paths {
                let rel_key = rel_path.to_string_lossy().to_string();
                let snap_file = cp.snapshot_path.join(rel_path);
                if let Ok(meta) = snap_file.symlink_metadata() {
                    if meta.is_file() {
                        let bytes = fs::read(&snap_file).map_err(|e| SavepointError::Io {
                            path: snap_file.clone(),
                            source: e,
                        })?;
                        match String::from_utf8(bytes) {
                            Ok(text) => {
                                let (scrubbed, count) = scrub_secrets_in_text(&text);
                                scrubbed_secrets_count += count;
                                files_at_step.insert(
                                    rel_key,
                                    BundleFileState::Present {
                                        content: scrubbed,
                                        is_hex: false,
                                    },
                                );
                            }
                            Err(non_utf8) => {
                                let hex_str = hex_encode(non_utf8.as_bytes());
                                files_at_step.insert(
                                    rel_key,
                                    BundleFileState::Present {
                                        content: hex_str,
                                        is_hex: true,
                                    },
                                );
                            }
                        }
                    }
                } else {
                    files_at_step.insert(rel_key, BundleFileState::Deleted);
                }
            }
            step_files.insert(cp.step_index, files_at_step);
        }

        let scrubbed_files_count = sensitive_paths.len();
        let tracked_files_count = candidate_paths.len();
        let checkpoints_count = tx.checkpoints.len();
        let transaction_id = tx.id.to_string();

        // Redact local host absolute paths from exported Transaction metadata
        tx.workspace_root = PathBuf::from(".");
        tx.shadow_root = PathBuf::from(".savepoint");
        for cp in &mut tx.checkpoints {
            cp.snapshot_path = PathBuf::from(format!("snapshots/cp_{}", cp.step_index));
        }

        let bundle = FlightBundle {
            format_version: "savepoint-bundle/v1".to_string(),
            created_at: Utc::now(),
            scrubbed_files_count,
            scrubbed_secrets_count,
            transaction: tx,
            step_files,
        };

        if let Some(parent) = output_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| SavepointError::Io {
                    path: parent.to_path_buf(),
                    source: e,
                })?;
            }
        }

        let serialized = serde_json::to_string_pretty(&bundle)?;
        fs::write(output_path, serialized).map_err(|e| SavepointError::Io {
            path: output_path.to_path_buf(),
            source: e,
        })?;

        Ok(BundleExportStats {
            transaction_id,
            output_path: output_path.to_path_buf(),
            checkpoints_count,
            tracked_files_count,
            scrubbed_files_count,
            scrubbed_secrets_count,
        })
    }

    /// Replays a `.spbundle` file into `target_workspace` at `target_step` (defaults to the bundle's active step).
    pub fn replay_bundle(
        bundle_path: &Path,
        target_workspace: &Path,
        target_step: Option<usize>,
    ) -> Result<BundleReplayStats> {
        let raw = fs::read_to_string(bundle_path).map_err(|e| SavepointError::Io {
            path: bundle_path.to_path_buf(),
            source: e,
        })?;
        let bundle: FlightBundle = serde_json::from_str(&raw)?;

        let step = target_step.unwrap_or_else(|| bundle.transaction.active_step());
        let files_map = bundle.step_files.get(&step).ok_or_else(|| {
            SavepointError::CheckpointNotFound(step, bundle.transaction.id.to_string())
        })?;

        fs::create_dir_all(target_workspace).map_err(|e| SavepointError::Io {
            path: target_workspace.to_path_buf(),
            source: e,
        })?;

        let mut restored_files_count = 0usize;
        for (rel_str, state) in files_map {
            let rel_path = Path::new(rel_str);
            validate_safe_relative_path(rel_path)?;
            ensure_no_symlink_ancestors(target_workspace, rel_path)?;
            let dest = target_workspace.join(rel_path);

            // Always remove any existing file, symlink, or directory at dest before writing or deleting
            // so we never follow an existing symlink or fail on read-only target files.
            if let Ok(meta) = dest.symlink_metadata() {
                if meta.is_dir() && !meta.file_type().is_symlink() {
                    fs::remove_dir_all(&dest).map_err(|e| SavepointError::Io {
                        path: dest.clone(),
                        source: e,
                    })?;
                } else {
                    fs::remove_file(&dest).map_err(|e| SavepointError::Io {
                        path: dest.clone(),
                        source: e,
                    })?;
                }
            }

            match state {
                BundleFileState::Present { content, is_hex } => {
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent).map_err(|e| SavepointError::Io {
                            path: parent.to_path_buf(),
                            source: e,
                        })?;
                    }
                    let bytes = if *is_hex {
                        hex_decode(content)?
                    } else {
                        content.as_bytes().to_vec()
                    };
                    fs::write(&dest, bytes).map_err(|e| SavepointError::Io {
                        path: dest,
                        source: e,
                    })?;
                    restored_files_count += 1;
                }
                BundleFileState::Deleted => {}
            }
        }

        Ok(BundleReplayStats {
            transaction_id: bundle.transaction.id.to_string(),
            replayed_step: step,
            restored_files_count,
        })
    }
}

fn validate_safe_relative_path(rel: &Path) -> Result<()> {
    if rel.as_os_str().is_empty() || rel.is_absolute() {
        return Err(SavepointError::Other(format!(
            "Unsafe bundle path rejected: {:?}",
            rel
        )));
    }
    for comp in rel.components() {
        match comp {
            Component::Normal(_) => {}
            _ => {
                return Err(SavepointError::Other(format!(
                    "Path traversal segment rejected in bundle path: {:?}",
                    rel
                )));
            }
        }
    }
    Ok(())
}

fn ensure_no_symlink_ancestors(target_workspace: &Path, rel: &Path) -> Result<()> {
    let mut cur = target_workspace.to_path_buf();
    if let Some(parent) = rel.parent() {
        for comp in parent.components() {
            cur.push(comp);
            if let Ok(meta) = cur.symlink_metadata() {
                if meta.file_type().is_symlink() {
                    return Err(SavepointError::Other(format!(
                        "Refusing to traverse symlinked directory during bundle replay: {:?}",
                        cur
                    )));
                }
            }
        }
    }
    Ok(())
}

fn is_sensitive_secret_path(rel: &Path) -> bool {
    let Some(name) = rel.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    if lower == ".env.example" || lower == ".env.template" || lower == ".env.sample" {
        return false;
    }
    if lower == ".env"
        || lower.starts_with(".env.")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.starts_with("id_rsa")
        || lower.starts_with("id_ed25519")
        || lower == "credentials.json"
    {
        return true;
    }
    false
}

/// Scrubs known API token prefixes (`sk-`, `ghp_`, `AKIA`, `AIza`, `xoxb-`) from text.
fn scrub_secrets_in_text(input: &str) -> (String, usize) {
    const PREFIXES: &[(&str, usize)] = &[
        ("sk-", 16),
        ("ghp_", 16),
        ("AKIA", 12),
        ("AIza", 16),
        ("xoxb-", 12),
    ];

    let mut out = String::with_capacity(input.len());
    let mut scrubbed_count = 0usize;
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let remaining = &input[i..];
        let mut matched_secret_len = None;
        let is_word_boundary =
            i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_' || bytes[i - 1] == b'-');

        if is_word_boundary {
            for &(prefix, min_tail_len) in PREFIXES {
                if let Some(after_prefix) = remaining.strip_prefix(prefix) {
                    let tail_len = after_prefix
                        .bytes()
                        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
                        .count();
                    if tail_len >= min_tail_len {
                        matched_secret_len = Some(prefix.len() + tail_len);
                        break;
                    }
                }
            }
        }

        if let Some(secret_len) = matched_secret_len {
            out.push_str("[REDACTED_SECRET]");
            scrubbed_count += 1;
            i += secret_len;
        } else {
            let ch = remaining.chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }

    (out, scrubbed_count)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn hex_decode(hex: &str) -> Result<Vec<u8>> {
    if !hex.is_ascii() || hex.len() % 2 != 0 {
        return Err(SavepointError::Other("Invalid hex string in bundle".to_string()));
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let byte = u8::from_str_radix(&hex[i..i + 2], 16)
            .map_err(|e| SavepointError::Other(format!("Invalid hex byte in bundle: {}", e)))?;
        out.push(byte);
    }
    Ok(out)
}
