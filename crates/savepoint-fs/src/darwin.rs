#[cfg(target_os = "macos")]
pub mod sys {
    use savepoint_core::{Result, SavepointError};
    use std::ffi::{c_char, c_int, CString};
    use std::path::Path;

    pub const CLONE_NOFOLLOW: c_int = 0x0001;
    pub const CLONE_NOOWNERCOPY: c_int = 0x0002;
    pub const CLONE_ACL: c_int = 0x0004;

    extern "C" {
        pub fn clonefile(src: *const c_char, dst: *const c_char, flags: c_int) -> c_int;
    }

    /// Clones `src` to `dst` using macOS APFS Copy-on-Write (CoW).
    /// Both `src` and `dst` must reside on the same APFS volume.
    /// `dst` must NOT exist prior to this call.
    pub fn apfs_clone(src: &Path, dst: &Path, no_follow: bool) -> Result<()> {
        let src_c = CString::new(
            src.to_str()
                .ok_or_else(|| SavepointError::Other(format!("Non-UTF8 src path: {:?}", src)))?,
        )
        .map_err(|e| SavepointError::Other(e.to_string()))?;

        let dst_c = CString::new(
            dst.to_str()
                .ok_or_else(|| SavepointError::Other(format!("Non-UTF8 dst path: {:?}", dst)))?,
        )
        .map_err(|e| SavepointError::Other(e.to_string()))?;

        let mut flags = 0;
        if no_follow {
            flags |= CLONE_NOFOLLOW;
        }

        let ret = unsafe { clonefile(src_c.as_ptr(), dst_c.as_ptr(), flags) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            return Err(SavepointError::CloneFileFailed {
                src: src.to_path_buf(),
                dst: dst.to_path_buf(),
                errno: err.raw_os_error().unwrap_or(-1),
            });
        }

        Ok(())
    }
}
