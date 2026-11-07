# Core CoW, Volume-Aware Storage & Two-Stage Fast Diff Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade `savepoint-fs` and `savepoint-core` with entry-filtered Copy-on-Write cloning, `st_dev` volume-aware storage routing, two-stage metadata-first diffing with heavy-directory collapse, per-delta CoW rollback with SQLite WAL cleanup, and SHA-256 Merkle checkpoint hashing.

**Architecture:** `clone_workspace` iterates top-level workspace entries (skipping `.git` and `.savepoint`) and invokes `clonefile(2)` on macOS or `ioctl(FICLONE)` on Linux with an `mtime`-preserving copy fallback. `compute_deltas` short-circuits on matching `(size, mtime, mtime_nsec)` metadata before falling back to a 64KB chunked reader, collapsing `node_modules`/`.venv` into single directory deltas and skipping build cache directories (`target`, `.next`). `ShadowEngine` routes storage by `st_dev`, hashes checkpoint metadata with SHA-256, restores delta paths via `clone_tree`, and removes orphaned SQLite `-wal`/`-shm` files on rollback.

**Tech Stack:** Rust 2021, `libc` (`clonefile`, `ioctl`, `FICLONE`), `walkdir`, `sha2`, `tempfile`.

**Spec:** `docs/superpowers/specs/2026-11-07-core-cow-fast-diff-design.md`

## Global Constraints

- Rust 2021 edition across the existing Cargo workspace (`savepoint-core`, `savepoint-fs`, `savepoint-cli`, `savepoint-pty`, `savepoint-tui`).
- Existing `Transaction` and `Checkpoint` JSON files (`meta.json`) must deserialize without error using `#[serde(default)]`.
- Never replace the `workspace_root` directory inode itself during `rewind` or `merge_branch`.
- Never open or read file bytes in `compute_deltas` when `(size, mtime_sec, mtime_nsec)` matches between base and current files.
- Never leave untracked Savepoint files visible to `git status` in the user's repository.

## Review Focus

- Broken symlinks inside the workspace (e.g., dangling symlink in `node_modules/.bin`) must not cause `clone_workspace` or `compute_deltas` to fail with `ENOENT`.
- Fallback copy (`fallback_copy_tree`) when CoW is unsupported or crosses devices must preserve `mtime` so an immediate `compute_deltas` reports 0 false-positive modifications.
- Unchanged files with `0o000` (unreadable) permissions whose `(size, mtime, mtime_nsec)` match must be skipped by Stage 1 metadata diff without attempting `File::open`, while same-size files with updated `mtime` and altered bytes must be caught by Stage 2 chunked comparison.
- Rolling back a SQLite database (`*.db`, `*.sqlite`, `*.sqlite3`) must delete orphaned `-wal` and `-shm` files created after the target checkpoint so SQLite does not replay stale frames.
- Modifying or adding packages inside `node_modules/` must emit at most a single `node_modules` delta in `Checkpoint.deltas` (preventing `meta.json` bloat) while still restoring `node_modules/` accurately on `rewind`.

---

### Task 1: Entry-Filtered CoW Cloning, Linux Reflink & Mtime-Preserving Fallback

**Files:**
- Create: `crates/savepoint-fs/src/linux.rs`
- Modify: `crates/savepoint-fs/src/clone.rs`
- Modify: `crates/savepoint-fs/src/lib.rs`
- Test: `crates/savepoint-fs/tests/cow_test.rs`

**Interfaces:**
- Consumes: `crate::darwin::sys::apfs_clone` (`crates/savepoint-fs/src/darwin.rs`)
- Produces:
  - `pub fn clone_tree(src: &Path, dst: &Path) -> savepoint_core::Result<()>`
  - `pub fn clone_workspace(workspace_root: &Path, snapshot_dir: &Path) -> savepoint_core::Result<()>`
  - `pub fn fallback_copy_tree(src: &Path, dst: &Path) -> savepoint_core::Result<()>`

- [ ] **Step 1: Write the failing tests in `crates/savepoint-fs/tests/cow_test.rs`**

Add tests for `clone_workspace` (skipping `.git` and `.savepoint`, preserving broken symlinks) and `fallback_copy_tree` (preserving exact `mtime` across files):

```rust
#[test]
fn test_clone_workspace_skips_git_and_savepoint_and_handles_broken_symlink() {
    use savepoint_fs::clone_workspace;
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path().join("ws");
    let snap = temp.path().join("snap");
    std::fs::create_dir_all(ws.join(".git/objects")).unwrap();
    std::fs::write(ws.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::create_dir_all(ws.join(".savepoint/spool")).unwrap();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/lib.rs"), "pub fn ok() {}").unwrap();
    symlink(Path::new("nonexistent_target"), ws.join("src/dangling_link")).unwrap();

    clone_workspace(&ws, &snap).expect("clone_workspace must succeed");

    assert!(!snap.join(".git").exists());
    assert!(!snap.join(".savepoint").exists());
    assert!(snap.join("src/lib.rs").exists());
    assert!(snap.join("src/dangling_link").symlink_metadata().is_ok());
}

#[test]
fn test_fallback_copy_preserves_mtime_for_zero_diff() {
    use savepoint_fs::clone::fallback_copy_tree;
    use std::os::unix::fs::MetadataExt;

    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    let dst = temp.path().join("dst");
    std::fs::create_dir_all(&src).unwrap();
    let f = src.join("data.txt");
    std::fs::write(&f, "hello world").unwrap();

    fallback_copy_tree(&src, &dst).expect("fallback_copy_tree must succeed");

    let m1 = std::fs::symlink_metadata(&f).unwrap();
    let m2 = std::fs::symlink_metadata(dst.join("data.txt")).unwrap();
    assert_eq!(m1.len(), m2.len());
    assert_eq!(m1.mtime(), m2.mtime());
    assert_eq!(m1.mtime_nsec(), m2.mtime_nsec());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd savepoint && cargo test --package savepoint-fs --test cow_test`
Expected: FAIL with `unresolved import savepoint_fs::clone_workspace` and `function fallback_copy_tree is private`

- [ ] **Step 3: Implement `crates/savepoint-fs/src/linux.rs`, `crates/savepoint-fs/src/clone.rs`, and export in `crates/savepoint-fs/src/lib.rs`**

In `crates/savepoint-fs/src/linux.rs` (`#[cfg(target_os = "linux")]`):
- Implement `pub fn ficlone_file(src: &Path, dst: &Path) -> std::io::Result<()>` opening `src` read-only and `dst` write-create-truncate, invoking `unsafe { libc::ioctl(dst_fd, 0x40049409 as _, src_fd) }` (`FICLONE = 0x40049409`), and calling `dst_file.set_modified(src_file.metadata()?.modified()?)`.

In `crates/savepoint-fs/src/clone.rs`:
- Implement `pub fn clone_workspace(workspace_root: &Path, snapshot_dir: &Path) -> Result<()>`: creates `snapshot_dir`, iterates `std::fs::read_dir(workspace_root)`, skips entry names `".git"` and `".savepoint"`, and calls `clone_tree(&entry.path(), &snapshot_dir.join(entry.file_name()))` for each remaining entry.
- Update `pub fn clone_tree(src: &Path, dst: &Path) -> Result<()>`:
  - Check `src.symlink_metadata()` instead of `src.exists()` so dangling symlinks are handled.
  - Remove `dst` if `dst.symlink_metadata().is_ok()`.
  - On macOS, call `apfs_clone(src, dst, true)`; if it returns `SavepointError::CloneFileFailed { errno, .. }` where `errno == libc::EXDEV || errno == libc::ENOTSUP`, fall back to `fallback_copy_tree(src, dst)`.
- Make `pub fn fallback_copy_tree(src: &Path, dst: &Path) -> Result<()>` public on all platforms:
  - Handle single file, symlink (`std::fs::read_link` + `std::os::unix::fs::symlink`), or directory (`WalkDir::new(src).follow_links(false)`).
  - For every copied regular file, preserve modification time via `let mtime = entry.metadata()?.modified()?; std::fs::File::options().write(true).open(&target_path)?.set_modified(mtime)?;`.

In `crates/savepoint-fs/src/lib.rs`:
- Export `pub use clone::{clone_tree, clone_workspace};`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd savepoint && cargo test --package savepoint-fs --test cow_test`
Expected: PASS (3 passed)

- [ ] **Step 5: Commit**

```bash
cd savepoint && git add crates/savepoint-fs/src/linux.rs crates/savepoint-fs/src/clone.rs crates/savepoint-fs/src/lib.rs crates/savepoint-fs/tests/cow_test.rs && git commit -m "feat(fs): add clone_workspace entry filter and mtime-preserving fallback"
```

---

### Task 2: Two-Stage Stat-First Diff Engine & Heavy-Directory Collapse

**Files:**
- Modify: `crates/savepoint-fs/src/diff.rs`
- Test: `crates/savepoint-fs/tests/diff_test.rs`

**Interfaces:**
- Consumes: `savepoint_core::{ChangeKind, FileDelta, Result, SavepointError}`
- Produces:
  - `pub const HEAVY_CACHE_DIRS: &[&str] = &["target", ".next", "__pycache__", ".gradle", ".turbo", ".cache"];`
  - `pub const HEAVY_DEP_DIRS: &[&str] = &["node_modules", ".venv", "venv"];`
  - `pub fn compute_deltas(base_dir: &Path, current_dir: &Path) -> Result<Vec<FileDelta>>`

- [ ] **Step 1: Write the failing tests in `crates/savepoint-fs/tests/diff_test.rs`**

Add tests verifying:
1. Stage 1 skips reading unchanged files (tested by chmodding an unchanged file in `current_dir` to `0o000` with identical `mtime` and `size`—`compute_deltas` must succeed with 0 deltas without opening it), while a same-size file with a newer `mtime` and different bytes is detected as `ChangeKind::Modified`.
2. Broken symlinks do not crash `compute_deltas`.
3. Files inside `target/` are ignored by `compute_deltas`, and changes to `node_modules/` produce a single collapsed `node_modules` delta instead of per-file deltas.

```rust
#[test]
fn test_two_stage_stat_fast_path_and_same_size_content_diff() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, SystemTime};

    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("base");
    let curr = temp.path().join("curr");
    std::fs::create_dir_all(&base).unwrap();

    let untouched_base = base.join("untouched.txt");
    let rewritten_base = base.join("rewritten.txt");
    std::fs::write(&untouched_base, "aaaa").unwrap();
    std::fs::write(&rewritten_base, "1111").unwrap();
    std::os::unix::fs::symlink(std::path::Path::new("missing_target"), base.join("broken_link")).unwrap();

    savepoint_fs::clone_workspace(&base, &curr).unwrap();

    // Make untouched file unreadable (0o000) while preserving its mtime from clone
    let orig_mtime = untouched_base.metadata().unwrap().modified().unwrap();
    let untouched_curr = curr.join("untouched.txt");
    std::fs::set_permissions(&untouched_curr, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Restore mtime in case chmod touched ctime only (mtime is unchanged)
    assert_eq!(
        untouched_curr.metadata().unwrap().modified().unwrap(),
        orig_mtime
    );

    // Overwrite rewritten.txt with same byte length (4 bytes) and advance mtime
    let rewritten_curr = curr.join("rewritten.txt");
    std::fs::write(&rewritten_curr, "2222").unwrap();
    let f = std::fs::File::options().write(true).open(&rewritten_curr).unwrap();
    f.set_modified(orig_mtime + Duration::from_secs(5)).unwrap();

    let deltas = savepoint_fs::compute_deltas(&base, &curr).expect("diff must succeed without opening untouched.txt");

    // Restore permissions so tempdir cleanup succeeds
    std::fs::set_permissions(&untouched_curr, std::fs::Permissions::from_mode(0o644)).unwrap();

    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].relative_path, std::path::PathBuf::from("rewritten.txt"));
    assert_eq!(deltas[0].kind, savepoint_core::ChangeKind::Modified);
}

#[test]
fn test_heavy_dirs_collapsed_and_build_caches_ignored() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("base");
    let curr = temp.path().join("curr");
    std::fs::create_dir_all(&base).unwrap();
    savepoint_fs::clone_workspace(&base, &curr).unwrap();

    // Populate target/ with build artifacts -> should be ignored in compute_deltas
    std::fs::create_dir_all(curr.join("target/debug/deps")).unwrap();
    std::fs::write(curr.join("target/debug/deps/libfoo.rlib"), "binary").unwrap();

    // Populate node_modules/ with multiple nested files -> should collapse to 1 "node_modules" delta
    std::fs::create_dir_all(curr.join("node_modules/pkg_a")).unwrap();
    std::fs::write(curr.join("node_modules/pkg_a/index.js"), "module.exports = 1;").unwrap();
    std::fs::create_dir_all(curr.join("node_modules/pkg_b")).unwrap();
    std::fs::write(curr.join("node_modules/pkg_b/index.js"), "module.exports = 2;").unwrap();

    let deltas = savepoint_fs::compute_deltas(&base, &curr).unwrap();
    assert_eq!(deltas.len(), 1);
    assert_eq!(deltas[0].relative_path, std::path::PathBuf::from("node_modules"));
    assert_eq!(deltas[0].kind, savepoint_core::ChangeKind::Created);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd savepoint && cargo test --package savepoint-fs --test diff_test`
Expected: FAIL (`Permission denied` when `fs::read` opens `untouched.txt`, and `deltas.len()` is 3 instead of 1 in `test_heavy_dirs_collapsed_and_build_caches_ignored`)

- [ ] **Step 3: Implement two-stage diff and heavy-directory collapse in `crates/savepoint-fs/src/diff.rs`**

In `crates/savepoint-fs/src/diff.rs`:
- Define `pub const HEAVY_CACHE_DIRS: &[&str] = &["target", ".next", "__pycache__", ".gradle", ".turbo", ".cache"];` and `pub const HEAVY_DEP_DIRS: &[&str] = &["node_modules", ".venv", "venv"];`.
- In `is_ignored(name: &OsStr) -> bool`: return `true` for `".savepoint"`, `".git"`, all `HEAVY_CACHE_DIRS`, and all `HEAVY_DEP_DIRS` during the recursive `WalkDir` pass.
- In `compute_deltas(base_dir: &Path, current_dir: &Path) -> Result<Vec<FileDelta>>`:
  - Index `base_dir` storing `(u64, i64, i64)` (`(len, mtime, mtime_nsec)` from `std::os::unix::fs::MetadataExt`) per relative path using `entry.path().symlink_metadata()`.
  - During `current_dir` scan:
    - If `old_size != new_size`, emit `ChangeKind::Modified`.
    - Else if `(old_mtime, old_nsec) == (new_mtime, new_nsec)`, skip (Unchanged, 0 file opens).
    - Else call `files_differ_streamed(&base_file, current_file)?` which opens both files with `std::io::BufReader::with_capacity(64 * 1024, f)` (or compares `std::fs::read_link` targets if symlink) and reads 64KB buffers in a loop.
  - After the `WalkDir` pass, check each `dep_dir` in `HEAVY_DEP_DIRS` at `base_dir.join(dep_dir)` vs `current_dir.join(dep_dir)`:
    - If `!base_dep.exists() && curr_dep.exists()` -> push `FileDelta { relative_path: PathBuf::from(dep_dir), kind: ChangeKind::Created, old_size: None, new_size: None }`.
    - If `base_dep.exists() && !curr_dep.exists()` -> push `FileDelta { relative_path: PathBuf::from(dep_dir), kind: ChangeKind::Deleted, old_size: None, new_size: None }`.
    - If both exist -> compare `(mtime(), mtime_nsec())` of the directory root `symlink_metadata()` (and any top-level children's `(mtime(), mtime_nsec())` inside `dep_dir`); if different, push `FileDelta { relative_path: PathBuf::from(dep_dir), kind: ChangeKind::Modified, old_size: None, new_size: None }`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd savepoint && cargo test --package savepoint-fs --test diff_test`
Expected: PASS (3 passed)

- [ ] **Step 5: Commit**

```bash
cd savepoint && git add crates/savepoint-fs/src/diff.rs crates/savepoint-fs/tests/diff_test.rs && git commit -m "feat(fs): implement two-stage stat diff and heavy directory collapse"
```

---

### Task 3: Volume-Aware Storage (`st_dev`), Per-Delta CoW Rewind, SQLite WAL Cleanup & SHA-256 `content_hash`

**Files:**
- Modify: `crates/savepoint-fs/Cargo.toml`
- Modify: `crates/savepoint-fs/src/engine.rs`
- Test: `crates/savepoint-fs/tests/engine_lifecycle_test.rs`

**Interfaces:**
- Consumes: `crate::clone::{clone_tree, clone_workspace}`, `crate::diff::compute_deltas`, `sha2::{Digest, Sha256}`
- Produces:
  - `ShadowEngine::resolve_storage_dir(workspace_root: &Path) -> PathBuf` (with `st_dev` routing)
  - `ShadowEngine::checkpoint` (populates `Checkpoint.content_hash: Some(String)`)
  - `ShadowEngine::rewind` & `ShadowEngine::merge_branch` (CoW restoration + SQLite `-wal`/`-shm` cleanup)

- [ ] **Step 1: Write the failing tests in `crates/savepoint-fs/tests/engine_lifecycle_test.rs`**

Add tests for:
1. `content_hash` populated with 64-char SHA-256 hex digest on checkpoints.
2. SQLite `-wal` and `-shm` orphan cleanup when rolling back a `.db` file.
3. Collapsed `node_modules` directory restoration via CoW on `rewind`.

```rust
#[test]
fn test_rewind_cleans_up_orphaned_sqlite_wal_and_shm_and_populates_content_hash() {
    use savepoint_fs::ShadowEngine;
    use std::fs;

    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();

    let db_path = ws.join("app.db");
    fs::write(&db_path, "sqlite-v1-clean").unwrap();

    let mut tx = ShadowEngine::begin_transaction(ws).unwrap();
    assert!(tx.checkpoints[0].content_hash.as_deref().unwrap().len() == 64);

    // Simulate dirty WAL writes during agent execution
    fs::write(&db_path, "sqlite-v2-dirty").unwrap();
    fs::write(ws.join("app.db-wal"), "dirty-wal-frames").unwrap();
    fs::write(ws.join("app.db-shm"), "dirty-shm-index").unwrap();

    let cp1 = ShadowEngine::checkpoint(&mut tx, Some("dirty db state"), None).unwrap();
    assert_eq!(cp1.content_hash.as_deref().unwrap().len(), 64);
    assert_ne!(tx.checkpoints[0].content_hash, cp1.content_hash);

    // Rewind back to Step 0 (where app.db-wal and app.db-shm did not exist)
    ShadowEngine::rewind(&mut tx, 0).unwrap();

    assert_eq!(fs::read_to_string(&db_path).unwrap(), "sqlite-v1-clean");
    assert!(!ws.join("app.db-wal").exists(), "orphaned -wal must be removed");
    assert!(!ws.join("app.db-shm").exists(), "orphaned -shm must be removed");
}

#[test]
fn test_heavy_node_modules_collapsed_in_deltas_and_restored_on_rewind() {
    use savepoint_fs::ShadowEngine;
    use std::fs;

    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();

    fs::create_dir_all(ws.join("node_modules/pkg_v1")).unwrap();
    fs::write(ws.join("node_modules/pkg_v1/index.js"), "v1").unwrap();

    let mut tx = ShadowEngine::begin_transaction(ws).unwrap();

    // Agent installs pkg_v2 and removes pkg_v1
    fs::remove_dir_all(ws.join("node_modules/pkg_v1")).unwrap();
    fs::create_dir_all(ws.join("node_modules/pkg_v2")).unwrap();
    fs::write(ws.join("node_modules/pkg_v2/index.js"), "v2").unwrap();

    let cp1 = ShadowEngine::checkpoint(&mut tx, Some("upgrade deps"), None).unwrap();
    assert_eq!(cp1.deltas.len(), 1);
    assert_eq!(cp1.deltas[0].relative_path, std::path::PathBuf::from("node_modules"));

    // Rewind to Step 0 -> pkg_v1 must be restored and pkg_v2 gone!
    ShadowEngine::rewind(&mut tx, 0).unwrap();
    assert!(ws.join("node_modules/pkg_v1/index.js").exists());
    assert!(!ws.join("node_modules/pkg_v2").exists());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd savepoint && cargo test --package savepoint-fs --test engine_lifecycle_test`
Expected: FAIL (`tx.checkpoints[0].content_hash.as_deref().unwrap()` panics on `None`)

- [ ] **Step 3: Implement `st_dev` routing, SHA-256 `content_hash`, CoW delta restore, and SQLite WAL cleanup in `crates/savepoint-fs/src/engine.rs`**

In `crates/savepoint-fs/Cargo.toml`:
- Add `sha2.workspace = true` under `[dependencies]`.

In `crates/savepoint-fs/src/engine.rs`:
- Import `crate::clone::{clone_tree, clone_workspace}` and `sha2::{Digest, Sha256}`.
- In `resolve_storage_dir(workspace_root: &Path) -> PathBuf`:
  - Check `SAVEPOINT_DIR` first.
  - Otherwise, create `~/.savepoint` if missing and compare `std::os::unix::fs::MetadataExt::dev()` between `~/.savepoint` and `canonical`.
  - If `dev()` matches, return `~/.savepoint/workspaces/<hash>`.
  - If `dev()` differs, try creating `<canonical.parent()>/.savepoint-spool/<hash>` if on the same `dev()`; otherwise fall back to `<canonical>/.savepoint/workspaces/<hash>` and append `.savepoint/\n` to `<canonical>/.git/info/exclude` if `.git/info` exists.
- Replace `clone_tree(&ws, &cp0_path)` and `clone_tree(&tx.workspace_root, &current_cp_path)` with `clone_workspace(...)`.
- Add helper `fn compute_checkpoint_hash(parent_step: Option<usize>, branch: &str, deltas: &[FileDelta]) -> String` using `Sha256::new()` and populate `content_hash: Some(hash)` in `begin_transaction` and `checkpoint`.
- Add helper `fn apply_snapshot_deltas(snapshot_path: &Path, workspace_root: &Path, deltas: &[FileDelta]) -> Result<()>` used by both `rewind` and `merge_branch`:
  - For `ChangeKind::Created`: remove file or dir at `workspace_root.join(&delta.relative_path)`.
  - For `ChangeKind::Modified | ChangeKind::Deleted`: call `clone_tree(&snapshot_path.join(&delta.relative_path), &workspace_root.join(&delta.relative_path))?`.
  - For any `delta.relative_path` whose file name ends with `.db`, `.sqlite`, or `.sqlite3`: check `"-wal"` and `"-shm"` suffixed companions; if the companion does not exist in `snapshot_path`, remove it from `workspace_root`.

- [ ] **Step 4: Run all tests in `savepoint-fs` and workspace to verify they pass**

Run: `cd savepoint && cargo test --package savepoint-fs`
Expected: PASS (all tests in `cow_test`, `diff_test`, `engine_lifecycle_test`, `branching_test` pass)

- [ ] **Step 5: Commit**

```bash
cd savepoint && git add crates/savepoint-fs/Cargo.toml crates/savepoint-fs/src/engine.rs crates/savepoint-fs/tests/engine_lifecycle_test.rs && git commit -m "feat(fs): add st_dev routing, CoW delta restore, SQLite WAL cleanup, and SHA-256 content_hash"
```
