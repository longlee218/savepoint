# Savepoint External State Providers Guide

Savepoint v0.2.0 extends ACID transactions beyond disk files to out-of-tree databases, caches, and background job queues. Every `sp save` or automatic agent turn checkpoint captures both your code repository and your configured state providers. When you `sp undo` or `sp rewind`, files and database/cache state revert together atomically.

---

## 1. Quick Configuration: `.sp-state.yaml`

Place `.sp-state.yaml` (or `.sp-state.yml`) in your workspace root:

```yaml
providers:
  # SQLite: WAL-safe online backup (< 1.0ms)
  - name: local_sqlite
    kind: sqlite
    path: data/app.db

  # PostgreSQL: Template database branching (< 15ms)
  - name: primary_pg
    kind: postgres
    url: ${DATABASE_URL:-postgres://sp:sp@127.0.0.1:5432/app_dev}
    terminate_connections: true

  # MySQL / MariaDB: Schema clone with triggers & views restored after data
  - name: legacy_mysql
    kind: mysql
    url: mysql://root:${MYSQL_PW:-secret}@127.0.0.1:3306/shop
    lock_timeout_secs: 15

  # Redis / Valkey / Redis Stack: RDB dump + TTL restore
  - name: session_cache
    kind: redis
    url: redis://127.0.0.1:6379/0
    include: ["session:*", "cache:*"]
    exclude: ["bull:*"]  # Opt-out job queues if desired

  # Memcached: LRU crawler metadata dump with flags and relative TTLs
  - name: memcached_tokens
    kind: memcached
    url: memcached://127.0.0.1:11211
    include: ["auth:*"]
```

---

## 2. CLI Inspection Commands

```bash
# List all configured state providers
sp state ls

# Test connectivity and preflight permissions on every configured store
sp state check
```

---

## 3. Supported Providers & Guarantees

### SQLite (`kind: sqlite`)
- **Technology:** Uses SQLite Online Backup API (`sqlite3_backup_*`) with `step(-1)`.
- **Concurrency:** Safe under un-checkpointed WAL frames and concurrent writers.
- **Rewind:** Replaces live database and safely cleans up volatile `-wal` and `-shm` index files.

### PostgreSQL (`kind: postgres`)
- **Technology:** `CREATE DATABASE ... WITH TEMPLATE <src> ALLOW_CONNECTIONS false`.
- **Connection Management:** Automatically terminates active client connections if `terminate_connections: true`, or fails cleanly with zero side-effects if set to `false`.
- **Safety:** Snapshot template databases are marked `ALLOW_CONNECTIONS false` to prevent accidental writes.

### MySQL / MariaDB (`kind: mysql`)
- **Technology:** Schema-level in-server cloning with table definitions (`LIKE`), bulk data, and triggers/views/stored procedures extracted as DDL.
- **Trigger Safety:** Triggers are created *after* data is restored, guaranteeing that `BEFORE INSERT` triggers never double-fire during rollback.
- **Character Sets:** Full support for `utf8mb4` (emojis, unicode), JSON columns, and `AUTO_INCREMENT` counters.

### Redis, Valkey, and Redis Stack (`kind: redis`)
- **Data Types:** Supports strings, lists, sets, sorted sets, hashes, streams, and binary keys/values.
- **TTL:** Preserves millisecond-precision key expiration (`PTTL` / `RESTORE ... PX`).
- **Scoping:** Filter keys using standard Redis globs (`include: ["app:*"]`, `exclude: ["bull:*"]`).

### BullMQ / Celery Ghost Job Prevention
- When agent code fails and rewinds, unconsumed waiting and delayed jobs created during the speculation turn are purged.
- When background workers run, only pre-checkpoint legitimate jobs are executed.

### Memcached (`kind: memcached`)
- **Technology:** `lru_crawler metadump all` + meta get/set over text/meta protocol.
- **Shared Instance Safety:** Diffs keys instead of running `flush_all`, leaving other namespaces untouched.
