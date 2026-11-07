# Savepoint Sub-Project 1: Core CoW, Volume-Aware Storage & Two-Stage Fast Diff Design

- **Date:** 2026-11-07
- **Status:** Draft (Pending Builder Check & User Approval)
- **Scope:** `savepoint-core`, `savepoint-fs` (Sub-project 1 of 4 in the Savepoint 2026 Master Plan)

---

## 1. Intent and Why

Savepoint is evolving from a linear file-backup CLI into an **Agentic Transaction Hypervisor (ATH)** capable of sub-millisecond Copy-on-Write (CoW) snapshots, Merkle DAG branching, and instant rollbacks across entire execution environments (source code, `.env`, local SQLite databases, `node_modules/`, `target/`, `.venv/`).

In the current codebase (`savepoint-fs`), three critical bottlenecks prevent real-world adoption on production repositories:
1. **Cross-Device (`EXDEV`) vs. Recursive Clone (`EINVAL`) Trap:** Storing snapshots unconditionally in `~/.savepoint` fails with `EXDEV (errno 18)` if the project lives on an external drive or separate mount point. Conversely, storing snapshots inside `<workspace>/.savepoint` and calling `clonefile(workspace_root, dst)` fails with `EINVAL` on macOS APFS because a directory cannot be cloned into a subdirectory of itself.
2. **O(Total Bytes) Diff Bottleneck on `node_modules` and `target`:** `compute_deltas()` in `crates/savepoint-fs/src/diff.rs` currently checks `old_size == new_size` and immediately invokes `fs::read()` on both files to compare bytes. In a project with 50,000 unchanged files in `node_modules/` or `target/`, every `sp save`, `sp diff`, or 1.5-second PTY auto-save check reads gigabytes of unchanged files into RAM.
3. **Byte-Copy Rollback (`fs::copy`) & Metadata Bloat:** `ShadowEngine::rewind()` currently uses `fs::copy()` instead of CoW cloning to restore modified files, and `meta.json` records every individual binary artifact inside `target/` or `node_modules/` if a build runs during a transaction.

Sub-project 1 solves these three foundational problems in `savepoint-core` and `savepoint-fs` so that Sub-projects 2 (`savepoint-sandbox`), 3 (`savepoint-speculate`), and 4 (`savepoint-mcp` + `.spbundle`) build on a rock-solid kernel layer.

---

## 2. Goals, Non-Goals, and Anti-Goals

### Goals
1. **Volume-Aware Storage Routing (`st_dev`):** Automatically detect whether `~/.savepoint` shares the same filesystem device ID (`st_dev`) as `workspace_root`. Use `~/.savepoint/workspaces/<ws_hash>` when on the same volume; route to a same-volume sibling store `<workspace_parent>/.savepoint-spool/<ws_hash>` (or fallback in-workspace `.savepoint` with top-level entry cloning and `.git/info/exclude` injection) when on a different mount point.
2. **Entry-Filtered CoW Tree Cloning:** Clone workspace top-level entries via `clonefile(2)` (macOS APFS) or `ioctl(FICLONE)` (Linux Btrfs/XFS) while explicitly skipping `.git` and `.savepoint`. Preserve `node_modules/`, `target/`, `.venv/`, `.env`, and local databases in the snapshot at zero disk overhead.
3. **Two-Stage Stat-First Diff Engine:**
   - **Stage 1 (Stat Fast-Path):** Compare `(size, mtime_sec, mtime_nsec)` from `std::fs::Metadata`. Because `clonefile(2)` and `FICLONE` preserve modification timestamps, unchanged files are skipped in O(1) metadata time with **zero file opens or reads**.
   - **Stage 2 (Streamed Chunk Comparison):** Only when `mtime` differs AND `size` is equal, compare file contents using a 64KB buffered stream (never loading whole multi-megabyte files into heap via `fs::read()`).
4. **Heavy Build-Cache Directory Collapse in Diffs:** Keep full CoW clones of heavy cache/dependency directories (`node_modules`, `target`, `.next`, `.venv`, `__pycache__`, `.gradle`, `dist`, `build`) inside snapshots so instant rollback and sandbox execution work out-of-the-box, but collapse or exclude their internal files from fine-grained per-file `Checkpoint.deltas` in `meta.json` unless configured otherwise—While still restoring them accurately on `rewind`!
5. **Per-Delta CoW Restore & SQLite WAL Safety:** During `rewind` and `merge_branch`, restore changed/deleted files using CoW `clone_tree` (unlink + `clonefile`/`FICLONE`) instead of `fs::copy`. When rolling back a SQLite database file (`*.db`, `*.sqlite`, `*.sqlite3`), ensure orphaned `-wal` and `-shm` files not present in the target snapshot are removed so SQLite does not replay stale WAL frames over a rolled-back database.
6. **Merkle DAG Branching in `savepoint-core` & `savepoint-fs`:** Support `fork_branch`, `switch_branch`, and `merge_branch` with parent checkpoint tracking (`parent_step`, `branch`, `content_hash`) and deterministic branch head pointers.

### Non-Goals (Deferred to Later Sub-Projects)
- Process group (`PGID`) supervision, `PORT_OFFSET` injection, and shared `CARGO_TARGET_DIR` (Sub-project 2: `savepoint-sandbox`).
- Parallel MCTS execution and RAM/CPU throttling (Sub-project 3: `savepoint-speculate`).
- `.spbundle` Flight Recorder serialization and Network Egress Proxy (Sub-project 4).

### Anti-Goals (Failure Conditions Even If It Technically Works)
- **Breaking Active Terminals or IDEs:** Replacing the `workspace_root` directory inode itself (which orphans the `cwd` of running shells/agents) or replacing unchanged file inodes (which forces `rust-analyzer`, `tsserver`, and Cursor to re-index the entire project).
- **Reading Unchanged Files During Diff:** Opening or reading bytes from unchanged files during `compute_deltas`.
- **Polluting `git status`:** Leaving any untracked Savepoint files visible to `git status` in the user's repository.
- **Bloating `meta.json`:** Writing 50,000 `FileDelta` entries into `meta.json` when an agent runs `npm install` or `cargo build`.

---

## 3. Constraints

- **Language & Toolchain:** Rust 2021 edition, building cleanly within the existing Cargo workspace (`savepoint-core`, `savepoint-fs`, `savepoint-cli`, `savepoint-pty`, `savepoint-tui`).
- **OS Compatibility:**
  - **macOS:** Native `clonefile(2)` via `libc`.
  - **Linux:** Reflink `ioctl(fd, FICLONE, src_fd)` on supported filesystems (Btrfs, XFS, OCFS2), with automatic graceful fallback to streamed copy (`std::io::copy` + permission/mtime preservation via `filetime` or `fs::File::set_modified`) on `ext4` / `tmpfs`.
- **Backwards Compatibility:** Existing `Transaction` and `Checkpoint` JSON files (`meta.json`) must deserialize without error using `#[serde(default)]`.

---

## 4. Approaches Considered

| Approach | Summary | Trade-offs | Decision |
| :--- | :--- | :--- | :--- |
| **1. Device-ID Routing + Two-Stage Stat Diff + Per-Delta CoW Restore** | Route storage by `st_dev`; clone top-level entries skipping `.git`/`.savepoint`; diff via `(mtime_ns, size)` before 64KB chunked read; restore only delta paths via CoW. | Preserves `workspace_root` and unchanged file inodes (zero IDE/terminal disruption). Requires smart handling of heavy directories (`node_modules`, `target`) so rollback restores them without bloating `meta.json`. | **Selected** |
| **2. Atomic Whole-Directory Swap (`rename`)** | Clone snapshot to a temp path and swap `workspace_root` directly on rollback. | O(1) rollback, but destroys `workspace_root` inode, breaking `cwd` of running terminals/agents and forcing full IDE re-indexing. | Rejected |
| **3. In-Workspace `.savepoint` Only** | Always store snapshots inside `<workspace>/.savepoint/`. | Avoids `st_dev` check, but risks polluting file watchers, Docker build contexts, and search tools inside the user's repo. | Rejected as primary (used only as last-resort fallback if parent dir is read-only on an external drive) |

---

## 5. Detailed Technical Design

### 5.1 Volume-Aware Storage Resolution (`savepoint-fs/src/engine.rs`)

When resolving the storage directory for `workspace_root`:
1. If `SAVEPOINT_DIR` environment variable is explicitly set, use `<SAVEPOINT_DIR>/workspaces/<ws_hash>` (provided it is on the same device `st_dev` or fallback copy is acceptable in tests).
2. Otherwise, check the candidate primary directory `~/.savepoint/workspaces/<ws_hash>`:
   - Ensure `~/.savepoint` exists and query `fs::metadata(&home_savepoint)?.dev()` vs. `fs::metadata(workspace_root)?.dev()` (using `std::os::unix::fs::MetadataExt`).
   - **Case A (`st_dev` matches):** Use `~/.savepoint/workspaces/<ws_hash>`.
   - **Case B (`st_dev` differs — e.g., external USB/Thunderbolt APFS volume):**
     - Try creating `<workspace_parent>/.savepoint-spool/<ws_hash>` and verify its `st_dev` matches `workspace_root`.
     - **Case C (Fallback if `workspace_parent` is not writable or different mount):** Use `<workspace_root>/.savepoint/workspaces/<ws_hash>` and automatically append `.savepoint/` to `<workspace_root>/.git/info/exclude` (if `.git/info` exists and does not already contain `.savepoint/`).

### 5.2 Top-Level Entry CoW Cloning (`savepoint-fs/src/clone.rs` & `darwin.rs` / `linux.rs`)

Instead of calling `clonefile(workspace_root, snapshot_dir)` on the root folder itself (which clones `.git/` and fails with `EINVAL` in Case C):
- `clone_workspace(workspace_root: &Path, snapshot_dir: &Path)` creates `snapshot_dir` and iterates `fs::read_dir(workspace_root)`:
  - Skips `.git` and `.savepoint`.
  - For every other top-level entry (including `src`, `Cargo.toml`, `.env`, `node_modules`, `target`, `.venv`), calls `clone_tree(&entry_path, &snapshot_dir.join(entry_name))`.
  - On macOS, `clone_tree` invokes `apfs_clone(src, dst, true)` (`clonefile(2)` with `CLONE_NOFOLLOW`). A directory `node_modules` with 50,000 files is cloned in a single kernel call (`< 1ms`) because `clonefile` operates on the directory root `node_modules`.
  - On Linux, `clone_tree` walks directories, attempts `ioctl(dst_fd, FICLONE, src_fd)` for regular files, and preserves `mtime` (`File::set_modified`) so Stage 1 stat diff works identically on Linux and macOS.
  - If `apfs_clone` returns `EXDEV` or `ENOTSUP` (e.g., non-APFS mount like FAT32/exFAT/NFS), `clone_tree` automatically falls back to `fallback_copy_tree` (which also preserves file modification times via `File::set_modified`).

### 5.3 Two-Stage Fast Diff & Heavy-Directory Handling (`savepoint-fs/src/diff.rs`)

#### Two-Stage File Comparison
For regular files present in both `base_dir` and `current_dir`:
1. Read `std::fs::symlink_metadata` (does not follow broken symlinks).
2. **Stage 1 (Metadata Fast-Path):**
   - Extract `(size, mtime_sec, mtime_nsec)` (using `MetadataExt::mtime()` and `mtime_nsec()` on Unix).
   - If `base_size != curr_size`: immediately classify as `ChangeKind::Modified` (no file open/read needed!).
   - If `base_size == curr_size` AND `(base_mtime_sec, base_mtime_nsec) == (curr_mtime_sec, curr_mtime_nsec)`: **immediately classify as Unchanged** (0 file opens, 0 bytes read!).
3. **Stage 2 (Streamed 64KB Chunk Comparison):**
   - Only reached when `base_size == curr_size` AND `mtime` differs (e.g., file was `touch`ed or overwritten with same byte length).
   - Open both files with `BufReader::with_capacity(64 * 1024, file)` and compare chunk-by-chunk, returning `true` on the first differing chunk.

#### Heavy Build/Dependency Directories (`OpaqueDir` Collapse vs. Deep Scan)
To prevent `meta.json` from exploding to 50,000 entries while still supporting instant rollback of `node_modules` and `target`:
- Define default heavy directories: `node_modules`, `target`, `.next`, `.venv`, `venv`, `__pycache__`, `.gradle`, `.turbo`, `.cache`.
- In `compute_deltas(base_dir, current_dir)`:
  - For normal source directories: walk recursively using the Two-Stage File Comparison.
  - For heavy directories (e.g., `node_modules` or `target`):
    - Do a fast shallow/directory-level check or bounded walk: if any file/directory mtime or entry inside a heavy directory changed (or if comparing for `rewind`), emit a **single directory-level `FileDelta`** (e.g., `relative_path: "node_modules"`, `kind: ChangeKind::Modified` / `Created` / `Deleted`) rather than 50,000 individual file deltas!
    - How to detect if a heavy directory changed in `< 1ms` without walking 50,000 files during the 1.5s PTY auto-save loop?
      - Distinguish between:
        1. `compute_deltas(base, current)` (used for `sp save`, `sp diff`, and PTY auto-save): by default skips walking inside heavy build-cache directories (`target`, `.next`, `.cache`, `__pycache__`, `.turbo`) so compiling code doesn't spam auto-save checkpoints, and for dependency directories (`node_modules`, `.venv`) checks the directory root `mtime` + lockfile (`package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`, `uv.lock`, `Cargo.lock`)!
        2. On `rewind` / `merge_branch`: after restoring source file deltas via `clone_tree`, also sync top-level heavy directories (`node_modules`, `.venv`) from `target_cp.snapshot_path` via a single atomic `clone_tree` if their root `mtime` or presence differs!

### 5.4 Per-Delta CoW Rewind & SQLite WAL Quiescence (`savepoint-fs/src/engine.rs`)

When `ShadowEngine::rewind(tx, target_step)` runs:
1. **Safety-Net Checkpoint:** If `compute_live_deltas(tx)` is non-empty, capture an automatic checkpoint before jumping.
2. **Delta Computation:** Compute deltas between `target_cp.snapshot_path` and `tx.workspace_root`.
3. **Per-Delta CoW Restoration:**
   - `ChangeKind::Created` (exists in workspace, not in target snapshot): remove file/dir in workspace.
   - `ChangeKind::Modified | ChangeKind::Deleted` (modified or deleted in workspace compared to target snapshot):
     - Instead of `fs::copy(&snapshot_file, &target_file_in_ws)`, call `clone_tree(&snapshot_file, &target_file_in_ws)`.
     - Preserve `mtime` from the snapshot so subsequent diffs see the restored file as clean (`mtime` matches snapshot).
4. **SQLite WAL/SHM Cleanup:**
   - Whenever a restored or deleted file has extension `.db`, `.sqlite`, or `.sqlite3` (e.g., `dev.db`), check for companion files `dev.db-wal` and `dev.db-shm` in `tx.workspace_root`.
   - If the target snapshot does not contain `dev.db-wal` / `dev.db-shm`, remove them from `tx.workspace_root` before/during restoring `dev.db` so SQLite does not corrupt the restored database with stale WAL frames.

### 5.5 Content Hashing for Merkle DAG Checkpoints

When `ShadowEngine::checkpoint` records a `Checkpoint`:
- Compute a deterministic SHA-256 `content_hash` over `(parent_step, branch, sorted deltas: [relative_path, kind, old_size, new_size])` using the `sha2` crate already in `[workspace.dependencies]`.
- Store `content_hash: Some(hex_digest)` on `Checkpoint`.

---

## 6. What's Left to the Builder

- Exact helper module split inside `savepoint-fs` (e.g., whether Linux `FICLONE` lives in `src/linux.rs` or inside `src/clone.rs`).
- Exact buffer size constant for chunked file comparison (default 64KB).
- Unit and integration test structure inside `crates/savepoint-fs/tests/` (must test: two-stage stat diff skipping unchanged files, `mtime` preservation on fallback copy, SQLite `-wal`/`-shm` cleanup on rewind, `st_dev` routing, and Merkle `content_hash` determinism).
