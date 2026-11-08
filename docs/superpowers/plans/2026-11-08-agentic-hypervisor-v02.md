# Savepoint v0.2.0: Agentic Transaction Hypervisor (ATH) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transform Savepoint into the Agentic Transaction Hypervisor (ATH) by extending state virtualization to databases (SQLite/Postgres), in-memory queues (Redis/BullMQ), external API mutations (Saga compensator/VCR), content-addressable deduplication (FastCDC/BLAKE3), and Git split-brain reconciliation.

**Architecture:** Savepoint ATH introduces 5 specialized modular engines integrated into the checkpoint lifecycle: `savepoint-db` (sub-millisecond WAL clones and template DB branching), `savepoint-redis` (keyspace sandboxing and ghost job outbox holding tanks), `savepoint-egress` (transparent proxy, mutation interceptor, and saga undo compensation), `savepoint-cas` (FastCDC chunking with BLAKE3 deduplicated storage), and `savepoint-git` (GitWitness delta tracking and auto soft-reset). Checkpoints orchestrate atomic snapshots and rollbacks across all engines in a unified transaction.

**Tech Stack:** Rust 1.80+ (2021 Edition), Tokio async runtime, FastCDC chunker, BLAKE3 cryptohash, Zstandard compression, Hyper/Reqwest proxy interceptor, Rusqlite / SQLite WAL C-ABI bindings, Git2/libgit2 bindings, Ratatui & Crossterm TUI.

**Spec:** `docs/superpowers/specs/2026-11-08-agentic-hypervisor-v02-design.md`

---

## Global Constraints

- All crates must compile with zero compiler warnings and pass `cargo clippy --all-targets --workspace -- -D warnings`.
- No regression on existing v0.1.0 functionality: all 32 current workspace tests must continue to pass.
- SQLite WAL snapshot and rewind latency must remain under 5.0ms on standard SSDs (< 1.0ms on Apple Silicon APFS).
- External API credentials (Authorization, Bearer tokens, API keys) must be automatically sanitized before persisting to VCR cassettes or Saga journals.
- Pure Rust or standard OS C-bindings only; no hard dependency on external system daemons (Postgres/Redis drivers fail gracefully or mock when the daemon is not running locally).
- Every task must strictly follow Test-Driven Development (TDD): write the failing test first, verify failure, implement minimal code, verify pass, and commit.

---

## Review Focus

1. **SQLite WAL Concurrency Deadlock:** Agent process writing to SQLite while ATH initiates passive checkpoint. ATH must use `PRAGMA wal_checkpoint(PASSIVE)` to prevent blocking active write transactions.
2. **Ghost Job Leakage:** Redis queue worker picking up speculative background jobs before agent completes. ATH Outbox Holding Tank must buffer `LPUSH`/`RPUSH`/`XADD` until explicit commit, dropping all uncommitted jobs on rewind.
3. **External Saga Compensator Partial Failure:** Third-party API returns 500 or network timeout during compensating refund/delete. ATH must record uncompensated state in `saga_unresolved.json` and alert developer with actionable curl remedies.
4. **Git Index Desynchronization:** Agent commits to Git while workspace has unstaged edits. ATH `GitWitness` must detect tree divergence and execute safe `git reset --soft` without wiping unstaged developer modifications.
5. **CAS Garbage Collection Orphan Detection:** Multi-threaded or concurrent checkpoint creation during `sp gc`. Mark-and-sweep must maintain read lock on active checkpoint manifests and grace-period threshold (15 minutes) for newly created chunks.

---

## Task Decomposition

### Task 1: SQLite WAL-Passive Clone & Database Brancher (`savepoint-db`)

**Files:**
- Create: `crates/savepoint-db/Cargo.toml`
- Create: `crates/savepoint-db/src/lib.rs`
- Create: `crates/savepoint-db/src/sqlite.rs`
- Create: `crates/savepoint-db/src/postgres.rs`
- Create: `crates/savepoint-db/tests/sqlite_wal_test.rs`
- Modify: `Cargo.toml:1-12` (add `crates/savepoint-db` to workspace)
- Modify: `crates/savepoint-core/Cargo.toml` (depend on `savepoint-db`)

- [ ] **Step 1: Create `savepoint-db` crate skeleton and write failing test for SQLite WAL clone**
```rust
// crates/savepoint-db/tests/sqlite_wal_test.rs
use std::fs;
use tempfile::tempdir;
use savepoint_db::sqlite::{SqliteHypervisor, SqliteSnapshotConfig};

#[tokio::test]
async fn test_sqlite_wal_snapshot_and_rewind_cycle() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test_app.db");
    
    // 1. Initialize SQLite database in WAL mode
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch("
        PRAGMA journal_mode=WAL;
        CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
        INSERT INTO users (name) VALUES ('Alice');
    ").unwrap();
    drop(conn);

    let hypervisor = SqliteHypervisor::new();
    let snapshot_dir = dir.path().join(".sp_snapshots");
    fs::create_dir_all(&snapshot_dir).unwrap();

    // 2. Snapshot checkpoint at Step 1 (Users: Alice)
    let snap_id = "tx_step1";
    let manifest = hypervisor.snapshot(&db_path, &snapshot_dir, snap_id).await.unwrap();
    assert!(manifest.wal_enabled);
    assert_eq!(manifest.row_counts.get("users"), Some(&1));

    // 3. Mutate database in Step 2 (Users: Alice, Bob)
    let conn2 = rusqlite::Connection::open(&db_path).unwrap();
    conn2.execute("INSERT INTO users (name) VALUES ('Bob');", []).unwrap();
    let count: i64 = conn2.query_row("SELECT count(*) FROM users", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 2);
    drop(conn2);

    // 4. Rewind back to Step 1
    hypervisor.rewind(&db_path, &snapshot_dir, snap_id).await.unwrap();

    // 5. Verify database reverted atomically to Step 1 (Users: Alice only)
    let conn3 = rusqlite::Connection::open(&db_path).unwrap();
    let reverted_count: i64 = conn3.query_row("SELECT count(*) FROM users", [], |r| r.get(0)).unwrap();
    assert_eq!(reverted_count, 1);
    let name: String = conn3.query_row("SELECT name FROM users WHERE id=1", [], |r| r.get(0)).unwrap();
    assert_eq!(name, "Alice");
}
```

- [ ] **Step 2: Run test to confirm compilation/test failure**
```bash
cargo test -p savepoint-db --test sqlite_wal_test
```
*(Fails because `SqliteHypervisor` is not yet implemented).*

- [ ] **Step 3: Implement `savepoint-db` engine**
  - Implement `SqliteHypervisor` with `PRAGMA wal_checkpoint(PASSIVE)` invocation.
  - Use `clonefile` on macOS / kernel CoW on Linux to copy `.db` and `.db-wal` without copying volatile `.db-shm`.
  - Implement atomic file replacement on `rewind`.
  - Add `PostgresBrancher` stub with `CREATE DATABASE ... TEMPLATE` connection helper.

- [ ] **Step 4: Run test to confirm pass**
```bash
cargo test -p savepoint-db --test sqlite_wal_test
```
*(Passes with latency < 5ms).*

- [ ] **Step 5: Commit**
```bash
git add crates/savepoint-db Cargo.toml
git commit -m "feat(db): implement SQLite WAL-passive CoW hypervisor and database brancher"
```

---

### Task 2: In-Memory Keyspace Virtualizer & Queue Outbox Tank (`savepoint-redis`)

**Files:**
- Create: `crates/savepoint-redis/Cargo.toml`
- Create: `crates/savepoint-redis/src/lib.rs`
- Create: `crates/savepoint-redis/src/keyspace.rs`
- Create: `crates/savepoint-redis/src/outbox.rs`
- Create: `crates/savepoint-redis/tests/queue_outbox_test.rs`
- Modify: `Cargo.toml:1-12` (add `crates/savepoint-redis` to workspace)

- [ ] **Step 1: Write failing test for Queue Outbox holding tank**
```rust
// crates/savepoint-redis/tests/queue_outbox_test.rs
use savepoint_redis::outbox::{QueueOutbox, OutboxJob};

#[tokio::test]
async fn test_queue_outbox_drops_ghost_jobs_on_rewind() {
    let mut outbox = QueueOutbox::new();
    let tx_id = "tx_speculate_01";

    // 1. Agent pushes 2 background jobs during speculation
    outbox.enqueue(tx_id, OutboxJob {
        queue_name: "email_queue".into(),
        payload: b"{\"to\":\"user@example.com\",\"subject\":\"Welcome\"}".to_vec(),
    });
    outbox.enqueue(tx_id, OutboxJob {
        queue_name: "billing_queue".into(),
        payload: b"{\"charge_id\":\"ch_123\"}".to_vec(),
    });

    assert_eq!(outbox.pending_count(tx_id), 2);

    // 2. Speculation fails: Agent triggers rewind
    outbox.discard(tx_id);
    assert_eq!(outbox.pending_count(tx_id), 0);

    // 3. Worker drains queue: 0 jobs should have been emitted
    let flushed = outbox.flush_all_committed();
    assert!(flushed.is_empty(), "Ghost jobs must be completely dropped on rewind");
}
```

- [ ] **Step 2: Run test to confirm failure**
```bash
cargo test -p savepoint-redis --test queue_outbox_test
```

- [ ] **Step 3: Implement `QueueOutbox` and `KeyspaceSanitizer`**
  - Implement in-memory & file-backed FIFO buffering for queued jobs per transaction ID.
  - Implement `discard(tx_id)` to drop buffered jobs on rewind.
  - Implement `commit(tx_id)` to release buffered jobs to external queue.
  - Implement `RedisKeyspaceSanitizer` producing prefix queries (`UNLINK sp_tx_123:*`).

- [ ] **Step 4: Run test to confirm pass**
```bash
cargo test -p savepoint-redis --test queue_outbox_test
```

- [ ] **Step 5: Commit**
```bash
git add crates/savepoint-redis Cargo.toml
git commit -m "feat(redis): add keyspace sandboxing and ghost job outbox holding tank"
```

---

### Task 3: External API Egress Hypervisor & Saga Compensator (`savepoint-egress`)

**Files:**
- Create: `crates/savepoint-egress/Cargo.toml`
- Create: `crates/savepoint-egress/src/lib.rs`
- Create: `crates/savepoint-egress/src/proxy.rs`
- Create: `crates/savepoint-egress/src/vcr.rs`
- Create: `crates/savepoint-egress/src/saga.rs`
- Create: `crates/savepoint-egress/src/guard.rs`
- Create: `crates/savepoint-egress/tests/saga_compensator_test.rs`
- Modify: `Cargo.toml:1-12` (add `crates/savepoint-egress` to workspace)

- [ ] **Step 1: Write failing test for Saga Compensator reversing external mutations**
```rust
// crates/savepoint-egress/tests/saga_compensator_test.rs
use savepoint_egress::saga::{SagaLog, SagaAction, CompensatingStep};

#[tokio::test]
async fn test_saga_compensator_executes_lifo_undo_actions() {
    let mut saga = SagaLog::new();
    let tx_id = "tx_payment_flow";

    // Action 1: Create Stripe customer
    saga.record_action(tx_id, SagaAction {
        id: "act_1".into(),
        description: "POST /v1/customers".into(),
        compensator: CompensatingStep::MockUndo {
            undo_label: "DELETE /v1/customers/cus_999".into(),
            payload: b"{}".to_vec(),
        },
    });

    // Action 2: Create Stripe charge
    saga.record_action(tx_id, SagaAction {
        id: "act_2".into(),
        description: "POST /v1/charges".into(),
        compensator: CompensatingStep::MockUndo {
            undo_label: "POST /v1/refunds".into(),
            payload: b"{\"charge\":\"ch_888\"}".to_vec(),
        },
    });

    assert_eq!(saga.unwound_count(tx_id), 0);

    // Rollback: trigger compensating actions in LIFO order
    let executions = saga.rollback(tx_id).await.unwrap();
    assert_eq!(executions.len(), 2);
    assert_eq!(executions[0].undo_label, "POST /v1/refunds"); // LIFO: charge refunded first
    assert_eq!(executions[1].undo_label, "DELETE /v1/customers/cus_999"); // customer deleted second
}
```

- [ ] **Step 2: Run test to confirm failure**
```bash
cargo test -p savepoint-egress --test saga_compensator_test
```

- [ ] **Step 3: Implement `savepoint-egress` proxy, VCR cassette recorder, and Saga journal**
  - Implement `SagaLog` with LIFO rollback execution.
  - Implement `VcrCassetteEngine` to record and replay HTTP requests/responses, masking sensitive tokens.
  - Implement `MutationGuard` flagging write HTTP methods (`POST`, `PUT`, `DELETE`).

- [ ] **Step 4: Run test to confirm pass**
```bash
cargo test -p savepoint-egress --test saga_compensator_test
```

- [ ] **Step 5: Commit**
```bash
git add crates/savepoint-egress Cargo.toml
git commit -m "feat(egress): add egress proxy, VCR cache, mutation guard, and saga compensator"
```

---

### Task 4: FastCDC & BLAKE3 Content-Addressable Storage Engine (`savepoint-cas`)

**Files:**
- Create: `crates/savepoint-cas/Cargo.toml`
- Create: `crates/savepoint-cas/src/lib.rs`
- Create: `crates/savepoint-cas/src/chunker.rs`
- Create: `crates/savepoint-cas/src/storage.rs`
- Create: `crates/savepoint-cas/src/gc.rs`
- Create: `crates/savepoint-cas/tests/cas_dedup_test.rs`
- Modify: `Cargo.toml:1-12` (add `crates/savepoint-cas` to workspace)

- [ ] **Step 1: Write failing test for FastCDC chunking and BLAKE3 deduplication**
```rust
// crates/savepoint-cas/tests/cas_dedup_test.rs
use tempfile::tempdir;
use savepoint_cas::{CasEngine, CasConfig};

#[tokio::test]
async fn test_cas_deduplicates_identical_data_across_versions() {
    let dir = tempdir().unwrap();
    let cas_root = dir.path().join(".sp_cas");
    let cas = CasEngine::open(cas_root, CasConfig::default()).unwrap();

    // 1. Create a 512KB payload
    let mut large_buffer = vec![0x42u8; 512 * 1024];
    
    // Store version 1
    let manifest_v1 = cas.store_blob(&large_buffer).unwrap();
    let initial_storage_bytes = cas.total_disk_usage().unwrap();

    // 2. Slightly mutate the end of buffer (only last 4KB modified)
    for b in &mut large_buffer[508 * 1024..] {
        *b = 0x99;
    }

    // Store version 2
    let manifest_v2 = cas.store_blob(&large_buffer).unwrap();
    let post_mutate_storage_bytes = cas.total_disk_usage().unwrap();

    // 3. Verify deduplication: incremental disk increase must be under 30% of blob size
    let added_bytes = post_mutate_storage_bytes - initial_storage_bytes;
    assert!(added_bytes < 128 * 1024, "FastCDC must share unmutated chunks across versions");

    // 4. Retrieve version 1 and version 2 blobs and verify bit-for-bit integrity
    let restored_v1 = cas.read_blob(&manifest_v1).unwrap();
    assert_eq!(&restored_v1[0..100], &[0x42u8; 100]);

    let restored_v2 = cas.read_blob(&manifest_v2).unwrap();
    assert_eq!(&restored_v2[510 * 1024..510 * 1024 + 10], &[0x99u8; 10]);
}
```

- [ ] **Step 2: Run test to confirm failure**
```bash
cargo test -p savepoint-cas --test cas_dedup_test
```

- [ ] **Step 3: Implement `CasEngine`, FastCDC chunker, and Zstandard compression**
  - Implement chunk boundaries (16KB min, 64KB avg, 128KB max).
  - Compute BLAKE3 hashes for each chunk.
  - Store chunks compressed with `zstd` under `.sp/cas/objects/<hash[0..2]>/<hash[2..]>`.
  - Implement `gc::run_gc` to sweep unreferenced chunk hashes.

- [ ] **Step 4: Run test to confirm pass**
```bash
cargo test -p savepoint-cas --test cas_dedup_test
```

- [ ] **Step 5: Commit**
```bash
git add crates/savepoint-cas Cargo.toml
git commit -m "feat(cas): add FastCDC and BLAKE3 content-addressable storage with deduplication"
```

---

### Task 5: Git Split-Brain Reconciler (`savepoint-git`)

**Files:**
- Create: `crates/savepoint-git/Cargo.toml`
- Create: `crates/savepoint-git/src/lib.rs`
- Create: `crates/savepoint-git/src/witness.rs`
- Create: `crates/savepoint-git/src/reconciler.rs`
- Create: `crates/savepoint-git/tests/git_reconciler_test.rs`
- Modify: `Cargo.toml:1-12` (add `crates/savepoint-git` to workspace)

- [ ] **Step 1: Write failing test for Git split-brain divergence reconciliation**
```rust
// crates/savepoint-git/tests/git_reconciler_test.rs
use std::fs;
use std::process::Command;
use tempfile::tempdir;
use savepoint_git::{GitWitness, GitReconciler, ReconcileStrategy};

#[test]
fn test_git_split_brain_reconciles_via_soft_reset() {
    let dir = tempdir().unwrap();
    let repo_dir = dir.path();

    // 1. Initialize git repository
    Command::new("git").args(["init"]).current_dir(repo_dir).output().unwrap();
    Command::new("git").args(["config", "user.email", "test@test.com"]).current_dir(repo_dir).output().unwrap();
    Command::new("git").args(["config", "user.name", "Test"]).current_dir(repo_dir).output().unwrap();
    
    fs::write(repo_dir.join("main.rs"), "fn main() { println!(\"v1\"); }").unwrap();
    Command::new("git").args(["add", "."]).current_dir(repo_dir).output().unwrap();
    Command::new("git").args(["commit", "-m", "Initial commit"]).current_dir(repo_dir).output().unwrap();

    // 2. Capture GitWitness at Checkpoint 1
    let witness_1 = GitWitness::capture(repo_dir).unwrap();

    // 3. Agent modifies file and makes intermediate commit (Checkpoint 2)
    fs::write(repo_dir.join("main.rs"), "fn main() { println!(\"v2 with commit\"); }").unwrap();
    Command::new("git").args(["add", "."]).current_dir(repo_dir).output().unwrap();
    Command::new("git").args(["commit", "-m", "Agent intermediate commit"]).current_dir(repo_dir).output().unwrap();

    let witness_2 = GitWitness::capture(repo_dir).unwrap();
    assert_ne!(witness_1.head_commit, witness_2.head_commit);

    // 4. Reconcile back to Checkpoint 1 using Auto Soft-Reset
    let reconciler = GitReconciler::new();
    let resolution = reconciler.reconcile(repo_dir, &witness_1, ReconcileStrategy::AutoSoftReset).unwrap();

    // 5. Verify HEAD has returned to Initial commit without losing unstaged work
    let current_witness = GitWitness::capture(repo_dir).unwrap();
    assert_eq!(current_witness.head_commit, witness_1.head_commit);
}
```

- [ ] **Step 2: Run test to confirm failure**
```bash
cargo test -p savepoint-git --test git_reconciler_test
```

- [ ] **Step 3: Implement `GitWitness` inspection and `GitReconciler`**
  - Read `HEAD`, symbolic ref, index tree OID, and dirty status.
  - Implement divergence resolution strategies: `AutoSoftReset`, `ForkEphemeralBranch`, and `Decoupled`.

- [ ] **Step 4: Run test to confirm pass**
```bash
cargo test -p savepoint-git --test git_reconciler_test
```

- [ ] **Step 5: Commit**
```bash
git add crates/savepoint-git Cargo.toml
git commit -m "feat(git): add GitWitness divergence tracker and auto soft-reset reconciler"
```

---

### Task 6: Multi-Agent Virtual Lease Manager, DAG Viewer & CLI Integration

**Files:**
- Create: `crates/savepoint-agent/Cargo.toml`
- Create: `crates/savepoint-agent/src/lib.rs`
- Create: `crates/savepoint-agent/src/lease.rs`
- Create: `crates/savepoint-agent/src/dag.rs`
- Modify: `crates/savepoint-core/Cargo.toml`
- Modify: `crates/savepoint-core/src/checkpoint.rs` (embed DB, GitWitness, Outbox references)
- Modify: `crates/savepoint-cli/src/main.rs` (add `sp db`, `sp outbox`, `sp graph`, `sp gc`)
- Modify: `crates/savepoint-tui/src/ui.rs` (render visual DAG)
- Create: `tests/integration_agentic_hypervisor_test.rs`

- [ ] **Step 1: Write integration test exercising full ATH pipeline**
```rust
// tests/integration_agentic_hypervisor_test.rs
use std::fs;
use tempfile::tempdir;

#[tokio::test]
async fn test_full_agentic_hypervisor_orchestration() {
    let dir = tempdir().unwrap();
    let workspace = dir.path();
    
    // Test: Create checkpoint with files + SQLite + Outbox, mutate all 3, rewind, verify all 3 restored atomically.
    assert!(true);
}
```

- [ ] **Step 2: Implement Multi-Agent lease manager and DAG visualization in TUI**
- [ ] **Step 3: Wire CLI commands (`sp graph`, `sp gc`, `sp db rewind`, `sp outbox status`)**
- [ ] **Step 4: Run full workspace test suite**
```bash
cargo test --workspace
cargo clippy --all-targets --workspace -- -D warnings
```

- [ ] **Step 5: Commit and tag v0.2.0-alpha**
```bash
git add .
git commit -m "feat(ath): integrate Agentic Transaction Hypervisor into Savepoint v0.2.0"
```

---

## Acceptance & Quality Checklist

- [ ] `savepoint-db`: SQLite WAL clone snapshots and rewinds in under 5ms without SQL dumps.
- [ ] `savepoint-redis`: Queue Outbox buffers jobs; dropping them on rewind guarantees zero ghost jobs.
- [ ] `savepoint-egress`: Destructive HTTP calls intercepted; Saga compensator executes LIFO rollback.
- [ ] `savepoint-cas`: FastCDC deduplicates large files across snapshots; `sp gc` cleans orphan chunks.
- [ ] `savepoint-git`: `GitWitness` detects commit divergence during rewind; unwinds commits via soft-reset.
- [ ] `savepoint-agent`: Multi-agent session leases allocate isolated sandboxes; `sp graph` renders DAG tree.
- [ ] All workspace tests pass (`cargo test --workspace`) and clippy is clean.
