use savepoint_core::{Result, SavepointError};
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

#[cfg(target_os = "macos")]
use crate::darwin::sys::apfs_clone;

#[cfg(target_os = "linux")]
use crate::linux::sys::ficlone_file;

/// Clones top-level workspace entries from `workspace_root` into `snapshot_dir`,
/// explicitly skipping `.git` and `.savepoint` directories.
pub fn clone_workspace(workspace_root: &Path, snapshot_dir: &Path) -> Result<()> {
    if !workspace_root.exists() {
        return Err(SavepointError::Io {
            path: workspace_root.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Workspace path does not exist: {:?}", workspace_root),
            ),
        });
    }

    if snapshot_dir.symlink_metadata().is_ok() {
        remove_path(snapshot_dir)?;
    }
    fs::create_dir_all(snapshot_dir).map_err(|e| SavepointError::Io {
        path: snapshot_dir.to_path_buf(),
        source: e,
    })?;

    let ignore = crate::ignore::SavepointIgnore::for_workspace(workspace_root);

    for entry in fs::read_dir(workspace_root).map_err(|e| SavepointError::Io {
        path: workspace_root.to_path_buf(),
        source: e,
    })? {
        let entry = entry.map_err(|e| SavepointError::Io {
            path: workspace_root.to_path_buf(),
            source: e,
        })?;
        let src_entry = entry.path();
        let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
        if ignore.is_ignored(&src_entry, is_dir) {
            continue;
        }
        let name = entry.file_name();
        let dst_entry = snapshot_dir.join(&name);
        clone_tree(&src_entry, &dst_entry)?;
    }

    Ok(())
}

/// Clones an entire directory, file, or symlink from `src` to `dst` using
/// Copy-on-Write (CoW) when supported by the underlying filesystem.
///
/// On macOS APFS, invokes `clonefile(2)` and falls back to `fallback_copy_tree`
/// on cross-device (`EXDEV`) or unsupported (`ENOTSUP`) mounts.
pub fn clone_tree(src: &Path, dst: &Path) -> Result<()> {
    if let Err(e) = src.symlink_metadata() {
        return Err(SavepointError::Io {
            path: src.to_path_buf(),
            source: e,
        });
    }

    // dst must not exist prior to clonefile
    if dst.symlink_metadata().is_ok() {
        remove_path(dst)?;
    }

    // Ensure parent directory of dst exists
    if let Some(parent) = dst.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| SavepointError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
    }

    #[cfg(target_os = "macos")]
    {
        match apfs_clone(src, dst, true) {
            Ok(()) => Ok(()),
            Err(SavepointError::CloneFileFailed { errno, .. })
                if errno == libc::EXDEV || errno == libc::ENOTSUP =>
            {
                fallback_copy_tree(src, dst)
            }
            Err(e) => Err(e),
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        fallback_copy_tree(src, dst)
    }
}

/// Copies a directory tree, single file, or symlink while preserving modification timestamps.
/// On Linux, attempts `ioctl(FICLONE)` reflink before falling back to byte copy.
pub fn fallback_copy_tree(src: &Path, dst: &Path) -> Result<()> {
    let src_meta = src.symlink_metadata().map_err(|e| SavepointError::Io {
        path: src.to_path_buf(),
        source: e,
    })?;

    if src_meta.file_type().is_symlink() {
        return copy_symlink(src, dst);
    }

    if src_meta.file_type().is_file() {
        return copy_single_file_preserving_mtime(src, dst, &src_meta);
    }

    let mut dirs_to_stamp = Vec::new();

    for entry in WalkDir::new(src).follow_links(false) {
        let entry = entry.map_err(|e| SavepointError::Other(e.to_string()))?;
        let rel_path = entry
            .path()
            .strip_prefix(src)
            .map_err(|e| SavepointError::Other(e.to_string()))?;
        let target_path = dst.join(rel_path);
        let ft = entry.file_type();

        if ft.is_dir() {
            fs::create_dir_all(&target_path).map_err(|e| SavepointError::Io {
                path: target_path.clone(),
                source: e,
            })?;
            if let Ok(meta) = entry.metadata() {
                if let Ok(mtime) = meta.modified() {
                    dirs_to_stamp.push((target_path, mtime));
                }
            }
        } else if ft.is_symlink() {
            copy_symlink(entry.path(), &target_path)?;
        } else if ft.is_file() {
            let meta = entry
                .metadata()
                .map_err(|e| SavepointError::Other(e.to_string()))?;
            copy_single_file_preserving_mtime(entry.path(), &target_path, &meta)?;
        }
    }

    // Stamp directory mtimes in reverse (post-order: deepest directories first)
    // so creating children never overwrites a parent directory's mtime.
    for (dir_path, mtime) in dirs_to_stamp.into_iter().rev() {
        if let Ok(dir_fh) = fs::File::open(&dir_path) {
            let _ = dir_fh.set_modified(mtime);
        }
    }

    Ok(())
}

fn copy_single_file_preserving_mtime(
    src: &Path,
    dst: &Path,
    src_meta: &fs::Metadata,
) -> Result<()> {
    #[cfg(target_os = "linux")]
    if ficlone_file(src, dst).is_ok() {
        if let Ok(mtime) = src_meta.modified() {
            if let Ok(dst_file) = fs::File::open(dst) {
                let _ = dst_file.set_modified(mtime);
            }
        }
        return Ok(());
    }

    fs::copy(src, dst).map_err(|e| SavepointError::Io {
        path: dst.to_path_buf(),
        source: e,
    })?;

    if let Ok(mtime) = src_meta.modified() {
        // Open read-only so read-only files (e.g. 0o444) still have their mtime restored via futimens
        if let Ok(dst_file) = fs::File::open(dst) {
            let _ = dst_file.set_modified(mtime);
        }
    }
    Ok(())
}

fn copy_symlink(src: &Path, dst: &Path) -> Result<()> {
    let link_target = fs::read_link(src).map_err(|e| SavepointError::Io {
        path: src.to_path_buf(),
        source: e,
    })?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&link_target, dst).map_err(|e| SavepointError::Io {
            path: dst.to_path_buf(),
            source: e,
        })?;
    }
    Ok(())
}

fn remove_path(p: &Path) -> Result<()> {
    let meta = p.symlink_metadata().map_err(|e| SavepointError::Io {
        path: p.to_path_buf(),
        source: e,
    })?;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        fs::remove_dir_all(p).map_err(|e| SavepointError::Io {
            path: p.to_path_buf(),
            source: e,
        })
    } else {
        fs::remove_file(p).map_err(|e| SavepointError::Io {
            path: p.to_path_buf(),
            source: e,
        })
    }
}
