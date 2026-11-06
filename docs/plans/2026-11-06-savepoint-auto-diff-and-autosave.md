# Savepoint Live Auto-Diff & Background Auto-Save (Sprint 6) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Giải quyết triệt để vấn đề người dùng phải nhớ lưu mốc thủ công:
1. `sp ui` và `sp diff` tự động phát hiện mọi thay đổi trực tiếp (Live Unsaved Changes) giữa thư mục làm việc và mốc gần nhất, hiển thị ngay mục `⚡ [HIỆN TẠI] (1 file bị xóa: - app.js)`.
2. `sp run` tự động chạy một luồng giám sát ngầm (Auto-Save Background Watcher), tự động lưu checkpoint sau khi Agent sửa/xóa file mà không cần người dùng gõ lệnh hay phím tắt nào.

**Architecture:**
- `savepoint-fs`: Thêm hàm `ShadowEngine::compute_live_deltas(&tx) -> Result<Vec<FileDelta>>`.
- `savepoint-tui`: Thêm mục `[HIỆN TẠI - Chưa lưu]` vào đầu danh sách timeline trong `ui.rs` nếu có thay đổi sống trong workspace.
- `savepoint-cli`:
  - `sp diff`: Quét và in ra các thay đổi sống chưa lưu.
  - `sp run`: Kích hoạt watcher ngầm (debounced 1.5s) tự động chốt mốc `Checkpoint #N: Auto-saved`.

**Tech Stack:** Rust 2021, savepoint-core, savepoint-fs, savepoint-tui, crossterm.

---

### Task 1: Thêm `compute_live_deltas` và nâng cấp `sp diff`

**Files:**
- Modify: `crates/savepoint-fs/src/engine.rs`
- Modify: `crates/savepoint-cli/src/lib.rs`
- Test: `crates/savepoint-fs/tests/engine_lifecycle_test.rs`

- [ ] Implement `ShadowEngine::compute_live_deltas(tx: &Transaction) -> Result<Vec<FileDelta>>`.
- [ ] Cập nhật `sp diff` để hiển thị các thay đổi sống giữa workspace hiện tại và checkpoint gần nhất.
- [ ] Test và verify.

---

### Task 2: Hiển thị Mục `[HIỆN TẠI]` Trực Tiếp Trong `sp ui` (Live TUI)

**Files:**
- Modify: `crates/savepoint-tui/src/app.rs`
- Modify: `crates/savepoint-tui/src/ui.rs`
- Modify: `crates/savepoint-tui/src/event.rs`
- Modify: `crates/savepoint-cli/src/lib.rs`

- [ ] Cập nhật `TimelineApp` nhận thêm `live_deltas: Vec<FileDelta>`.
- [ ] Hiển thị dòng đầu tiên trong TUI: `⚡ [HIỆN TẠI - Chưa lưu] (N files thay đổi)`.
- [ ] Khi chọn bất kỳ mốc nào (ví dụ Bước #0) và bấm Enter $\rightarrow$ Hoàn nguyên workspace về mốc đó.

---

### Task 3: Background Auto-Save Watcher Trong `sp run`

**Files:**
- Modify: `crates/savepoint-cli/src/lib.rs`

- [ ] Bổ sung luồng kiểm tra thay đổi định kỳ (1.5s debounce) trong `sp run`.
- [ ] Khi phát hiện file bị sửa/xóa/tạo mới $\rightarrow$ tự động lưu `sp checkpoint` và thông báo nhẹ nhàng lên màn hình.
- [ ] Biên dịch release và cập nhật binary toàn hệ thống.
