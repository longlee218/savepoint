# Savepoint v0.2.0: Agentic Transaction Hypervisor (ATH) System Architecture & Design Specification

> **Document Status:** Approved Design Spec  
> **Target Release:** Savepoint v0.2.0  
> **Primary Authors:** DeepMind Agentic Systems Architecture Team & Antigravity  
> **Date:** 2026-11-08  

---

## 1. Executive Summary & Problem Formulation

In modern AI agent workflows (Claude Code, Codex, SWE-agent, Devins), autonomous coding agents do far more than modify code files on disk:
1. **Direct Database Manipulations:** Running database migrations (`prisma migrate`, `alembic upgrade`, `rails db:migrate`), executing seed scripts, and writing row-level fixtures into SQLite, PostgreSQL, and MySQL.
2. **In-Memory Caches & Job Queues:** Enqueuing tasks in Redis / Memcached / RabbitMQ / BullMQ / Celery. When an agent experiments and fails, rolling back code files leaves "Ghost Jobs" in the queue, firing background workers against rolled-back code and corrupting external state.
3. **External API Calls & Third-Party Egress:** Triggering payment sandbox calls (Stripe charges, subscriptions), cloud provisioning (AWS S3 bucket creation), transactional communications (SendGrid / Twilio), and webhooks. When an agent rewinds its local disk, external APIs remain mutated and irreconcilable.
4. **Git Split-Brain Divergence:** Agents running `git commit` midway through speculation. If `savepoint rewind` only reverts disk files, the Git `HEAD` commit becomes detached or points to committed files that were reverted on disk, confusing developers and tools.
5. **Storage Explosion:** File-level copy-on-write without deduplication explodes disk usage when multi-agent swarms build heavy artifacts (`target/`, `node_modules/`, Python virtual environments).

**Savepoint v0.2.0: Agentic Transaction Hypervisor (ATH)** expands Savepoint from a file-level speculation tool into a full-system hypervisor that captures, isolates, and atomically rolls back the agent's complete operational universe: **Filesystem, Databases, Caches/Queues, External API Egress, and Git State.**

```
                                  +---------------------------------------+
                                  |         Autonomous AI Agent           |
                                  |   (Claude Code, Codex, Cursor, PTY)   |
                                  +-------------------+-------------------+
                                                      |
                                                      v
                   +---------------------------------------------------------------------+
                   |             Savepoint ATH (Agentic Transaction Hypervisor)           |
                   |                                                                     |
                   |  +---------------------+  +--------------------+  +--------------+  |
                   |  |   savepoint-fs      |  |    savepoint-db    |  | savepoint-cas|  |
                   |  | Kernel CoW Overlay  |  | SQLite WAL / PgSQL |  | FastCDC +    |  |
                   |  |                     |  | Branching Hyperv.  |  | BLAKE3 CAS   |  |
                   |  +---------------------+  +--------------------+  +--------------+  |
                   |  +---------------------+  +--------------------+  +--------------+  |
                   |  |  savepoint-egress   |  |   savepoint-redis  |  |savepoint-git |  |
                   |  | HTTP Proxy, Saga,   |  | Keyspace Sandbox & |  | GitWitness   |  |
                   |  | VCR, Outbox Tank    |  | Queue Outbox Guard |  | Reconciler   |  |
                   |  +---------------------+  +--------------------+  +--------------+  |
                   +----------------------------------+----------------------------------+
                                                      |
             +--------------------+-------------------+--------------------+--------------------+
             |                    |                   |                    |                    |
             v                    v                   v                    v                    v
     +---------------+    +---------------+   +---------------+    +---------------+   +-----------------+
     | Filesystem &  |    | Relational DB |   | Redis/Queues  |    | External APIs |   | Git Repository  |
     | Build Target  |    | (SQLite / PG) |   | (Celery, Bull)|    | (Stripe, AWS) |   | Working Tree    |
     +---------------+    +---------------+   +---------------+    +---------------+   +-----------------+
```

---

## 2. Core Subsystem Architectures

### 2.1. Deep State Database Virtualization (`savepoint-db`)

#### 2.1.1. SQLite WAL CoW Hypervisor (< 1ms snapshot / rewind)
SQLite databases in development workspaces (`db.sqlite3`, `app.db`) run in WAL (Write-Ahead Logging) mode.
- **The Problem:** Running SQL text dumps (`.dump`) takes hundreds of milliseconds to seconds and locks write locks. Naive file copying of `.sqlite3` while transactions occur corrupts the snapshot if WAL is active.
- **The ATH Solution:**
  1. ATH checks whether the target database file is in WAL mode by reading byte offsets 18 and 19 (values 2 = WAL).
  2. ATH executes `PRAGMA wal_checkpoint(PASSIVE);` via an embedded low-overhead connection. Passive checkpointing flushes dirty pages without acquiring an exclusive writer lock or blocking concurrent readers.
  3. ATH uses APFS `clonefile` / Btrfs CoW ioctl to instantly clone `db.sqlite3` and `db.sqlite3-wal` into the checkpoint cache:
     ```
     .sp/db_snapshots/<tx_id>/db.sqlite3
     .sp/db_snapshots/<tx_id>/db.sqlite3-wal
     ```
     *(Notice: `db.sqlite3-shm` shared memory index is intentionally excluded and regenerated on connect).*
  4. **Rewind Protocol:** ATH closes any open read handles and invokes `clonefile` in reverse: swapping the checkpoint WAL and database file atomically back into place. Benchmark: **< 0.8ms**.

#### 2.1.2. PostgreSQL / MySQL Template Database Branching
For enterprise projects running local Postgres or Docker Compose databases:
- **Snapshot Protocol:**
  - Before an agent begins speculative execution on a test suite or migration, ATH creates an ephemeral isolated template clone:
    ```sql
    CREATE DATABASE app_sp_tx_12345 TEMPLATE app_dev;
    ```
  - This utilizes PostgreSQL's underlying block copy engine (bypassing SQL dump/restore) and completes in **10–15ms**.
  - ATH transparently rewrites the database connection URL inside the agent's sandbox environment variables (`DATABASE_URL=postgres://localhost:5432/app_sp_tx_12345`).
- **Rewind Protocol:**
  - If the agent fails or triggers `sp rewind`, ATH drops the ephemeral database:
    ```sql
    DROP DATABASE IF EXISTS app_sp_tx_12345 WITH (FORCE);
    ```
  - Zero side effects pollute `app_dev`.

---

### 2.2. In-Memory Store & Queue Virtualization (`savepoint-redis`)

#### 2.2.1. Redis Logical Database & Keyspace Sandboxing
When agents test APIs that write session caches, rate-limits, or counters to Redis:
- ATH injects a transaction-scoped namespace or switches logical DB index:
  ```rust
  pub struct RedisSandboxConfig {
      pub prefix: String,        // e.g., "sp_tx_a8f1:"
      pub isolated_db_index: u32,// e.g., DB 15
  }
  ```
- **Rewind Protocol:**
  - On rollback, ATH triggers an asynchronous `UNLINK` on all keys matching the prefix via scan or drops the isolated database index (`SELECT 15; FLUSHDB ASYNC;`). Rollback overhead: **< 3ms**.

#### 2.2.2. Ghost Job Prevention via Transactional Outbox
Background job queues (BullMQ, Celery, Sidekiq) running against Redis often consume jobs immediately. If an agent enqueues an asynchronous email send or web crawl, then fails and rewinds, the worker picks up the job and fails or acts destructively.
- ATH intercepts outbound job push commands (`LPUSH`, `RPUSH`, `XADD`, `ZADD`) using an egress shim:
  - ATH diverts incoming queue payloads into an **Outbox Holding Tank**.
  - **On Commit (`sp accept` / `sp to-git`):** The Outbox Holding Tank flushes all buffered queue messages to the active Redis cluster.
  - **On Rollback (`sp rewind`):** The Outbox Holding Tank simply drops all uncommitted messages. **Zero ghost jobs escape.**

---

### 2.3. External API Egress Hypervisor & Saga Compensator (`savepoint-egress`)

```
                  +----------------------------------------------+
                  |         Agent Process (curl / SDK)           |
                  +----------------------+-----------------------+
                                         |
                                         v (HTTP_PROXY / HTTPS_PROXY)
                  +----------------------------------------------+
                  |    savepoint-egress Transparent Mitm Proxy   |
                  +----------------------+-----------------------+
                                         |
               +-------------------------+-------------------------+
               |                         |                         |
               v                         v                         v
     [Safe GET / Reads]         [Idempotent / Cached]      [Destructive POST/DELETE]
     Direct or VCR Cache        Replay recorded VCR        Mutation Guard Intercept:
     e.g., Stripe List Cust.    e.g., GET /v1/charges      1. Mock Sandbox Response
                                                           2. Require Human Auth
                                                           3. Register Saga Compensator
```

#### 2.3.1. Transparent Mitm Egress Proxy
When `sp sandbox` or `sp speculate` runs, ATH injects environment variables:
`HTTP_PROXY=http://127.0.0.1:18443` and `HTTPS_PROXY=http://127.0.0.1:18443`.
The proxy operates in three modes:
1. **Record & Replay (VCR Engine):** Read-only requests (`GET`, `HEAD`, `OPTIONS`) are cached in `.sp/vcr_cassettes/`. Deterministic replays allow instant rerun of agent tests with zero latency and zero network consumption.
2. **Mutation Guard:** When destructive verbs (`POST`, `PUT`, `DELETE`, `PATCH`) target external hosts (e.g., `api.stripe.com`, `api.github.com`, `s3.amazonaws.com`):
   - **Mode Safe (Default):** Returns synthetic realistic HTTP 200/201 responses derived from OpenAPI specs or cached templates.
   - **Mode Prompt:** Pauses the agent PTY and asks the developer:
     ```
     [Savepoint Egress Guard] Agent is attempting POST to https://api.stripe.com/v1/customers
     Payload: {"email": "test@domain.com"}
     Allow external network mutation? [y/N/mock]:
     ```

#### 2.3.2. Saga Compensator Engine (Automated Inverse Actions)
If the developer permits real external network mutations, ATH records an undo action in its **Saga Log**:
```rust
pub struct SagaEntry {
    pub forward_req: HttpRequestSnapshot,
    pub response: HttpResponseSnapshot,
    pub inverse_action: Option<CompensatingAction>,
}

pub enum CompensatingAction {
    HttpUndo {
        method: HttpMethod,
        url: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    },
    CustomScript(String),
}
```
*Example:* When the agent executes `POST https://api.stripe.com/v1/charges` returning `id: "ch_3MtwL"`, the registered compensator is `POST https://api.stripe.com/v1/refunds` with `{"charge": "ch_3MtwL"}`.
- **Rewind Protocol:** When `sp rewind` triggers, ATH executes all registered compensating actions in reverse order (LIFO), returning the external service to its pre-speculation state.

---

### 2.4. Storage Deduplication & Content-Addressable Storage (`savepoint-cas`)

To prevent multi-agent swarms and iterative checkpoints from consuming gigabytes of disk space:

```
[File Stream] ---> [FastCDC Chunker (Chunk Size 16KB - 64KB - 128KB)]
                         |
                         v
              [BLAKE3 256-bit Cryptographic Hash]
                         |
                         v
              +---------------------+
              | Is Chunk in CAS DB? |
              +----------+----------+
                    /          \
                (Yes)          (No)
                 /                \
        [Inc Ref Count]       [Zstandard Compression]
        [Reuse chunk_id]               |
                                       v
                              [Write to .sp/cas/objects/ab/cd...]
```

#### 2.4.1. FastCDC & BLAKE3 Content-Addressable Storage
- Rather than full-file snapshots, `savepoint-cas` chops files larger than 128KB into content-defined chunks using **FastCDC** (Fast Content-Defined Chunking algorithm).
- Each chunk is addressed by its **BLAKE3 hash** (over 3x faster than SHA-256 and AVX-512 accelerated).
- Identical chunks across checkpoints (or identical dependencies across multiple agent branches) are stored **exactly once**.
- Chunks are compressed using **Zstandard (Level 3)**, yielding typical 70–85% storage reduction on build artifacts and node modules.

#### 2.4.2. Tiered Lifecycle & Garbage Collection (`sp gc`)
ATH manages storage across 3 performance tiers:
- **Tier 1 (Hot):** Active working tree & scratch sandbox (APFS / Btrfs CoW kernel references, 0ms latency).
- **Tier 2 (Warm):** Recent checkpoints (last 50 checkpoints stored as CAS chunk manifests).
- **Tier 3 (Cold):** Archived milestones bundled into `.spbundle` zstd tar archives.
- **`sp gc` (Garbage Collection):** Scans active checkpoint roots, performs mark-and-sweep on unreferenced BLAKE3 objects in `.sp/cas/`, and prunes orphan chunks.

---

### 2.5. Git Split-Brain Reconciler (`savepoint-git`)

When an agent runs `git commit -m "temp fix"` inside an active speculation session:
1. ATH's filesystem layer captures the modified `.git/` refs and objects.
2. In prior versions, rolling back files without rolling back git state causes **Git Split-Brain**:
   - Disk files revert to Step 1, but `HEAD` points to Step 4.
   - Running `git status` shows all Step 1 files as deleted or unstaged!

#### 2.5.1. The `GitWitness` Protocol
Every checkpoint captures a `GitWitness`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitWitness {
    pub head_commit: Option<String>,
    pub current_branch: Option<String>,
    pub is_detached: bool,
    pub index_tree_oid: Option<String>,
    pub dirty_files_count: usize,
}
```

#### 2.5.2. Divergence Detection Matrix on `sp rewind`
When the developer or agent calls `sp rewind <checkpoint_id>`:
1. ATH compares the target checkpoint's `GitWitness` ($W_{target}$) with the active workspace's current Git state ($W_{current}$).
2. **Case A ($W_{current}.head\_commit == W_{target}.head\_commit$):** Git history did not move. Apply standard CoW file rollback.
3. **Case B ($W_{current}.head\_commit \neq W_{target}.head\_commit$):** The agent made commits between the target checkpoint and now.
   - **Resolution 1 (Auto Soft-Reset - Default):** ATH performs `git reset --soft <target_head>`. All intermediate commits are safely unwrapped back into the working tree index, preventing commit loss while matching the target checkpoint.
   - **Resolution 2 (Fork Ephemeral Branch):** ATH creates `refs/heads/savepoint-orphaned-<tx-id>` pointing to the abandoned commit before resetting `HEAD`. The developer can recover their discarded work at any time via `git cherry-pick` or `git merge`.
   - **Resolution 3 (Decoupled Mode):** If the developer opted to ignore Git, ATH warns clearly and leaves `.git` untouched.

---

### 2.6. Multi-Agent Workspace Coordination & Visual DAG (`savepoint-agent`)

When multiple AI subagents (e.g., Architect, Backend Coder, Frontend Specialist, Security Reviewer) operate in parallel:
- **Virtual Session Leases:** ATH allocates isolated runtime environments per agent ID:
  - Unique sandbox directory: `.sp/workspaces/agent_<uuid>/`
  - Dynamic port offsets: Agent 1 (Port 3000 -> 3100), Agent 2 (Port 3000 -> 3200)
  - Isolated database clone: `app_agent_<uuid>`
  - Isolated Redis DB: DB index $(10 + i)$
- **Visual DAG Explorer:**
  ASCII and Unicode DAG graph representation rendered directly into TUI and CLI (`sp graph`):
  ```
  [*] Root (main @ 7a3d9)
   |
   +--- (Agent-Alpha) Checkpoint #1: "Added SQLite auth schema"
   |     |
   |     \--- Checkpoint #2: "Implemented JWT token verify" [HEAD-Alpha]
   |
   \--- (Agent-Beta)  Checkpoint #3: "Tailwind UI Navbar layout"
         |
         \--- Checkpoint #4: "Dark mode toggle component" [HEAD-Beta]
  ```
- **AST-Aware 3-Way Semantic Merge:**
  When merging Agent Alpha's changes and Agent Beta's changes into main:
  - Standard git line-based diff fails if both agents modified imports or route definitions.
  - ATH runs Tree-Sitter based AST merging: functions with non-overlapping AST identifiers are automatically stitched together with zero conflict markers.

---

## 3. Data Structures & File Layout

```
.sp/
├── config.yaml                     # Savepoint repository settings & hypervisor flags
├── cas/                            # Content-Addressable Storage (CAS)
│   ├── objects/                    # BLAKE3 hashed chunk storage
│   │   ├── 4a/
│   │   │   └── 3f9b201... (zstd)
│   │   └── ...
│   └── index.db                    # SQLite index of chunk references & refcounts
├── db_snapshots/                   # Deep database snapshots
│   ├── sp_tx_01/
│   │   ├── manifest.json           # Database type, connection string, file sizes
│   │   ├── sqlite.db               # CoW cloned SQLite file
│   │   └── sqlite.db-wal           # CoW cloned WAL file
├── egress/                         # Egress hypervisor storage
│   ├── vcr_cassettes/              # Recorded HTTP transactions
│   │   └── cassette_stripe_read.json
│   ├── outbox.db                   # Buffered queue messages pending commit
│   └── saga_journal.json           # LIFO compensation log
├── checkpoints/                    # Checkpoint metadata manifests
│   ├── ckpt_001.json               # FS deltas + GitWitness + DB snapshot ref
│   └── ...
└── workspaces/                     # Multi-agent virtual leases
    ├── agent_alpha/
    └── agent_beta/
```

---

## 4. Performance & Reliability Guarantees

| Operation | Baseline Target | Savepoint ATH Guarantee | Underlying Mechanism |
| :--- | :--- | :--- | :--- |
| **SQLite Snapshot** | 200ms - 2s (text dump) | **< 1.0ms** | `PRAGMA wal_checkpoint(PASSIVE)` + APFS CoW clone |
| **SQLite Rewind** | 500ms - 3s (restore dump) | **< 0.8ms** | Kernel atomic file swap |
| **Postgres Snapshot** | 5s - 30s (`pg_dump`) | **< 20ms** | `CREATE DATABASE ... TEMPLATE` |
| **Postgres Rewind** | 10s - 45s (`pg_restore`) | **< 15ms** | `DROP DATABASE ... WITH (FORCE)` |
| **Redis Keyspace Rollback** | N/A (Keys left dirty) | **< 3ms** | Key prefix `UNLINK` / `FLUSHDB ASYNC` |
| **Ghost Job Prevention** | Fail (Jobs execute) | **100% Dropped** | Transactional Outbox holding tank |
| **Storage Savings** | 1x - 5x explosion | **80% - 92% Deduplication** | FastCDC (64KB avg) + BLAKE3 + Zstd |
| **Git Split-Brain** | Desync / File corruption | **Zero Desync** | `GitWitness` + Auto Soft-Reset / Ephemeral Branch |

---

## 5. Security & Isolation Boundaries

1. **Egress Interception:** The transparent proxy runs on loopback `127.0.0.1` and binds an unprivileged local port. All TLS connections are intercepted strictly in development mode using a locally generated, self-signed CA stored in `.sp/certs/`.
2. **Credential Safety:** The Egress Proxy masks authorization headers (`Bearer **********`, `Basic **********`) in saved VCR cassettes to prevent leaking secret keys into repo state.
3. **Database Access:** Template database creation requires local Postgres credentials specified in the developer's `.env.test` or `.env.local`; ATH never connects to external production database hosts.

---

## 6. Verification & Test Strategy

1. **Unit Test Suite:**
   - SQLite WAL state transition: testing dirty write, passive checkpoint, CoW clone, external write, atomic swap rollback.
   - Redis prefix key extraction and async unlink benchmarking.
   - FastCDC chunk boundary stability and BLAKE3 collision immunity.
   - `GitWitness` commit delta calculation and auto soft-reset simulation.
2. **Integration Test Scenarios:**
   - Agent runs Prisma migration on SQLite, enqueues 5 BullMQ tasks in Redis, issues Stripe customer creation via HTTP, and makes a git commit.
   - `sp rewind` called:
     - Prisma migration undone (database reverted).
     - Redis keys deleted.
     - Outbox queue dropped (0 jobs reach workers).
     - Stripe saga compensator fires refund/cleanup.
     - Git commit soft-reset cleanly back into staging.
3. **Stress & Fuzz Testing:**
   - 1,000 rapid checkpoints created across a 500MB synthetic repository containing binary blobs and source files to verify FastCDC memory boundaries and zero corrupted CAS chunks.
