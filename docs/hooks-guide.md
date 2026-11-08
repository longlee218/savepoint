# Savepoint Lifecycle Hooks Guide

Savepoint supports declarative lifecycle hooks to keep external state (databases, Docker containers, Redis caches, dev servers) in sync with filesystem rollbacks.

---

## Configuration File

Place `.sp-hooks.yaml`, `.sp-hooks.yml`, or `.sp-hooks.json` in your repository root (or in `.savepoint/hooks.yaml`).

```yaml
# .sp-hooks.yaml
pre_checkpoint:
  # Dump database state before taking a snapshot
  - "sqlite3 dev.db .dump > .sp-tmp/db_snapshot.sql"

post_checkpoint:
  # Optional: notify or audit
  - "echo [savepoint] Checkpoint step $SAVEPOINT_STEP captured"

pre_rollback:
  # Actions before filesystem rollback
  - "echo [savepoint] Rolling back to step $SAVEPOINT_TARGET_STEP"

post_rollback:
  # Restore database state and flush cache after rollback
  - "sqlite3 dev.db < .sp-tmp/db_snapshot.sql"
  - "redis-cli flushdb || true"
```

---

## PostgreSQL Docker Example

```yaml
pre_checkpoint:
  - "docker exec postgres_container pg_dump -U postgres myapp > .sp-tmp/pg_dump.sql"

post_rollback:
  - "docker exec -i postgres_container psql -U postgres myapp < .sp-tmp/pg_dump.sql"
```

---

## Environment Variables Injected Into Hooks

| Variable | Description |
|:---|:---|
| `SAVEPOINT_HOOK` | Name of hook being executed (`pre_checkpoint`, `post_checkpoint`, `pre_rollback`, `post_rollback`). |
| `SAVEPOINT_WORKSPACE` | Absolute path to the repository workspace root. |
| `SAVEPOINT_TX_ID` | Active transaction identifier (`tx_xxxxxxxx`). |
| `SAVEPOINT_STEP` | Current step index being checkpointed. |
| `SAVEPOINT_TARGET_STEP` | Target step index being rewound or restored to. |

---

## Safe Scratch Directory: `.sp-tmp/`

Savepoint's diff engine automatically excludes `.sp-tmp/`. Dumps and temporary artifacts stored inside `.sp-tmp/` are preserved across rollbacks so `post_rollback` can restore database dumps reliably.
