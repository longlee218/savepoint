# Savepoint 2026 — Sub-Project 2 Implementation Plan: Hardened Sandbox & Process Jail (`savepoint-sandbox`)

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden `savepoint-sandbox` with volume-aware (`st_dev`) CoW creation (`clone_workspace`), Unix Process Group (`PGID`) supervision (`kill(-pgid, SIGKILL)` on completion, timeout, and `Drop`), bounded output/timeout execution, environment/port/shared-build-cache virtualization (`SandboxConfig`), and CoW delta promotion (`clone_tree`).

**Architecture:**
- Add `.sp-tmp` to `HEAVY_CACHE_DIRS` in `crates/savepoint-fs/src/diff.rs` so isolated `$TMPDIR` files inside a sandbox never pollute workspace diffs or promotions.
- Extend `crates/savepoint-sandbox` (`Cargo.toml`, `src/lib.rs`, `src/sandbox.rs`) with `SandboxConfig`, `CommandOutput` (including `timed_out: bool`), `Sandbox::create_with_config`, `run_command_with_timeout`, `PGID` process group tracking (`active_pgids`), and `clone_tree`-backed `promote_to_workspace`.
- Verify all invariants with integration tests in `crates/savepoint-sandbox/tests/sandbox_jail_test.rs`.

**Tech Stack:** Rust 2021, `libc` (`kill`, `SIGKILL`), `std::os::unix::process::CommandExt` (`process_group(0)`), `savepoint-core`, `savepoint-fs`.

---

## File Structure & Responsibilities

1. `crates/savepoint-fs/src/diff.rs`
   - Add `".sp-tmp"` to `HEAVY_CACHE_DIRS` and `is_ignored_entry` so sandbox temporary files are ignored during `compute_deltas`.
2. `crates/savepoint-sandbox/Cargo.toml`
   - Add `libc.workspace = true`.
3. `crates/savepoint-sandbox/src/lib.rs`
   - Re-export `CommandOutput`, `Sandbox`, and `SandboxConfig`.
4. `crates/savepoint-sandbox/src/sandbox.rs`
   - Implement `SandboxConfig`, `CommandOutput`, `Sandbox::create`, `Sandbox::create_with_config`, `Sandbox::run_command`, `Sandbox::run_command_with_timeout`, `Sandbox::compute_deltas`, `Sandbox::promote_to_workspace`, `Sandbox::destroy`, and `Drop`.
5. `crates/savepoint-sandbox/tests/sandbox_jail_test.rs`
   - Integration tests covering `PGID` background daemon termination, command timeout + group kill, environment/port/shared-cache/TMPDIR virtualization, and CoW promotion including collapsed `node_modules`.

---

### Task 1: Volume-Aware CoW Sandbox Creation, PGID Jail, Timeout & Env Virtualization (`savepoint-sandbox`)

**Files:**
- Modify: `crates/savepoint-fs/src/diff.rs`
- Modify: `crates/savepoint-sandbox/Cargo.toml`
- Modify: `crates/savepoint-sandbox/src/lib.rs`
- Modify: `crates/savepoint-sandbox/src/sandbox.rs`
- Create: `crates/savepoint-sandbox/tests/sandbox_jail_test.rs`

- [ ] **Step 1: Write failing integration tests in `crates/savepoint-sandbox/tests/sandbox_jail_test.rs`**

```rust
use std::fs;
use std::time::Duration;
use savepoint_sandbox::{Sandbox, SandboxConfig};

#[test]
fn test_pgid_kills_background_grandchild_daemons() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();
    fs::write(ws.join("hello.txt"), "world").unwrap();

    let sandbox = Sandbox::create(ws, Some("pgid_test")).unwrap();
    let pid_file = sandbox.path.join("bg_pid.txt");

    // Spawn a background sleep process inside the command and print its PID
    let cmd = format!(
        "sleep 60 >/dev/null 2>&1 & echo $! > {}",
        pid_file.display()
    );
    let out = sandbox.run_command("sh", &["-c", &cmd]).unwrap();
    assert_eq!(out.exit_code, 0);
    assert!(!out.timed_out);

    let bg_pid: i32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .expect("valid pid");

    // Give the kernel a brief moment to process SIGKILL on the process group
    std::thread::sleep(Duration::from_millis(30));

    // Verify background grandchild process is dead (kill(pid, 0) returns -1 with ESRCH)
    let alive = unsafe { libc::kill(bg_pid, 0) == 0 };
    assert!(
        !alive,
        "Background grandchild PID {} survived after run_command finished!",
        bg_pid
    );
}

#[test]
fn test_command_timeout_kills_hung_process_group() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();
    fs::write(ws.join("main.rs"), "fn main() {}").unwrap();

    let mut cfg = SandboxConfig::default();
    cfg.id = Some("timeout_test".to_string());
    cfg.default_timeout = Some(Duration::from_millis(150));

    let sandbox = Sandbox::create_with_config(ws, cfg).unwrap();
    let out = sandbox.run_command("sh", &["-c", "sleep 30"]).unwrap();

    assert!(out.timed_out, "Expected command to time out");
    assert_ne!(out.exit_code, 0);
    assert!(out.duration_ms < 2000, "Timeout took too long: {}ms", out.duration_ms);
}

#[test]
fn test_env_port_offset_and_shared_cache_virtualization() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();
    fs::write(ws.join("app.txt"), "v1").unwrap();

    let mut cfg = SandboxConfig::default();
    cfg.id = Some("env_test".to_string());
    cfg.port_offset = 7;
    cfg.base_port = 3000;
    cfg.share_build_cache = true;

    let sandbox = Sandbox::create_with_config(ws, cfg).unwrap();
    let out = sandbox
        .run_command(
            "sh",
            &[
                "-c",
                "echo \"hello\" > \"$TMPDIR/scratch.tmp\" && printf \"%s|%s|%s\" \"$PORT\" \"$SAVEPOINT_PORT_OFFSET\" \"$CARGO_TARGET_DIR\"",
            ],
        )
        .unwrap();

    assert_eq!(out.exit_code, 0);
    let parts: Vec<&str> = out.stdout.split('|').collect();
    assert_eq!(parts[0], "3007");
    assert_eq!(parts[1], "7");
    assert!(parts[2].contains("shared-cache/cargo-target"));

    // Files written inside $TMPDIR (.sp-tmp) must NOT pollute workspace deltas
    let deltas = sandbox.compute_deltas().unwrap();
    assert!(
        deltas.is_empty(),
        "Expected 0 deltas when writing only to TMPDIR, got: {:?}",
        deltas
    );
}

#[test]
fn test_promote_to_workspace_uses_cow_and_handles_collapsed_node_modules() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src/lib.rs"), "pub fn v1() {}").unwrap();

    let sandbox = Sandbox::create(ws, Some("promote_test")).unwrap();

    // Modify src/lib.rs and create a collapsed heavy dependency directory node_modules
    fs::write(sandbox.path.join("src/lib.rs"), "pub fn v2() {}").unwrap();
    fs::create_dir_all(sandbox.path.join("node_modules/lodash")).unwrap();
    fs::write(
        sandbox.path.join("node_modules/lodash/index.js"),
        "module.exports = {};",
    )
    .unwrap();

    let promoted_deltas = sandbox
        .promote_to_workspace(ws)
        .expect("promote_to_workspace must succeed on files and collapsed node_modules");

    assert_eq!(promoted_deltas.len(), 2);
    assert_eq!(fs::read_to_string(ws.join("src/lib.rs")).unwrap(), "pub fn v2() {}");
    assert!(ws.join("node_modules/lodash/index.js").exists());
}
```

- [ ] **Step 2: Run test to verify it fails (RED)**

Run: `cargo test --package savepoint-sandbox`
Expected: Compilation failure (`unresolved import savepoint_sandbox::SandboxConfig`, `no field timed_out on type CommandOutput`, `no function create_with_config`).

- [ ] **Step 3: Implement `.sp-tmp` ignore in `savepoint-fs` and hardened `Sandbox` in `savepoint-sandbox`**

1. In `crates/savepoint-fs/src/diff.rs`, add `".sp-tmp"` to `HEAVY_CACHE_DIRS` and `is_ignored_entry`.
2. In `crates/savepoint-sandbox/Cargo.toml`, add `libc.workspace = true`.
3. In `crates/savepoint-sandbox/src/lib.rs`, export `SandboxConfig`.
4. In `crates/savepoint-sandbox/src/sandbox.rs`, implement `SandboxConfig`, `CommandOutput`, `Sandbox`, `PGID` process group supervision, bounded stdout/stderr reader threads, environment/port/shared-cache virtualization, and `clone_tree` promotion.

- [ ] **Step 4: Run tests to verify they pass (GREEN)**

Run: `cargo test --package savepoint-fs && cargo test --package savepoint-sandbox`
Expected: PASS (all tests in `savepoint-fs` and `savepoint-sandbox` pass).

- [ ] **Step 5: Commit**

```bash
git add crates/savepoint-fs/src/diff.rs crates/savepoint-sandbox/Cargo.toml crates/savepoint-sandbox/src/lib.rs crates/savepoint-sandbox/src/sandbox.rs crates/savepoint-sandbox/tests/sandbox_jail_test.rs
git commit -m "feat(sandbox): add PGID process group jail, timeout reaping, env/cache virtualization, and CoW promote"
```

---

## Review Focus

- **Process Group (`PGID`) Reaping Safety:** Verify `CommandExt::process_group(0)` sets `pgid == child.id() as i32` and `kill_process_group(pgid)` sends `libc::kill(-pgid, libc::SIGKILL)` (never `pgid <= 1`) both after normal completion, on timeout, and in `Drop` / `destroy()`.
- **Pipe Deadlock Prevention:** Verify `stdout` and `stderr` are drained concurrently on background threads while waiting on `child.try_wait()`, and excess output beyond `max_output_bytes` is discarded rather than closing the pipe early or blocking the child.
- **Cross-Crate Seam (`savepoint-fs` ↔ `savepoint-sandbox`):** Verify `Sandbox::create_with_config` places sandboxes under `ShadowEngine::resolve_storage_dir(&canonical_source).join("sandboxes")`, clones with `clone_workspace`, and `promote_to_workspace` uses `clone_tree` so collapsed `node_modules` deltas promote cleanly without `EISDIR`.
