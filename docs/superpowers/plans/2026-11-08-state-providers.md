# Savepoint State Providers Implementation Plan (v0.2.0 Tasks 1–2, revised)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every Savepoint checkpoint also captures the state of the project's external data stores (SQLite, PostgreSQL, MySQL/MariaDB, Redis/Valkey, Memcached), and every rewind restores it, so code and data never drift apart.

**Architecture:** A new crate `savepoint-state` defines one `StateProvider` trait (`preflight → snapshot → verify → restore → discard`) with one adapter per store. A `StateManager` reads `.sp-state.yaml`, snapshots all providers into `<tx>/state/cp_N/<provider>/` atomically (all-or-nothing, with external artifacts discarded on failure), and restores them only after every provider passes `verify`. `savepoint-fs::ShadowEngine` calls the manager from `begin_transaction`, `checkpoint`, `rewind` and `commit`.

**Tech Stack:** Rust 2021 (sync, matching `savepoint-fs`), `rusqlite` (bundled, backup API), `postgres`, `mysql` (pure-Rust flate2), `redis` 1.x, a hand-written Memcached meta-protocol client, Node 24 + `bullmq` for queue scenarios, Docker Compose for the provider fleet.

**Spec:** `docs/superpowers/specs/2026-11-08-agentic-hypervisor-v02-design.md` (§2.1, §2.2). This plan supersedes Tasks 1–2 of `2026-11-08-agentic-hypervisor-v02.md`.

## Spec corrections (decided during execution)

| Spec claim | Reality | Decision |
| :--- | :--- | :--- |
| SQLite: `wal_checkpoint(PASSIVE)` + `clonefile` of `.db`/`.db-wal` | PASSIVE may leave frames un-checkpointed and a file copy can race a concurrent checkpoint (torn pages) | Use the SQLite Online Backup API with a single `step(-1)` under one read transaction; restore through the backup API, falling back to atomic file replace when the live file is corrupt or page sizes differ |
| Postgres: `CREATE DATABASE … TEMPLATE` in 10–15 ms | Cost is O(database size); fails with `55006` while *any* session is connected to the source | Terminate sessions on the target DB only (opt-out `terminate_connections: false`), retry on `55006`; snapshot DBs are `ALLOW_CONNECTIONS false` so they can never block a restore; owner, ACL, connection limit, comment and `ALTER DATABASE … SET` are re-applied because templates do not carry them |
| MySQL: template branching | MySQL has no template databases | In-server logical copy into a hidden schema under `LOCK TABLES` (cross-table consistent), explicit column lists (skips generated, keeps invisible), `NO_AUTO_VALUE_ON_ZERO`; views/routines/triggers/events stored as DDL in the manifest and replayed *after* data so triggers never fire during restore |
| Redis: key prefix + outbox via intercepted `LPUSH` | Needs app cooperation or a RESP proxy | `SCAN`+`DUMP`/`PTTL` under `CLIENT PAUSE WRITE` (consistent, expiry frozen); restore via `MULTI`/`EXEC` (atomic to other clients); `include`/`exclude` globs. RESP outbox proxy deferred to the egress milestone |
| Ghost jobs “100% dropped” | True only for jobs not yet consumed; a rewind also re-queues jobs that completed after the checkpoint | Documented semantics; `exclude: ["bull:*"]` opts queues out of rewind. Proven with real BullMQ workers, including a worker mid-job during rewind |
| Memcached | Not in spec | `lru_crawler metadump all` + `mg … v f t`; restore diffs keys (no `flush_all`, safe on shared instances) |

## Global Constraints

- `cargo test --workspace` and `cargo clippy --all-targets --workspace -- -D warnings` stay green.
- Provider tests read `SP_TEST_*` env vars (see `scripts/provider-env.sh`); when unset they print `SKIPPED <reason>` and return — never a silent pass. `scripts/test-providers.sh` runs them all against Docker.
- Tests never touch a developer's own services: the fleet binds 5xxxx ports and uses tmpfs.
- No mutation of live state before every provider's `verify` passed. Snapshot failure removes every artifact created so far.
- Identifiers are always quoted (`"…"` Postgres, `` `…` `` MySQL); snapshot names are `sp_snap_<20 hex>` (≤ 63 bytes).
- Secrets: `.sp-state.yaml` supports `${VAR}` / `${VAR:-default}`; URLs with passwords are never written to manifests.

## Review Focus

1. Application keeps connections open (dev server, ORM pool) while Savepoint snapshots/restores → must succeed with `terminate_connections: true`, fail cleanly with zero side effects with `false`.
2. Agent ran a migration after the checkpoint (new table, dropped column, new trigger) → rewind restores the exact schema, data, sequences / AUTO_INCREMENT counters.
3. Concurrent writers during snapshot → snapshot is transactionally consistent (paired parent/child rows, Redis multi-key invariants).
4. Odd identifiers (quotes, backticks, spaces, unicode, 63-byte names) and binary values (CRLF, NUL, all 256 byte values).
5. A queue worker is mid-job when a rewind happens → worker survives, queue stays healthy, ghost jobs never run.

---

### Task 1: Crate skeleton, byte-glob, checksummed record file, config
**Files:** `crates/savepoint-state/{Cargo.toml,src/lib.rs,src/error.rs,src/glob.rs,src/records.rs,src/config.rs,src/provider.rs}`, tests `tests/config_test.rs`; unit tests inline for glob/records.
- [ ] Failing tests: glob (`*`, `?`, `[a-c]`, `[^x]`, `\*`), record round-trip + checksum tamper detection + truncation detection, config (env interpolation, unknown field rejection, duplicate/invalid names, per-kind required fields).
- [ ] Implement, run, commit.

### Task 2: SQLite provider (`src/sqlite.rs`, `tests/sqlite_provider_test.rs`)
- [ ] Failing tests: round-trip incl. schema migration; un-checkpointed WAL frames captured; consistent snapshot under a concurrent writer (rows inserted in groups of 10 ⇒ count % 10 == 0); open reader connection sees restored data; absent-at-checkpoint ⇒ deleted on restore; deleted-after ⇒ recreated; corrupt live file ⇒ recovered.
- [ ] Implement, run, commit.

### Task 3: PostgreSQL provider (`src/postgres.rs`, `tests/postgres_provider_test.rs`)
- [ ] Failing tests: full-object round-trip (tables, FKs, sequences, views, functions, triggers, enums, extension); idle + in-transaction app sessions; reconnect-storm thread; `terminate_connections: false` ⇒ clean error and no snapshot DB left; owner/ACL/settings/comment preserved; weird and 63-byte names; absent-at-checkpoint; multi-checkpoint rewind and fast-forward; snapshot DB refuses connections; `discard` drops it; unreachable server.
- [ ] Implement, run against PG16 fleet + local PG18, commit.

### Task 4: MySQL / MariaDB provider (`src/mysql.rs`, `tests/mysql_provider_test.rs`)
- [ ] Failing tests (run against MySQL 8.4 and MariaDB 11.4): FKs, generated (virtual/stored), invisible, `DEFAULT CURRENT_TIMESTAMP`, JSON, ENUM/SET, BLOB, emoji, NULL, id 0, AUTO_INCREMENT counter after trailing delete; view-on-view, BEFORE INSERT trigger not double-applied, procedure, function; migration then rewind; open transaction holding locks; consistency under concurrent paired writer; weird names; absent-at-checkpoint; discard.
- [ ] Implement, run, commit.

### Task 5: Redis provider (`src/redis.rs`, `tests/redis_provider_test.rs`)
- [ ] Failing tests (Redis 7, Redis 8, Valkey 8, Redis Stack): all core types incl. streams with consumer groups; binary keys/values; TTL preserved; include/exclude scoping; other logical DBs untouched; consistent snapshot under a multi-key writer; module type (RedisJSON) on Redis Stack; cluster refusal.
- [ ] Implement, run, commit.

### Task 6: BullMQ scenarios (`tests/bullmq/{package.json,harness.mjs}`, `tests/bullmq_provider_test.rs`)
- [ ] Ghost jobs (waiting + delayed) dropped; only pre-checkpoint job runs; job-id counter rewinds without stale data; `exclude: ["bull:*"]` leaves queues untouched; worker mid-job during rewind survives and the queue keeps processing.
- [ ] Implement harness, run, commit.

### Task 7: Memcached provider (`src/memcached.rs`, `tests/memcached_provider_test.rs`)
- [ ] Failing tests: binary values with CRLF/NUL, flags, TTL, 1.5 MB value, 5k keys, add/modify/delete after checkpoint, prefix scoping leaves foreign keys untouched, url-encoded keys.
- [ ] Implement, run, commit.

### Task 8: StateManager + engine + CLI integration
**Files:** `src/manager.rs`, `tests/manager_test.rs`, `crates/savepoint-core/src/{types.rs,error.rs}`, `crates/savepoint-fs/src/engine.rs`, `crates/savepoint-fs/tests/state_integration_test.rs`, `crates/savepoint-cli/src/lib.rs`.
- [ ] Failing tests: all-or-nothing snapshot (second provider fails ⇒ first discarded, dir removed); restore refuses incomplete snapshot without touching live state; engine checkpoint/rewind restores a SQLite DB together with files; rewind always takes a safety checkpoint when providers are configured; commit discards external snapshots.
- [ ] `sp state check` (preflight every provider) and `sp state ls`.
- [ ] Implement, full workspace test + clippy, docs (`docs/state-providers.md`), commit.
