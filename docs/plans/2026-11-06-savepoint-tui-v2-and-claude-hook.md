# Savepoint TUI v2 (Time Machine) & Claude Code Integration (Sprint 4) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Nâng cấp trải nghiệm người dùng của Savepoint lên mức tối đa: 
1. Thêm các lệnh nhanh `sp undo` và `sp save`.
2. Thiết kế lại TUI `sp ui` thành giao diện "Time Machine" tối giản, cực kỳ trực quan và dễ hiểu.
3. Tích hợp Slash Command cho Claude Code (`/undo`, `/save`, `/timeline`) để người dùng chat trực tiếp với Claude mà không cần nhớ phím tắt.

**Architecture:** 
- `savepoint-fs`: Thêm phương thức `rewind_previous()` để lùi 1 bước mà không cần tính toán số step thủ công.
- `savepoint-tui`: Thiết kế lại `ui.rs` theo giao diện Time Machine tối giản: hiển thị danh sách dọc các bước rõ ràng kèm mô tả tiếng Việt/Anh trực quan, loại bỏ toàn bộ từ ngữ học thuật khó hiểu.
- `savepoint-cli`: Thêm subcommand `undo`, `save`, và `hook claude` (tự động tạo file slash command vào `~/.claude/commands/`).

**Tech Stack:** Rust 2021, ratatui, crossterm, savepoint-core, savepoint-fs.

---

### Task 1: Thêm `rewind_previous()` và Subcommands `sp undo`, `sp save`

**Files:**
- Modify: `crates/savepoint-fs/src/engine.rs`
- Modify: `crates/savepoint-cli/src/lib.rs`
- Test: `crates/savepoint-fs/tests/engine_lifecycle_test.rs`

- [ ] Implement `ShadowEngine::rewind_previous(tx: &mut Transaction) -> Result<usize>` hoàn nguyên về bước trước đó gần nhất.
- [ ] Add `sp undo` (alias `sp rollback`) và `sp save` (alias `sp checkpoint`) vào `savepoint-cli`.
- [ ] Test và verify `sp undo` lùi đúng 1 bước.

---

### Task 2: Tái thiết kế Giao diện TUI Time Machine Siêu Tối Giản

**Files:**
- Modify: `crates/savepoint-tui/src/ui.rs`
- Modify: `crates/savepoint-tui/src/app.rs`

- [ ] Redesign `render_ui` trong `ui.rs`:
  - Khung viền sạch sẽ, tiêu đề: `⏱ SAVEPOINT // TIME MACHINE`
  - Danh sách các bước dạng timeline:
    `● [BƯỚC 2] Vừa xong       • 2 files thay đổi (Đang ở đây)`
    `● [BƯỚC 1] 2 phút trước  • 1 file thay đổi  (Bấm Enter để quay về)`
    `● [BƯỚC 0] Khởi đầu       • Trạng thái ban đầu`
  - Hộp thông tin chi tiết bên dưới chỉ hiện tên file đã thay đổi một cách rõ ràng.
  - Hướng dẫn điều khiển đơn giản ở chân trang: `[↑ / ↓] Chọn bước   •   [Enter] Quay về bước này   •   [Esc / q] Thoát`
- [ ] Test `cargo test -p savepoint-tui`.

---

### Task 3: Claude Code Slash Commands Integration (`/undo`, `/save`, `/timeline`)

**Files:**
- Modify: `crates/savepoint-cli/src/lib.rs`
- Create: `~/.claude/commands/undo.md`
- Create: `~/.claude/commands/save.md`
- Create: `~/.claude/commands/timeline.md`

- [ ] Thêm lệnh `sp hook claude` tự động ghi các file slash command vào thư mục `~/.claude/commands/`.
- [ ] Khởi chạy lệnh `sp hook claude` để cài đặt vào Claude Code của người dùng.
- [ ] Kiểm tra xác nhận file markdown được tạo đúng chuẩn Claude Code slash command.

---

### Task 4: Biên dịch Release & Cập nhật Binary Toàn Hệ Thống

**Files:**
- Build: `target/release/sp`
- Deploy: `~/.local/bin/sp`, `~/.cargo/bin/sp`

- [ ] Build release toàn bộ workspace.
- [ ] Copy binary mới vào `~/.local/bin/sp` và `~/.cargo/bin/sp`.
- [ ] Chạy kiểm thử xác nhận.
