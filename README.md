# Savepoint ⚡
> **Databases got ACID transactions 40 years ago. Today, AI Agents get them too.**

Savepoint is an ultra-fast, zero-copy, transactional and time-travel runtime for autonomous AI Agents (Claude Code, Cursor, Aider, OpenCode, Codex).

```
   [Developer Prompt]
           │
           ▼
   ┌────────────────────────────────────────────────────────┐
   │             SAVEPOINT TRANSACTION RUNTIME              │
   │                                                        │
   │   ⚡ APFS Copy-on-Write (Sub-millisecond Snapshots)   │
   │   🛑 Transactional Outbox (Side-effects Interceptor)   │
   │   ⏪ Instant Time-Travel & Delta Rollback (< 2ms)      │
   └────────────────────────────────────────────────────────┘
           │
           ▼
     [AI Coding Agent in Terminal]
```

## Why Savepoint?

In the era of autonomous agents, AI writes and executes code directly in your development environment. When an agent breaks tests, deletes database migrations, or corrupts files 5 turns into a conversation, fixing it manually wastes 30–60 minutes.

**Savepoint introduces ACID transactions to your filesystem:**
- **Zero-Storage Snapshots:** Powered by native macOS APFS `clonefile(2)` and Linux OverlayFS. Creating a full snapshot takes **< 1 millisecond** with **0 bytes of additional disk space**.
- **Interactive Multi-Turn Rollback:** Chat with Claude Code or Aider normally in your terminal. If the agent goes off the rails, rewind to any previous turn in 1.8ms.
- **The F1 Brake Thesis:** High-performance F1 brakes weren't invented to slow the car down — they exist so the driver **dares to push 350 km/h**. Savepoint lets you give agents full autonomy without fear.

---

## Benchmark

| Operation | Time | Storage Overhead |
|:---|:---|:---|
| **APFS CoW Snapshot (100 files)** | **963 µs (0.96ms)** | **0 MB** |
| **Delta Compute & Diff** | **< 2 ms** | In-memory |
| **Instant Rollback / Rewind** | **1.8 ms** | Atomic |

---

## Quickstart

```bash
# 1. Start a transaction in your project
$ sp begin

# 2. Modify code or let your AI Agent run
$ sp checkpoint -m "Turn 1: Refactor auth to JWT"

# 3. View timeline and file changes
$ sp status

# 4. Rewind if something breaks
$ sp rewind --to 1

# 5. Commit when satisfied
$ sp commit
```

---

## License

Apache-2.0 or MIT.
