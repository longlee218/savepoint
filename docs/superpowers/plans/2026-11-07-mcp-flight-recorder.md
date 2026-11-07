# Savepoint 2026 — Sub-Project 4 Implementation Plan: Agent Protocol & Bug Flight Recorder (`savepoint-mcp` + `.spbundle`)

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the portable `.spbundle` Bug Flight Recorder (`FlightRecorder::export_bundle` & `FlightRecorder::replay_bundle`) with automatic secret scrubbing in `savepoint-fs`, expose `savepoint_export_bundle`, `savepoint_replay_bundle`, `ping`, and policy-aware `savepoint_speculate` in `savepoint-mcp`, and wire `sp share` / `sp replay` into `savepoint-cli`.

**Architecture:**
- Add `ShadowEngine::load_latest_transaction` in `crates/savepoint-fs/src/engine.rs` so `FlightRecorder` can export either an active transaction or the most recently committed transaction.
- Create `crates/savepoint-fs/src/bundle.rs` implementing `FlightRecorder`, `FlightBundle`, `BundleFileState`, `BundleExportStats`, `BundleReplayStats`, sensitive file exclusion (`.env*`, `*.pem`, `*.key`, `id_rsa*`), and inline secret token redaction (`sk-`, `ghp_`, `AKIA`, `AIza`, `xoxb-`).
- Upgrade `crates/savepoint-mcp/src/handler.rs` to support `ping`, `savepoint_export_bundle`, `savepoint_replay_bundle`, and `policy`/`max_concurrency` options on `savepoint_speculate`.
- Add `Share` and `Replay` subcommands to `crates/savepoint-cli/src/lib.rs`.

**Tech Stack:** Rust 2021, `serde`, `serde_json`, `chrono`, `walkdir`, `savepoint-core`, `savepoint-fs`, `savepoint-speculate`, `savepoint-mcp`, `savepoint-cli`.

---

## File Structure & Responsibilities

1. `crates/savepoint-fs/src/engine.rs`
   - Add `ShadowEngine::load_latest_transaction(workspace_root: &Path) -> Result<Option<Transaction>>`.
2. `crates/savepoint-fs/src/bundle.rs`
   - Implement `FlightRecorder`, `FlightBundle`, `BundleFileState`, `BundleExportStats`, `BundleReplayStats`, secret path filtering, and inline token scrubbing.
3. `crates/savepoint-fs/src/lib.rs`
   - Export `pub mod bundle;` and re-export `FlightRecorder`, `FlightBundle`, `BundleExportStats`, `BundleReplayStats`.
4. `crates/savepoint-fs/tests/bundle_test.rs`
   - Integration test verifying `.spbundle` export, `.env` exclusion, inline `sk-...` token scrubbing, and multi-step time-travel replay into a clean directory.
5. `crates/savepoint-mcp/src/handler.rs`
   - Add `ping`, `savepoint_export_bundle`, `savepoint_replay_bundle`, and policy/concurrency options on `savepoint_speculate`.
6. `crates/savepoint-mcp/tests/mcp_test.rs`
   - Integration test exercising `initialize`, `ping`, `tools/list`, `savepoint_begin`, `savepoint_checkpoint`, `savepoint_speculate`, `savepoint_export_bundle`, and `savepoint_replay_bundle`.
7. `crates/savepoint-cli/src/lib.rs`
   - Add `Commands::Share` and `Commands::Replay`.

---

### Task 1: Portable `.spbundle` Flight Recorder with Secret Scrubbing & Hardened MCP/CLI (`savepoint-fs`, `savepoint-mcp`, `savepoint-cli`)

**Files:**
- Modify: `crates/savepoint-fs/src/engine.rs`
- Create: `crates/savepoint-fs/src/bundle.rs`
- Modify: `crates/savepoint-fs/src/lib.rs`
- Create: `crates/savepoint-fs/tests/bundle_test.rs`
- Modify: `crates/savepoint-mcp/src/handler.rs`
- Create: `crates/savepoint-mcp/tests/mcp_test.rs`
- Modify: `crates/savepoint-cli/src/lib.rs`

- [ ] **Step 1: Write failing integration tests in `crates/savepoint-fs/tests/bundle_test.rs` and `crates/savepoint-mcp/tests/mcp_test.rs`**

`crates/savepoint-fs/tests/bundle_test.rs`:
```rust
use std::fs;
use savepoint_fs::{FlightRecorder, ShadowEngine};

#[test]
fn test_flight_recorder_export_scrubs_secrets_and_replays_step_accurately() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path().join("ws");
    let repro = temp.path().join("repro");
    let bundle_path = temp.path().join("bug_report.spbundle");

    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src/app.rs"), "pub fn run() -> &'static str { \"v1\" }").unwrap();
    fs::write(ws.join(".env"), "OPENAI_API_KEY=sk-1234567890abcdefghijklmnop\n").unwrap();

    let mut tx = ShadowEngine::begin_transaction(&ws).unwrap();

    // Step 1: valid feature + config file containing an accidental inline API token
    fs::write(ws.join("src/app.rs"), "pub fn run() -> &'static str { \"v2_good\" }").unwrap();
    fs::write(
        ws.join("src/config.toml"),
        "endpoint = \"https://api.example.com\"\ntoken = \"sk-proj9876543210abcdefghijklmnop\"\n",
    )
    .unwrap();
    ShadowEngine::checkpoint(&mut tx, Some("Turn 1 with secret sk-inline1234567890abcdefgh"), None)
        .unwrap();

    // Step 2: buggy code introduced by agent
    fs::write(ws.join("src/app.rs"), "pub fn run() -> &'static str { \"v3_buggy\" }").unwrap();
    ShadowEngine::checkpoint(&mut tx, Some("Turn 2 broke app.rs"), None).unwrap();

    // Commit transaction to also prove load_latest_transaction works on committed transactions
    ShadowEngine::commit(&mut tx).unwrap();

    let export_stats = FlightRecorder::export_bundle(&ws, &bundle_path).unwrap();
    assert!(bundle_path.exists());
    assert!(
        export_stats.scrubbed_files_count >= 1,
        "Expected .env to be scrubbed, got {}",
        export_stats.scrubbed_files_count
    );
    assert!(
        export_stats.scrubbed_secrets_count >= 2,
        "Expected inline sk- tokens in config.toml and prompt to be scrubbed, got {}",
        export_stats.scrubbed_secrets_count
    );

    let raw_bundle_json = fs::read_to_string(&bundle_path).unwrap();
    assert!(!raw_bundle_json.contains("sk-1234567890abcdefghijklmnop"));
    assert!(!raw_bundle_json.contains("sk-proj9876543210abcdefghijklmnop"));
    assert!(!raw_bundle_json.contains("sk-inline1234567890abcdefgh"));
    assert!(raw_bundle_json.contains("[REDACTED_SECRET]"));

    // Replay Step 1 into repro directory
    let replay1 = FlightRecorder::replay_bundle(&bundle_path, &repro, Some(1)).unwrap();
    assert_eq!(replay1.replayed_step, 1);
    assert_eq!(
        fs::read_to_string(repro.join("src/app.rs")).unwrap(),
        "pub fn run() -> &'static str { \"v2_good\" }"
    );
    let restored_cfg = fs::read_to_string(repro.join("src/config.toml")).unwrap();
    assert!(restored_cfg.contains("[REDACTED_SECRET]"));
    assert!(!repro.join(".env").exists(), ".env must never be exported or replayed");

    // Replay Step 2 into repro directory
    let replay2 = FlightRecorder::replay_bundle(&bundle_path, &repro, Some(2)).unwrap();
    assert_eq!(replay2.replayed_step, 2);
    assert_eq!(
        fs::read_to_string(repro.join("src/app.rs")).unwrap(),
        "pub fn run() -> &'static str { \"v3_buggy\" }"
    );
}
```

`crates/savepoint-mcp/tests/mcp_test.rs`:
```rust
use std::fs;
use serde_json::json;
use savepoint_mcp::protocol::JsonRpcRequest;
use savepoint_mcp::McpHandler;

#[test]
fn test_mcp_tools_lifecycle_and_bundle_export_replay() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path().join("mcp_ws");
    let repro = temp.path().join("mcp_repro");
    let bundle_file = temp.path().join("mcp.spbundle");
    fs::create_dir_all(&ws).unwrap();
    fs::write(ws.join("code.txt"), "v1").unwrap();

    // 1. ping
    let ping_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(1)),
        method: "ping".to_string(),
        params: None,
    };
    let ping_resp = McpHandler::handle_request(ping_req).unwrap();
    assert!(ping_resp.error.is_none());

    // 2. savepoint_begin
    let begin_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(2)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "savepoint_begin",
            "arguments": { "path": ws.to_string_lossy() }
        })),
    };
    let begin_resp = McpHandler::handle_request(begin_req).unwrap();
    assert!(begin_resp.error.is_none());

    // 3. savepoint_speculate with policy = first_pass_wins
    let spec_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(3)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "savepoint_speculate",
            "arguments": {
                "path": ws.to_string_lossy(),
                "policy": "first_pass_wins",
                "max_concurrency": 2,
                "validation_cmd": "grep -q 'v2_winner' code.txt",
                "candidates": [
                    { "name": "cand_win", "setup_commands": ["echo 'v2_winner' > code.txt"] },
                    { "name": "cand_lose", "setup_commands": ["echo 'v2_loser' > code.txt"] }
                ]
            }
        })),
    };
    let spec_resp = McpHandler::handle_request(spec_req).unwrap();
    assert!(spec_resp.error.is_none());
    assert_eq!(fs::read_to_string(ws.join("code.txt")).unwrap().trim(), "v2_winner");

    // 4. savepoint_export_bundle
    let export_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(4)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "savepoint_export_bundle",
            "arguments": {
                "path": ws.to_string_lossy(),
                "output": bundle_file.to_string_lossy()
            }
        })),
    };
    let export_resp = McpHandler::handle_request(export_req).unwrap();
    assert!(export_resp.error.is_none());
    assert!(bundle_file.exists());

    // 5. savepoint_replay_bundle (replay step 0 -> baseline "v1")
    fs::write(repro.join("dummy"), "").ok();
    let replay_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(5)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "savepoint_replay_bundle",
            "arguments": {
                "bundle": bundle_file.to_string_lossy(),
                "target_path": repro.to_string_lossy(),
                "step": 0
            }
        })),
    };
    let replay_resp = McpHandler::handle_request(replay_req).unwrap();
    assert!(replay_resp.error.is_none());
    assert_eq!(fs::read_to_string(repro.join("code.txt")).unwrap().trim(), "v1");
}
```

- [ ] **Step 2: Run tests to verify they fail (RED)**

Run: `cargo test --package savepoint-fs --test bundle_test && cargo test --package savepoint-mcp --test mcp_test`
Expected: Compilation failure (`unresolved import savepoint_fs::FlightRecorder`).

- [ ] **Step 3: Implement `load_latest_transaction`, `bundle.rs`, MCP tools, and CLI subcommands**

1. Add `ShadowEngine::load_latest_transaction` in `crates/savepoint-fs/src/engine.rs`.
2. Create `crates/savepoint-fs/src/bundle.rs` and export in `crates/savepoint-fs/src/lib.rs`.
3. Update `crates/savepoint-mcp/src/handler.rs` with `ping`, `savepoint_export_bundle`, `savepoint_replay_bundle`, and `policy`/`max_concurrency` in `savepoint_speculate`.
4. Add `Commands::Share` and `Commands::Replay` to `crates/savepoint-cli/src/lib.rs`.

- [ ] **Step 4: Run tests across workspace to verify they pass (GREEN)**

Run: `cargo test --workspace`
Expected: PASS (all unit and integration tests across the entire workspace pass).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/savepoint-fs/src/engine.rs crates/savepoint-fs/src/bundle.rs crates/savepoint-fs/src/lib.rs crates/savepoint-fs/tests/bundle_test.rs crates/savepoint-mcp crates/savepoint-cli
git commit -m "feat(mcp,bundle): add .spbundle flight recorder with secret scrubbing, MCP bundle/policy tools, and CLI share/replay"
```

---

## Review Focus

- **Secret Scrubbing Completeness & Safety:** Verify `.env`, `.env.*` (excluding `.env.example`/`.env.template`), `*.pem`, `*.key`, `id_rsa*`, `id_ed25519*` are never included in `.spbundle`, and inline tokens (`sk-`, `ghp_`, `AKIA`, `AIza`, `xoxb-`) are redacted from both UTF-8 file contents and checkpoint prompts/summaries.
- **Path Traversal Prevention on Replay:** Verify `FlightRecorder::replay_bundle` rejects any relative path in `step_files` that is absolute, starts with `/`, or contains `..` (`Component::ParentDir` / `Component::RootDir`) before writing or deleting files in `target_workspace`.
- **Cross-Crate Integration (`savepoint-fs`, `savepoint-mcp`, `savepoint-cli`):** Verify `cargo test --workspace` compiles and passes cleanly across all 8 workspace crates.
