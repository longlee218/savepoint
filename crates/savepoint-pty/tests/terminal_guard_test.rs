use savepoint_pty::RawTerminalGuard;

#[test]
fn test_raw_terminal_guard_creation() {
    // Note: in CI or headless environments enable_raw_mode may succeed or fail depending on isatty
    // Here we test that the guard can be constructed or gracefully handles non-tty.
    let guard_res = RawTerminalGuard::new();
    if let Ok(guard) = guard_res {
        drop(guard);
    }
}
