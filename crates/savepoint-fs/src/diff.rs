use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use savepoint_core::{ChangeKind, FileDelta, Result, SavepointError};

/// Computes file-level deltas between `base_dir` and `current_dir`.
/// Ignores `.savepoint` and `.git` directories by default.
pub fn compute_deltas(base_dir: &Path, current_dir: &Path) -> Result<Vec<FileDelta>> {
    let mut base_files: HashMap<PathBuf, u64> = HashMap::new();
    let mut deltas = Vec::new();
    let mut seen_in_current = HashSet::new();

    // 1. Index base directory
    if base_dir.exists() {
        for entry in WalkDir::new(base_dir)
            .into_iter()
            .filter_entry(|e| !is_ignored(e.file_name()))
        {
            let entry = entry.map_err(|e| SavepointError::Other(e.to_string()))?;
            if entry.file_type().is_file() {
                let rel = entry
                    .path()
                    .strip_prefix(base_dir)
                    .map_err(|e| SavepointError::Other(e.to_string()))?
                    .to_path_buf();
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                base_files.insert(rel, size);
            }
        }
    }

    // 2. Scan current directory
    if current_dir.exists() {
        for entry in WalkDir::new(current_dir)
            .into_iter()
            .filter_entry(|e| !is_ignored(e.file_name()))
        {
            let entry = entry.map_err(|e| SavepointError::Other(e.to_string()))?;
            if entry.file_type().is_file() {
                let rel = entry
                    .path()
                    .strip_prefix(current_dir)
                    .map_err(|e| SavepointError::Other(e.to_string()))?
                    .to_path_buf();
                seen_in_current.insert(rel.clone());
                let new_size = entry.metadata().map(|m| m.len()).unwrap_or(0);

                if let Some(&old_size) = base_files.get(&rel) {
                    // Check if content changed
                    let base_file = base_dir.join(&rel);
                    let current_file = entry.path();
                    let has_changed = if old_size != new_size {
                        true
                    } else {
                        // Quick byte comparison
                        files_differ(&base_file, current_file)?
                    };

                    if has_changed {
                        deltas.push(FileDelta {
                            relative_path: rel,
                            kind: ChangeKind::Modified,
                            old_size: Some(old_size),
                            new_size: Some(new_size),
                        });
                    }
                } else {
                    deltas.push(FileDelta {
                        relative_path: rel,
                        kind: ChangeKind::Created,
                        old_size: None,
                        new_size: Some(new_size),
                    });
                }
            }
        }
    }

    // 3. Any file in base not seen in current was deleted
    for (rel, old_size) in base_files {
        if !seen_in_current.contains(&rel) {
            deltas.push(FileDelta {
                relative_path: rel,
                kind: ChangeKind::Deleted,
                old_size: Some(old_size),
                new_size: None,
            });
        }
    }

    // Sort for deterministic order
    deltas.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(deltas)
}

fn is_ignored(name: &std::ffi::OsStr) -> bool {
    let s = name.to_string_lossy();
    s == ".savepoint" || s == ".git"
}

fn files_differ(p1: &Path, p2: &Path) -> Result<bool> {
    let b1 = fs::read(p1).map_err(|e| SavepointError::Io {
        path: p1.to_path_buf(),
        source: e,
    })?;
    let b2 = fs::read(p2).map_err(|e| SavepointError::Io {
        path: p2.to_path_buf(),
        source: e,
    })?;
    Ok(b1 != b2)
}
