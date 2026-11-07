# Savepoint 2026 — Sub-Project 2 Design Spec: Hardened Sandbox & Process Jail (`savepoint-sandbox`)

**Date:** 2026-11-07
**Status:** Approved
**Target Crate:** `savepoint-sandbox`

---

## 1. Context & Problem Statement

In Savepoint 2026 (Agentic Transaction Hypervisor), speculative branches and validation commands execute inside ephemeral Copy-on-Write (CoW) sandboxes (`savepoint-sandbox`). The initial spike of `crates/savepoint-sandbox/src/sandbox.rs` has four critical gaps identified during our adversarial review:

1. **Orphan / Zombie Process Leaks (`PGID` Gap):** `std::process::Command::output()` spawns child processes in the parent's process group. When a test runner or script spawns background daemons (`node`, `vite`, `pytest` workers, database helpers) and exits or times out, those grandchildren survive, holding open ports and file locks indefinitely.
2. **No Timeout or Output Cap:** A hung test or infinite loop blocks the agent forever, and a runaway `yes` or debug log loop can exhaust host memory.
3. **Cold Rebuild & Port Collision Overhead:** Running multiple sandboxes concurrently without environment virtualization causes port collisions (`EADDRINUSE` on port 3000/8080) and forces tools like `cargo`, `npm`, and `pip` to either rebuild from scratch or collide on temp directories.
4. **Cross-Device (`st_dev`) & Directory Promotion Bugs:**
   - `Sandbox::create` hardcoded `~/.savepoint/sandboxes/<id>` and called `clone_tree` on the root directory instead of using `ShadowEngine::resolve_storage_dir` (same `st_dev`) and `clone_workspace` (skipping `.git` and `.savepoint`).
   - `Sandbox::promote_to_workspace` used `fs::copy`, which fails with `EISDIR` on collapsed directory deltas (`node_modules`, `.venv`, `venv`) and does not use CoW `clone_tree`.

---

## 2. Goals & Non-Goals

### Goals
1. **Zero Orphan Processes (`PGID` Supervision):** Every command spawned inside a `Sandbox` runs in its own dedicated Unix process group (`process_group(0)` / `setpgid(0, 0)`). When a command finishes, times out, or when `Sandbox` is dropped, Savepoint sends `SIGKILL` to the entire process group (`libc::kill(-pgid, libc::SIGKILL)`), guaranteeing zero surviving child/grandchild processes.
2. **Bounded Execution (Timeout & Output Cap):** Support configurable command timeouts (`timeout: Option<Duration>`) and cap captured `stdout`/`stderr` streams (default 2 MiB per stream) so runaway processes never hang or OOM the hypervisor.
3. **Environment, Port & Build Cache Virtualization (`SandboxConfig`):**
   - Inject `SAVEPOINT_SANDBOX_ID`, `SAVEPOINT_PORT_OFFSET`, and `PORT` (`base_port + port_offset`, e.g., `3000 + offset`).
   - Isolate `TMPDIR` to a dedicated `.sp-tmp` directory inside the sandbox (excluded from workspace diffing and promotion).
   - Optionally point `CARGO_TARGET_DIR`, `NPM_CONFIG_CACHE`, and `PIP_CACHE_DIR` to shared cache directories under `<workspace_storage>/shared-cache/{cargo-target,npm-cache,pip-cache}` so Rust/Node/Python builds reuse compiled artifacts across sandboxes.
4. **Volume-Aware Creation & CoW Promotion:**
   - Place sandboxes under `<ShadowEngine::resolve_storage_dir(workspace)>/sandboxes/<id>` so `clone_workspace` always operates on the same `st_dev`.
   - Use `clone_workspace` during `Sandbox::create` and `clone_tree` during `Sandbox::promote_to_workspace` (supporting regular files, symlinks, and collapsed `HEAVY_DEP_DIRS` like `node_modules`).

### Non-Goals
- Mandatory kernel-level Seatbelt (`sandbox-exec`) or Linux `bwrap` chrooting by default (avoiding compiler/keychain breakage on developer machines).
- Parallel scheduling or RAM/CPU concurrency governing across multiple sandboxes (scoped to Sub-Project 3: `savepoint-speculate`).

---

## 3. Architecture & Component Design

### 3.1 Data Structures (`crates/savepoint-sandbox/src/sandbox.rs`)

```rust
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    pub id: Option<String>,
    pub port_offset: u16,
    pub base_port: u16,
    pub default_timeout: Option<Duration>,
    pub max_output_bytes: usize,
    pub share_build_cache: bool,
    pub extra_env: HashMap<String, String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            id: None,
            port_offset: 0,
            base_port: 3000,
            default_timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 2 * 1024 * 1024, // 2 MiB
            share_build_cache: true,
            extra_env: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u128,
    pub timed_out: bool,
}
```

`Sandbox` holds:
- `pub id: String`
- `pub path: PathBuf` (the sandbox root directory)
- `pub source: PathBuf` (canonical source workspace root)
- `pub storage_root: PathBuf` (`ShadowEngine::resolve_storage_dir(&source)`)
- `pub config: SandboxConfig`
- `pub auto_cleanup: bool`
- `active_pgids: Arc<Mutex<HashSet<i32>>>` (tracks any process group IDs spawned by this sandbox so `Drop` or `destroy()` can reap them even if interrupted)

### 3.2 Volume-Aware Creation (`Sandbox::create` & `Sandbox::create_with_config`)
1. Canonicalize `source_workspace`.
2. Resolve `storage_root = ShadowEngine::resolve_storage_dir(&canonical_source)`.
3. Compute `sandbox_dir = storage_root.join("sandboxes").join(&sandbox_id)`.
4. Remove existing `sandbox_dir` if present, then invoke `savepoint_fs::clone_workspace(&canonical_source, &sandbox_dir)` so `.git` and `.savepoint` are skipped and APFS `clonefile(2)` / Linux `FICLONE` is used on the same `st_dev`.
5. Create `<sandbox_dir>/.sp-tmp` for `TMPDIR` isolation (and update `savepoint_fs::diff::HEAVY_CACHE_DIRS` to include `.sp-tmp` so temporary files created inside `$TMPDIR` never pollute `compute_deltas` or `promote_to_workspace`).

### 3.3 Process Group (`PGID`) Jail & Timeout Runner (`run_command` / `run_command_with_timeout`)
1. Configure `std::process::Command::new(cmd)`:
   - `.args(args)`
   - `.current_dir(&self.path)`
   - `.stdin(Stdio::null())`, `.stdout(Stdio::piped())`, `.stderr(Stdio::piped())`
   - On Unix (`#[cfg(unix)]`), apply `std::os::unix::process::CommandExt::process_group(0)` so the child becomes the leader of a new process group with `pgid == child.id() as i32`.
   - Inject virtualized environment variables:
     - `SAVEPOINT_SANDBOX_ID = &self.id`
     - `SAVEPOINT_PORT_OFFSET = self.config.port_offset.to_string()`
     - `PORT = (self.config.base_port + self.config.port_offset).to_string()`
     - `TMPDIR = self.path.join(".sp-tmp")`
     - If `self.config.share_build_cache` is true, create and inject:
       - `CARGO_TARGET_DIR = self.storage_root.join("shared-cache/cargo-target")`
       - `NPM_CONFIG_CACHE = self.storage_root.join("shared-cache/npm-cache")`
       - `PIP_CACHE_DIR = self.storage_root.join("shared-cache/pip-cache")`
     - Apply any entries in `self.config.extra_env`.
2. Spawn the child process and record `pgid = child.id() as i32` in `self.active_pgids`.
3. Spawn two lightweight reader threads to drain `child.stdout` and `child.stderr` up to `self.config.max_output_bytes` (continuing to discard excess bytes to prevent SIGPIPE/pipe buffer deadlock if the child writes more than `max_output_bytes`).
4. Poll `child.try_wait()` in a short sleep loop (e.g., 5ms) up to `effective_timeout`:
   - If the child exits within the timeout, **still send `unsafe { libc::kill(-pgid, libc::SIGKILL) }`** to reap any background daemon processes spawned inside that process group!
   - If the timeout expires, set `timed_out = true`, send `unsafe { libc::kill(-pgid, libc::SIGKILL) }`, call `let _ = child.wait()`, and return `CommandOutput` with `exit_code = -1` (or `124`), `timed_out = true`.
5. Remove `pgid` from `self.active_pgids` and join the stdout/stderr reader threads.

### 3.4 CoW Workspace Promotion (`promote_to_workspace`)
1. Compute deltas between `dest_workspace` and `self.path`:
   `let deltas = compute_deltas(dest_workspace, &self.path)?;`
2. For each `delta` in `&deltas`:
   - `ChangeKind::Created | ChangeKind::Modified`:
     Invoke `savepoint_fs::clone_tree(&sandbox_file, &target_file)?;` (handles regular files, symlinks, and collapsed directories like `node_modules` via CoW).
   - `ChangeKind::Deleted`:
     Remove `target_file` (`remove_dir_all` if non-symlink directory, else `remove_file`).
3. Return `Ok(deltas)`.

### 3.5 Cleanup & Drop Safety (`destroy` & `Drop`)
- Both `destroy()` and `Drop::drop()` iterate over `self.active_pgids` and send `libc::kill(-pgid, libc::SIGKILL)` before removing `self.path` (including `.sp-tmp`).

---

## 4. Verification & Testing Strategy

1. **`test_pgid_kills_background_grandchild_daemons`:**
   - Run a shell command in `Sandbox` that spawns a background grandchild (`sh -c "sleep 60 & echo spawned"`).
   - Verify that as soon as `run_command` returns, the background `sleep 60` process in that process group has been terminated by `kill(-pgid, SIGKILL)`.
2. **`test_command_timeout_kills_hung_process_group`:**
   - Configure a sandbox with a 150ms timeout and run `sh -c "sleep 30"`.
   - Verify `run_command` returns in < 1s with `output.timed_out == true` and `output.exit_code != 0`.
3. **`test_env_port_offset_and_shared_cache_virtualization`:**
   - Create a sandbox with `port_offset: 7`, run `sh -c 'echo "$PORT|$SAVEPOINT_PORT_OFFSET|$CARGO_TARGET_DIR|$TMPDIR"'`, and verify `PORT=3007`, `SAVEPOINT_PORT_OFFSET=7`, `CARGO_TARGET_DIR` points to `shared-cache/cargo-target`, and files written to `$TMPDIR` do not appear in `sandbox.compute_deltas()`.
4. **`test_promote_to_workspace_uses_cow_and_handles_collapsed_node_modules`:**
   - Modify source files and create `node_modules/pkg/index.js` inside the sandbox, call `promote_to_workspace`, and verify both source files and `node_modules/pkg/index.js` are promoted to the destination workspace without `EISDIR`.
