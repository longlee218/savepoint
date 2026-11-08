# Savepoint Architecture Overview

Savepoint acts as an **Agentic Transaction Hypervisor (ATH)** sitting between AI Coding Agents (such as Claude Code, Cursor, Aider, Codex) and the host filesystem/operating system.

---

## High-Level Topology

```
                  ┌────────────────────────────────────────┐
                  │       Autonomous AI Coding Agent       │
                  │   (Claude Code / Cursor / Aider)       │
                  └──────────────────┬─────────────────────┘
                                     │
               ┌─────────────────────┼─────────────────────┐
               ▼                     ▼                     ▼
      [ PTY Supervisor ]     [ MCP JSON-RPC ]      [ Direct CLI / TUI ]
      (savepoint-pty)        (savepoint-mcp)       (savepoint-tui)
               │                     │                     │
               └─────────────────────┼─────────────────────┘
                                     ▼
                  ┌────────────────────────────────────────┐
                  │          Shadow Engine Core            │
                  │            (savepoint-fs)              │
                  │                                        │
                  │  • Volume-Aware Storage (st_dev)       │
                  │  • Copy-on-Write (clonefile / FICLONE) │
                  │  • Two-Stage Fast Diff Engine          │
                  │  • Lifecycle Hooks (.sp-hooks.yaml)    │
                  │  • Flight Recorder (.spbundle)         │
                  │  • Git Bridge (squash to git)          │
                  └──────────────────┬─────────────────────┘
                                     │
               ┌─────────────────────┴─────────────────────┐
               ▼                                           ▼
      [ Speculative Engine ]                      [ Hardened Jail ]
     (savepoint-speculate)                      (savepoint-sandbox)
      • Resource Governor                        • Unix PGID Group Kill
      • Parallel Worker Pool                     • Bounded Pipe Drain
      • FirstPass / BestPass                     • Network Egress Guard
```

---

## Subsystem Details

### 1. Zero-Copy Copy-on-Write (`savepoint-fs`)
Savepoint avoids deep file copies by using kernel-level block deduplication:
- **macOS:** Native `clonefile(2)` system call clones directory inodes in sub-milliseconds without duplicating disk blocks.
- **Linux:** Btrfs/XFS/bcachefs `ioctl(FICLONE)` or fallback block copy preserving exact microsecond modification times (`mtime_nsec`).
- **Volume-Aware Device Routing (`st_dev`):** Cross-device links (`EXDEV`) are eliminated by detecting whether `workspace_root` shares a filesystem device with `~/.savepoint`. If on external drives, storage spools to sibling mount points automatically.

### 2. Hardened Sandbox Jail (`savepoint-sandbox`)
When agents run speculative commands or tests:
- **Process Group Isolation:** Every child process is spawned inside its own Unix Process Group (`setpgid(0, 0)`).
- **Zombie Reaper:** On exit, timeout, or branch cancellation, `libc::kill(-pgid, SIGKILL)` terminates the entire process tree (including grandchild daemon servers, dev watchers, and orphaned background runners).
- **Environment & Port Virtualization:** `PORT` and `TMPDIR` are assigned unique ephemeral offsets (`.sp-tmp`), while heavy package manager caches (`CARGO_TARGET_DIR`, `NPM_CONFIG_CACHE`) are shared read-safely across sandboxes.

### 3. Speculative Multi-Branch Execution (`savepoint-speculate`)
Enables running multiple model strategies or candidate patches concurrently:
- **Resource Governor:** Dynamically throttles concurrency using system capacity:
  $$\min\left(\lfloor \text{RAM} / 4 \rfloor, \lfloor \text{Cores} / 2 \rfloor, 3\right)$$
- **Dual Convergence:**
  - `FirstPassWins`: First candidate to pass validation cancels queued and mid-flight runners immediately.
  - `BestPassWins`: Evaluates all candidates and promotes the fastest, cleanest patch with the fewest changed files.

### 4. Portable Flight Recorder (`.spbundle`)
- Packages complete workspace states and multi-turn deltas into an audit-safe JSON bundle.
- **Secret Scrubber:** Automatically detects and excludes `.env*`, `.pem`, `.key`, `id_rsa*` files, and scrubs API keys matching prefixes (`sk-`, `ghp_`, `AKIA`, `AIza`, `xoxb-`).
- **Replay Protection:** Validates against symlink directory escapes and provides `--dry-run` inspection before applying.
