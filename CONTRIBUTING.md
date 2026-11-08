# Contributing to Savepoint

Thank you for your interest in contributing to Savepoint! This document outlines our development guidelines, workspace architecture, and testing standards.

---

## Workspace Architecture

Savepoint is structured as a modular Rust workspace consisting of 8 specialized crates:

| Crate | Responsibility |
|:---|:---|
| [`savepoint-core`](crates/savepoint-core) | Pure domain primitives (`Transaction`, `Checkpoint`, `FileDelta`, `TransactionId`, error types). |
| [`savepoint-fs`](crates/savepoint-fs) | Volume-aware storage (`st_dev`), Copy-on-Write cloning (`clonefile`, `FICLONE`), two-stage diff engine, lifecycle hooks (`.sp-hooks.yaml`), Git bridge, and portable `.spbundle` Flight Recorder. |
| [`savepoint-sandbox`](crates/savepoint-sandbox) | Isolated CoW sandboxes, Unix process group (`PGID`) isolation, child process timeout reaping, bounded pipe draining, and port/cache virtualization. |
| [`savepoint-speculate`](crates/savepoint-speculate) | Speculative multi-branch execution engine, adaptive `ResourceGovernor` (RAM/CPU concurrency limits), worker pools, and convergence policies (`FirstPassWins`, `BestPassWins`). |
| [`savepoint-mcp`](crates/savepoint-mcp) | Model Context Protocol (MCP) JSON-RPC stdio server exposing transactional tools (`begin_transaction`, `create_checkpoint`, `rollback_checkpoint`, `export_flight_bundle`). |
| [`savepoint-pty`](crates/savepoint-pty) | Non-blocking pseudoterminal supervisor, raw terminal guard, and hotkey filtering (`Ctrl+S`, `Ctrl+R`). |
| [`savepoint-tui`](crates/savepoint-tui) | Terminal User Interface powered by `ratatui` with vim navigation (`j`/`k`, `g`/`G`, `J`/`K`), stateful list scrolling, and delta inspectors. |
| [`savepoint-cli`](crates/savepoint-cli) | Unified CLI binary (`sp` and `savepoint`). |

---

## Development Workflow

### Prerequisites
- Rust stable (1.80+)
- Cargo
- Git

### Building the Project
```bash
cargo build --workspace
```

### Running Tests
All tests must pass before submitting a pull request:
```bash
cargo test --workspace
```

### Code Formatting & Linting
Ensure code complies with formatting and clippy linter rules:
```bash
# Check formatting
cargo fmt --all -- --check

# Run linter
cargo clippy --all-targets --workspace -- -D warnings
```

---

## Pull Request Guidelines

1. **Test-Driven Development (TDD):** New features or bug fixes must include unit or integration tests verifying the behavior.
2. **Zero Breaking Changes to Safe Defaults:** File restoration must be non-destructive (safety-net auto-save checkpoints are preserved).
3. **Clean Commit Messages:** Follow the [Conventional Commits](https://www.conventionalcommits.org/) specification:
   - `feat(fs): ...`
   - `fix(sandbox): ...`
   - `docs(readme): ...`
   - `refactor(tui): ...`

---

## License

By contributing to Savepoint, you agree that your contributions will be dual-licensed under both the **Apache License 2.0** and the **MIT License**.
