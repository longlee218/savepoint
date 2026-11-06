use std::path::Path;
use savepoint_core::{Result, SavepointError};

#[cfg(target_os = "macos")]
use crate::darwin::sys::apfs_clone;

/// Clones an entire directory or single file from `src` to `dst` using
/// Copy-on-Write (CoW).
///
/// On macOS APFS, this invokes `clonefile(2)` which duplicates the inode
/// tree and blocks instantly with 0 bytes physical storage allocated.
pub fn clone_tree(src: &Path, dst: &Path) -> Result<()> {
    if !src.exists() {
        return Err(SavepointError::Io {
            path: src.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Source path does not exist: {:?}", src),
            ),
        });
    }

    // dst must not exist prior to clonefile
    if dst.exists() {
        if dst.is_dir() {
            std::fs::remove_dir_all(dst).map_err(|e| SavepointError::Io {
                path: dst.to_path_buf(),
                source: e,
            })?;
        } else {
            std::fs::remove_file(dst).map_err(|e| SavepointError::Io {
                path: dst.to_path_buf(),
                source: e,
            })?;
        }
    }

    // Ensure parent directory of dst exists
    if let Some(parent) = dst.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| SavepointError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
    }

    #[cfg(target_os = "macos")]
    {
        apfs_clone(src, dst, true)
    }

    #[cfg(not(target_os = "macos"))]
    {
        fallback_copy_tree(src, dst)
    }
}

#[cfg(not(target_os = "macos"))]
fn fallback_copy_tree(src: &Path, dst: &Path) -> Result<()> {
    use walkdir::WalkDir;

    for entry in WalkDir::new(src) {
        let entry = entry.map_err(|e| SavepointError::Other(e.to_string()))?;
        let rel_path = entry.path().strip_prefix(src).map_err(|e| SavepointError::Other(e.to_string()))?;
        let target_path = dst.join(rel_path);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target_path).map_err(|e| SavepointError::Io {
                path: target_path,
                source: e,
            })?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| SavepointError::Io {
                    path: parent.to_path_buf(),
                    source: e,
                })?;
            }
            std::fs::copy(entry.path(), &target_path).map_err(|e| SavepointError::Io {
                path: target_path,
                source: e,
            })?;
        }
    }
    Ok(())
}
