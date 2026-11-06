use crossterm::terminal::{disable_raw_mode, enable_raw_mode, is_raw_mode_enabled};
use std::io;

/// RAII Guard that puts the terminal in raw mode and guarantees
/// restoration of cooked mode when dropped.
pub struct RawTerminalGuard {
    was_raw: bool,
}

impl RawTerminalGuard {
    pub fn new() -> io::Result<Self> {
        let was_raw = is_raw_mode_enabled().unwrap_or(false);
        if !was_raw {
            enable_raw_mode()?;
        }
        Ok(Self { was_raw })
    }
}

impl Drop for RawTerminalGuard {
    fn drop(&mut self) {
        if !self.was_raw {
            let _ = disable_raw_mode();
        }
    }
}
