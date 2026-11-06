# Savepoint Interactive TUI Timeline (Sprint 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Xây dựng giao diện Terminal UI (TUI) trực quan tuyệt đẹp bằng `ratatui` cho Savepoint, cho phép nhà phát triển xem toàn bộ dòng thời gian (Timeline) các lượt Agent tương tác, duyệt chi tiết từng file thay đổi và bấm phím `Enter` để tua ngược thời gian tức thì.

**Architecture:** Tạo crate mới `crates/savepoint-tui` tách biệt logic hiển thị khỏi CLI. Giao diện chia làm 3 khối chính:
- **Header:** Transaction ID, thư mục workspace, số lượng checkpoints, trạng thái giao dịch.
- **Left Pane (Timeline Tree):** Danh sách các bước `[Step #0]`, `[Step #1]`, `[Step #2]` có con trỏ lựa chọn di chuyển bằng phím mũi tên hoặc `j/k`.
- **Right Pane (Step Inspector):** Chi tiết prompt, thời gian, và danh sách file deltas có màu sắc (`+ Created`, `~ Modified`, `- Deleted`).
- **Footer (Action Bar):** Phím tắt điều khiển (`[Enter] Rewind to this Step`, `[c] Commit`, `[q/Esc] Back/Exit`).

**Tech Stack:** Rust 2021, ratatui (0.28+), crossterm (0.28), savepoint-core, savepoint-fs.

**Spec:** `/Users/longlh/.gemini/antigravity/brain/04cd9e88-8d71-40b2-8f4d-be06084a4668/savepoint_system_specification.md`

## Global Constraints

- TUI phải khởi động tức thì trong dưới 5ms mà không gây giật lag.
- Xử lý mượt mà khi terminal bị resize hoặc kích thước màn hình nhỏ.
- Đảm bảo terminal luôn phục hồi chế độ hiển thị bình thường khi đóng TUI (thoát alt-screen).

---

### Task 1: Thiết lập Crate `savepoint-tui` & Dependency `ratatui`

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/savepoint-tui/Cargo.toml`
- Create: `crates/savepoint-tui/src/lib.rs`

- [ ] Add `ratatui` dependency vào workspace root `Cargo.toml`.
- [ ] Khởi tạo `crates/savepoint-tui` kết nối với `savepoint-core`.
- [ ] Kiểm tra `cargo check -p savepoint-tui`.

---

### Task 2: Hiện thực App State & Layout Renderer

**Files:**
- Create: `crates/savepoint-tui/src/app.rs`
- Create: `crates/savepoint-tui/src/ui.rs`
- Modify: `crates/savepoint-tui/src/lib.rs`
- Test: `crates/savepoint-tui/tests/app_state_test.rs`

- [ ] Write unit test cho `TimelineApp` (kiểm tra điều hướng cursor lên/xuống, bounds checking).
- [ ] Implement `TimelineApp` quản lý transaction và selected checkpoint index.
- [ ] Implement `render_ui` vẽ Header, Timeline List, Inspector và Footer.
- [ ] Verify test passes.

---

### Task 3: Interactive Event Loop & Action Returns

**Files:**
- Create: `crates/savepoint-tui/src/event.rs`
- Modify: `crates/savepoint-tui/src/lib.rs`

- [ ] Implement `run_timeline(tx: &Transaction) -> Result<TimelineAction>` với các hành động:
  - `TimelineAction::Rewind(usize)`
  - `TimelineAction::Commit`
  - `TimelineAction::Cancel`
- [ ] Xử lý phím `Enter`, `c`, `q`, `Esc`, `Up`, `Down`, `j`, `k`.

---

### Task 4: Tích hợp vào `savepoint-cli` (`sp timeline` / `sp ui` & `Ctrl+R`)

**Files:**
- Modify: `crates/savepoint-cli/Cargo.toml`
- Modify: `crates/savepoint-cli/src/lib.rs`

- [ ] Thêm lệnh `sp timeline` (alias `sp ui`).
- [ ] Kết nối `Ctrl+R` trong `sp run` mở trực tiếp TUI Timeline.
- [ ] Kiểm thử thực tế end-to-end.
