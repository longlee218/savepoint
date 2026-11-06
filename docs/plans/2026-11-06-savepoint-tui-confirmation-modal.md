# Savepoint TUI Safety & Confirmation Dialog (Sprint 5) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Khắc phục triệt để nguy cơ thao tác nhầm: Bổ sung Hộp thoại xác nhận an toàn (Confirmation Dialog) trước khi thực hiện Tua ngược (Rewind), tách biệt phím `Enter` (Xem chi tiết) và phím `r` (Tua ngược), bảo đảm trải nghiệm chuyên nghiệp và an toàn tuyệt đối cho người dùng.

**Architecture:**
- `savepoint-tui`:
  - Mở rộng `TimelineApp` với trạng thái `ViewMode`: `Browsing`, `ConfirmRewind(usize)`.
  - Phím `Enter` hoặc `Space`: Xem chi tiết diff file.
  - Phím `r`: Mở popup hộp thoại xác nhận:
    `⚠️ XÁC NHẬN TUA NGƯỢC: Bạn có chắc chắn muốn đưa dự án về Bước #N? [y / Enter] Đồng ý • [n / Esc] Hủy`.
  - Chỉ khi người dùng bấm xác nhận rõ ràng, runtime mới phát tín hiệu `TimelineAction::Rewind`.

**Tech Stack:** Rust 2021, ratatui (Clear widget, Centered Rect, Popups), crossterm.

---

### Task 1: Cập nhật App State với Confirmation Mode

**Files:**
- Modify: `crates/savepoint-tui/src/app.rs`
- Test: `crates/savepoint-tui/tests/app_state_test.rs`

- [ ] Thêm enum `ViewMode { Normal, ConfirmingRewind(usize) }` vào `TimelineApp`.
- [ ] Bổ sung logic mở modal xác nhận và xử lý phím `y / n / Enter / Esc`.
- [ ] Test unit test xác nhận không trigger `Rewind` khi chưa confirm.

---

### Task 2: Render Popup Hộp Thoại Xác Nhận trong `ui.rs`

**Files:**
- Modify: `crates/savepoint-tui/src/ui.rs`

- [ ] Hiện thực hàm `render_confirm_modal` vẽ hộp thoại cảnh báo nổi ở giữa màn hình (dùng `Clear` và viền đỏ/vàng).
- [ ] Cập nhật thanh hướng dẫn chân trang: `[r] Tua ngược   •   [Enter] Xem chi tiết   •   [c] Commit   •   [q] Thoát`.

---

### Task 3: Cập nhật Event Loop & Biên dịch Release

**Files:**
- Modify: `crates/savepoint-tui/src/event.rs`

- [ ] Xử lý sự kiện phím theo từng `ViewMode`.
- [ ] Biên dịch release và cập nhật binary `~/.local/bin/sp`.
- [ ] Kiểm thử thực tế.
