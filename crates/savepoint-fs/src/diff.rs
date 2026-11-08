use crate::ignore::{SavepointIgnore, HEAVY_DEP_DIRS};
use savepoint_core::{ChangeKind, FileDelta, Result, SavepointError};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStat {
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
    is_symlink: bool,
}

/// Computes file-level deltas between `base_dir` and `current_dir`.
/// Uses a two-stage stat-first comparison and collapses `HEAVY_DEP_DIRS` into single directory deltas.
pub fn compute_deltas(base_dir: &Path, current_dir: &Path) -> Result<Vec<FileDelta>> {
    let ignore = SavepointIgnore::for_workspace(current_dir);
    let mut base_files: HashMap<PathBuf, FileStat> = HashMap::new();
    let mut deltas = Vec::new();
    let mut seen_in_current = HashSet::new();

    // 1. Index base directory
    if base_dir.exists() {
        for entry in WalkDir::new(base_dir)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !ignore.is_entry_ignored(e, base_dir))
        {
            let entry = entry.map_err(|e| SavepointError::Other(e.to_string()))?;
            let ft = entry.file_type();
            if ft.is_file() || ft.is_symlink() {
                let rel = entry
                    .path()
                    .strip_prefix(base_dir)
                    .map_err(|e| SavepointError::Other(e.to_string()))?
                    .to_path_buf();
                if let Ok(meta) = entry.path().symlink_metadata() {
                    base_files.insert(
                        rel,
                        FileStat {
                            size: meta.len(),
                            mtime: meta.mtime(),
                            mtime_nsec: meta.mtime_nsec(),
                            is_symlink: ft.is_symlink(),
                        },
                    );
                }
            }
        }
    }

    // 2. Scan current directory
    if current_dir.exists() {
        for entry in WalkDir::new(current_dir)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !ignore.is_entry_ignored(e, current_dir))
        {
            let entry = entry.map_err(|e| SavepointError::Other(e.to_string()))?;
            let ft = entry.file_type();
            if ft.is_file() || ft.is_symlink() {
                let rel = entry
                    .path()
                    .strip_prefix(current_dir)
                    .map_err(|e| SavepointError::Other(e.to_string()))?
                    .to_path_buf();
                seen_in_current.insert(rel.clone());

                let meta = match entry.path().symlink_metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let new_size = meta.len();
                let new_mtime = meta.mtime();
                let new_nsec = meta.mtime_nsec();
                let is_symlink = ft.is_symlink();

                if let Some(&old_stat) = base_files.get(&rel) {
                    let base_file = base_dir.join(&rel);
                    let current_file = entry.path();

                    let has_changed =
                        if old_stat.is_symlink != is_symlink || old_stat.size != new_size {
                            true
                        } else if old_stat.mtime == new_mtime && old_stat.mtime_nsec == new_nsec {
                            // Stage 1 Fast-Path: size and nanosecond mtime match -> Unchanged (0 file opens)
                            false
                        } else if is_symlink {
                            symlinks_differ(&base_file, current_file)?
                        } else {
                            // Stage 2: 64KB chunked stream comparison
                            files_differ_streamed(&base_file, current_file)?
                        };

                    if has_changed {
                        deltas.push(FileDelta {
                            relative_path: rel,
                            kind: ChangeKind::Modified,
                            old_size: Some(old_stat.size),
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
    for (rel, old_stat) in base_files {
        if !seen_in_current.contains(&rel) {
            deltas.push(FileDelta {
                relative_path: rel,
                kind: ChangeKind::Deleted,
                old_size: Some(old_stat.size),
                new_size: None,
            });
        }
    }

    // 4. Check collapsed heavy dependency directories (node_modules, .venv, venv)
    // Only if they are not explicitly ignored by .gitignore
    for &dep_dir in HEAVY_DEP_DIRS {
        let curr_dep = current_dir.join(dep_dir);
        if ignore.is_ignored(&curr_dep, true) {
            continue;
        }

        let base_dep = base_dir.join(dep_dir);
        let base_exists = base_dep.symlink_metadata().is_ok();
        let curr_exists = curr_dep.symlink_metadata().is_ok();

        match (base_exists, curr_exists) {
            (false, true) => {
                deltas.push(FileDelta {
                    relative_path: PathBuf::from(dep_dir),
                    kind: ChangeKind::Created,
                    old_size: None,
                    new_size: None,
                });
            }
            (true, false) => {
                deltas.push(FileDelta {
                    relative_path: PathBuf::from(dep_dir),
                    kind: ChangeKind::Deleted,
                    old_size: None,
                    new_size: None,
                });
            }
            (true, true) => {
                if heavy_dir_changed(&base_dep, &curr_dep) {
                    deltas.push(FileDelta {
                        relative_path: PathBuf::from(dep_dir),
                        kind: ChangeKind::Modified,
                        old_size: None,
                        new_size: None,
                    });
                }
            }
            (false, false) => {}
        }
    }

    // Sort for deterministic order
    deltas.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(deltas)
}

fn heavy_dir_changed(base_dep: &Path, curr_dep: &Path) -> bool {
    let (_bm, _cm) = match (base_dep.symlink_metadata(), curr_dep.symlink_metadata()) {
        (Ok(bm), Ok(cm)) => (bm, cm),
        _ => return true,
    };

    // Check if a lockfile exists in parent directory and differs
    if let (Some(base_parent), Some(curr_parent)) = (base_dep.parent(), curr_dep.parent()) {
        for lock in [
            "package-lock.json",
            "bun.lock",
            "bun.lockb",
            "pnpm-lock.yaml",
            "yarn.lock",
            "poetry.lock",
            "Cargo.lock",
        ] {
            let b_lock = base_parent.join(lock);
            let c_lock = curr_parent.join(lock);
            if b_lock.is_file() && c_lock.is_file() {
                if let (Ok(bm), Ok(cm)) = (b_lock.metadata(), c_lock.metadata()) {
                    if bm.len() != cm.len() || bm.modified().ok() != cm.modified().ok() {
                        return true;
                    }
                }
            }
        }
    }

    // Inspect immediate top-level entries inside the dependency folder
    let (Ok(b_rd), Ok(c_rd)) = (fs::read_dir(base_dep), fs::read_dir(curr_dep)) else {
        return true;
    };
    let mut b_entries = HashMap::new();
    for entry in b_rd.flatten() {
        if let Ok(m) = entry.path().symlink_metadata() {
            let is_dir = m.file_type().is_dir();
            b_entries.insert(
                entry.file_name(),
                (is_dir, m.len(), m.mtime(), m.mtime_nsec()),
            );
        }
    }

    let mut c_count = 0;
    for entry in c_rd.flatten() {
        c_count += 1;
        let Ok(m) = entry.path().symlink_metadata() else {
            return true;
        };
        let is_dir = m.file_type().is_dir();
        match b_entries.get(&entry.file_name()) {
            Some(&(old_is_dir, old_len, mt, nsec)) => {
                if old_is_dir != is_dir {
                    return true;
                }
                // For regular files, check length and timestamp.
                // For directories, APFS clonefile assigns new directory mtimes,
                // so matching directory name and type is expected and stable.
                if !is_dir && (old_len != m.len() || mt != m.mtime() || nsec != m.mtime_nsec()) {
                    return true;
                }
            }
            None => return true,
        }
    }

    c_count != b_entries.len()
}

fn symlinks_differ(p1: &Path, p2: &Path) -> Result<bool> {
    let l1 = fs::read_link(p1).map_err(|e| SavepointError::Io {
        path: p1.to_path_buf(),
        source: e,
    })?;
    let l2 = fs::read_link(p2).map_err(|e| SavepointError::Io {
        path: p2.to_path_buf(),
        source: e,
    })?;
    Ok(l1 != l2)
}

fn files_differ_streamed(p1: &Path, p2: &Path) -> Result<bool> {
    const CHUNK_SIZE: usize = 64 * 1024;
    let mut f1 = File::open(p1).map_err(|e| SavepointError::Io {
        path: p1.to_path_buf(),
        source: e,
    })?;
    let mut f2 = File::open(p2).map_err(|e| SavepointError::Io {
        path: p2.to_path_buf(),
        source: e,
    })?;

    let mut buf1 = vec![0u8; CHUNK_SIZE];
    let mut buf2 = vec![0u8; CHUNK_SIZE];

    loop {
        let n1 = read_full_chunk(&mut f1, &mut buf1, p1)?;
        let n2 = read_full_chunk(&mut f2, &mut buf2, p2)?;
        if n1 != n2 || buf1[..n1] != buf2[..n2] {
            return Ok(true);
        }
        if n1 == 0 {
            return Ok(false);
        }
    }
}

fn read_full_chunk<R: Read>(reader: &mut R, buf: &mut [u8], path: &Path) -> Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match reader.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(SavepointError::Io {
                    path: path.to_path_buf(),
                    source: e,
                });
            }
        }
    }
    Ok(total)
}
