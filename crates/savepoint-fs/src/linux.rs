#[cfg(target_os = "linux")]
pub mod sys {
    use std::fs::{File, OpenOptions};
    use std::os::unix::io::AsRawFd;
    use std::path::Path;

    const FICLONE: libc::c_ulong = 0x40049409;

    pub fn ficlone_file(src: &Path, dst: &Path) -> std::io::Result<()> {
        let src_file = File::open(src)?;
        let src_meta = src_file.metadata()?;
        let dst_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(dst)?;

        let ret = unsafe { libc::ioctl(dst_file.as_raw_fd(), FICLONE as _, src_file.as_raw_fd()) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }

        dst_file.set_permissions(src_meta.permissions())?;
        if let Ok(mtime) = src_meta.modified() {
            let _ = dst_file.set_modified(mtime);
        }
        Ok(())
    }
}
