# Savepoint Core & ShadowFS Engine (Sprint 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Xây dựng lõi Cargo workspace và engine ShadowFS cho Savepoint, khai thác syscall native `clonefile(2)` trên macOS APFS để tạo snapshot sao chép tức thì (< 3ms, zero storage overhead), tính toán file deltas, hỗ trợ rollback và commit nguyên tử.

**Architecture:** Sử dụng kiến trúc module hóa với Rust Cargo Workspace: `savepoint-core` định nghĩa domain models và error types; `savepoint-fs` phụ trách tương tác FFI tầng thấp với Darwin APFS `clonefile` và giải thuật quét diff cây thư mục; `savepoint-cli` cung cấp giao diện dòng lệnh cơ bản để test trực tiếp.

**Tech Stack:** Rust 1.91+, libc (Darwin sys/clonefile FFI), walkdir, tempfile, clap, serde, chrono.

**Spec:** `/Users/longlh/.gemini/antigravity/brain/04cd9e88-8d71-40b2-8f4d-be06084a4668/savepoint_system_specification.md`

## Global Constraints

- Phải biên dịch và chạy mượt mà trên macOS Apple Silicon (ARM64) với hệ thống tệp APFS.
- Tốc độ CoW snapshot cho thư mục workspace phải đạt dưới 5ms (mục tiêu lý tưởng < 3ms).
- Zero storage overhead: Snapshot chỉ tiêu tốn metadata blocks cho đến khi có file bị sửa đổi thực tế.
- Xử lý lỗi an toàn: Không bao giờ làm mất dữ liệu workspace của người dùng khi rollback hoặc commit.

## Review Focus

1. Thư mục đích `dst` đã tồn tại trước khi gọi `clonefile` -> APFS trả về EEXIST; hàm phải tự dọn dẹp hoặc tạo đường dẫn duy nhất trước khi clone.
2. Workspace chứa symlinks đứt gãy hoặc circular symlinks -> cờ `CLONE_NOFOLLOW` phải bảo đảm không bị lặp vô tận.
3. File bị khóa hoặc quyền truy cập chỉ đọc -> engine phải phát hiện lỗi permission và rollback sạch sẽ.
4. Xóa file trong shadow workspace rồi commit -> Workspace gốc phải xóa đúng file đó (tính toán `ChangeKind::Deleted` chính xác).
5. File deltas phải phân biệt đúng giữa `Created`, `Modified`, và `Deleted`.

---

### Task 1: Thiết lập Data Models & Error System trong `savepoint-core`

**Files:**
- Create: `crates/savepoint-core/Cargo.toml`
- Create: `crates/savepoint-core/src/lib.rs`
- Create: `crates/savepoint-core/src/types.rs`
- Create: `crates/savepoint-core/src/error.rs`
- Test: `crates/savepoint-core/tests/types_test.rs`

- [ ] Write failing test for `Transaction`, `Checkpoint`, `FileDelta`, và `TransactionId` serialization/deserialization.
- [ ] Run `cargo test -p savepoint-core` and verify test fails.
- [ ] Implement `Transaction`, `Checkpoint`, `TransactionStatus`, `FileDelta`, `ChangeKind` và `SavepointError`.
- [ ] Run `cargo test -p savepoint-core` and verify test passes.
- [ ] Commit or checkpoint Task 1.

---

### Task 2: Hiện thực APFS CoW Engine trong `savepoint-fs`

**Files:**
- Create: `crates/savepoint-fs/Cargo.toml`
- Create: `crates/savepoint-fs/src/lib.rs`
- Create: `crates/savepoint-fs/src/darwin.rs`
- Create: `crates/savepoint-fs/src/clone.rs`
- Test: `crates/savepoint-fs/tests/cow_test.rs`

- [ ] Write failing test in `crates/savepoint-fs/tests/cow_test.rs` kiểm tra việc clone 1 thư mục chứa nhiều file bằng `clone_tree` và benchmark thời gian thực thi (< 5ms).
- [ ] Run `cargo test -p savepoint-fs --test cow_test` and verify it fails.
- [ ] Implement `darwin::sys::apfs_clone` với `libc::clonefile` và wrapper `clone_tree(src, dst)`.
- [ ] Run `cargo test -p savepoint-fs --test cow_test` and confirm pass and verify benchmark latency.
- [ ] Commit or checkpoint Task 2.

---

### Task 3: Directory Diff Engine trong `savepoint-fs`

**Files:**
- Create: `crates/savepoint-fs/src/diff.rs`
- Modify: `crates/savepoint-fs/src/lib.rs`
- Test: `crates/savepoint-fs/tests/diff_test.rs`

- [ ] Write failing test in `crates/savepoint-fs/tests/diff_test.rs` kiểm tra phát hiện: file mới tạo (`Created`), file sửa nội dung (`Modified`), file bị xóa (`Deleted`).
- [ ] Run test and verify it fails.
- [ ] Implement `compute_deltas(base_dir: &Path, current_dir: &Path) -> Result<Vec<FileDelta>>` sử dụng walkdir và so sánh metadata / content hash.
- [ ] Run test and verify all delta scenarios pass.
- [ ] Commit or checkpoint Task 3.

---

### Task 4: ShadowEngine — Quản lý Vòng đời Transaction (Init, Checkpoint, Rewind, Commit)

**Files:**
- Create: `crates/savepoint-fs/src/engine.rs`
- Modify: `crates/savepoint-fs/src/lib.rs`
- Test: `crates/savepoint-fs/tests/engine_lifecycle_test.rs`

- [ ] Write failing test in `engine_lifecycle_test.rs`:
  1. Khởi tạo transaction trên một workspace giả lập.
  2. Tạo Turn 1: sửa file A -> `checkpoint()`.
  3. Tạo Turn 2: sửa file B, xóa file A -> `checkpoint()`.
  4. `rewind(Turn 1)`: kiểm tra file A được phục hồi nguyên vẹn, file B biến mất.
  5. `commit()`: xác nhận các thay đổi hợp lệ được phản ánh vào workspace gốc.
- [ ] Run test and verify it fails.
- [ ] Implement `ShadowEngine` quản lý cấu trúc `.savepoint/transactions/<tx_id>/snapshots/` với `clonefile`.
- [ ] Run test and verify the entire transaction lifecycle passes cleanly.
- [ ] Commit or checkpoint Task 4.

---

### Task 5: `savepoint-cli` Prototype để kiểm chứng thực tế

**Files:**
- Create: `crates/savepoint-cli/Cargo.toml`
- Create: `crates/savepoint-cli/src/main.rs`
- Test: Run direct CLI commands: `sp init`, `sp checkpoint`, `sp status`, `sp rewind`, `sp commit`.

- [ ] Implement CLI commands using `clap`.
- [ ] Build release binary and test end-to-end trên một sample directory.
- [ ] Verify timing logs và performance metrics.
