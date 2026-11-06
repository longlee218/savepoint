# Savepoint PTY Interactive Session Supervisor (Sprint 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Hiện thực crate `savepoint-pty` bọc các interactive CLI agent (`claude`, `aider`, `codex`, `bash`) trong pseudo-terminal (PTY) ở raw mode, giữ nguyên 100% trải nghiệm tương tác (ANSI color, streaming, terminal resize), đồng thời bắt magic hotkeys (`Ctrl+R` Time-Travel Rewind, `Ctrl+S` Checkpoint) và tự động quản lý transaction vòng đời.

**Architecture:** Crate `savepoint-pty` kết nối `savepoint-fs` với `portable-pty` và `crossterm`. Host terminal chuyển sang raw mode; luồng I/O được multiplex giữa user stdin, child PTY, và Savepoint supervisor interceptor. Khi bấm `Ctrl+R`, terminal tạm dừng chuyển tiếp dữ liệu đến child process để hiển thị popup menu tua ngược thời gian, khôi phục trạng thái workspace trong sub-millisecond mà không làm ngắt kết nối session của Agent.

**Tech Stack:** Rust 2021, portable-pty, crossterm (raw mode, terminal events), tokio / std threads, savepoint-core, savepoint-fs.

**Spec:** `/Users/longlh/.gemini/antigravity/brain/04cd9e88-8d71-40b2-8f4d-be06084a4668/savepoint_system_specification.md`

## Global Constraints

- Không được làm gián đoạn hoặc làm trễ luồng ký tự của terminal (zero perceptible input/output lag).
- Thoát an toàn: Đảm bảo khi tiến trình con kết thúc hoặc người dùng nhấn Ctrl+C/Ctrl+D, terminal luôn được khôi phục về chế độ bình thường (cooked mode).
- Hỗ trợ đầy đủ tín hiệu SIGWINCH (window resize) để các TUI phức tạp của Claude Code hiển thị chuẩn xác.

## Review Focus

1. Host terminal không thoát raw mode khi panic -> Cần `Drop` guard để luôn gọi `disable_raw_mode()`.
2. Ctrl+C trong PTY -> Phải gửi signal đến child process chứ không kill Savepoint đột ngột.
3. Ký tự non-UTF8 hoặc byte escape sequences phức tạp từ terminal -> xử lý dạng raw byte slices (`&[u8]`).
4. Khôi phục trạng thái khi đang giữa một turn -> PTY thông báo cho Agent biết workspace đã bị rollback.

---

### Task 1: Thiết lập Crate `savepoint-pty` & Raw Terminal Guard

**Files:**
- Create: `crates/savepoint-pty/Cargo.toml`
- Create: `crates/savepoint-pty/src/lib.rs`
- Create: `crates/savepoint-pty/src/terminal.rs`
- Test: `crates/savepoint-pty/tests/terminal_guard_test.rs`

- [ ] Add `savepoint-pty` to root `Cargo.toml` workspace.
- [ ] Write failing test for `RawTerminalGuard` ensuring raw mode enters and cleans up on drop.
- [ ] Implement `RawTerminalGuard` using `crossterm::terminal`.
- [ ] Run `cargo test -p savepoint-pty` and verify test passes.

---

### Task 2: PTY Process Spawner & I/O Multiplexer

**Files:**
- Create: `crates/savepoint-pty/src/supervisor.rs`
- Modify: `crates/savepoint-pty/src/lib.rs`
- Test: `crates/savepoint-pty/tests/supervisor_test.rs`

- [ ] Write test spawning an echo process in PTY, writing input and capturing streamed output.
- [ ] Implement `PtySupervisor` using `portable-pty` to spawn child process and pipe stdin/stdout.
- [ ] Run test and verify stream multiplexing passes.

---

### Task 3: Magic Hotkey Interceptor (`Ctrl+R` Time-Travel & `Ctrl+S` Manual Checkpoint)

**Files:**
- Create: `crates/savepoint-pty/src/interceptor.rs`
- Modify: `crates/savepoint-pty/src/supervisor.rs`
- Test: `crates/savepoint-pty/tests/hotkey_test.rs`

- [ ] Write test verifying that `0x12` (`Ctrl+R`) and `0x13` (`Ctrl+S`) are intercepted from the byte stream while normal text passes transparently.
- [ ] Implement `InputInterceptor` filter.
- [ ] Verify hotkey detection works without altering normal keystrokes.

---

### Task 4: Tích hợp `sp <agent_cmd>` vào `savepoint-cli`

**Files:**
- Modify: `crates/savepoint-cli/Cargo.toml`
- Modify: `crates/savepoint-cli/src/main.rs`
- Test: Manual end-to-end test running `sp bash` / `sp echo`.

- [ ] Add `run` command to `savepoint-cli` (e.g. `sp run claude` or direct `sp claude`).
- [ ] Wire automatic transaction creation on startup (`sp begin`), auto-checkpoints, and interactive Ctrl+R rewind menu.
- [ ] Test end-to-end interactive session.
