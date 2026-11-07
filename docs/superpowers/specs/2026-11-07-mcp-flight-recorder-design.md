# Savepoint 2026 — Sub-Project 4 Design Spec: Agent Protocol & Bug Flight Recorder (`savepoint-mcp` + `.spbundle`)

**Date:** 2026-11-07
**Status:** Approved
**Target Crates:** `savepoint-fs` (`bundle` module + `engine.rs`), `savepoint-mcp`, `savepoint-cli`

---

## 1. Context & Problem Statement

Sub-Project 4 completes the **Savepoint 2026: Agentic Transaction Hypervisor (ATH)** architecture by delivering the **Bug Flight Recorder (`.spbundle`)** and hardening the **Model Context Protocol (MCP) Server (`savepoint-mcp`)** and **CLI (`savepoint-cli`)**.

Our adversarial review identified three critical requirements for this final pillar:
1. **Delta-Only Portable `.spbundle` (`sp share` / `sp replay`):** Exporting an entire multi-gigabyte workspace for bug reproduction is impractical. Instead, `.spbundle` must capture the `Transaction` timeline metadata (`Checkpoint`s, `deltas`, `content_hash`) alongside only the files touched across the transaction's checkpoints (plus their `cp_0` baseline versions), keeping bundles compact (kilobytes) while allowing full time-travel replay at any step (`sp replay <bundle.spbundle> --step <N>`).
2. **Automatic Secret Scrubbing:** Exporting a bundle to attach to a GitHub Issue or share with another developer/agent must never leak sensitive files (`.env`, `.env.*`, `*.pem`, `*.key`, `id_rsa*`) or plaintext API tokens (`sk-...`, `ghp_...`, `AKIA...`, `AIza...`, `xoxb-...`) in prompts or text payloads.
3. **Hardened MCP Protocol & Tool Coverage (`savepoint-mcp`):**
   - `savepoint_speculate` in `savepoint-mcp` must expose `policy` (`"best_pass_wins"` | `"first_pass_wins"`) and `max_concurrency` from Sub-Project 3 (`SpeculationConfig`).
   - Expose `savepoint_export_bundle` and `savepoint_replay_bundle` tools to MCP clients.
   - Properly handle JSON-RPC `ping` requests and notifications (`notifications/initialized`).

---

## 2. Goals & Non-Goals

### Goals
1. **Portable `.spbundle` Flight Recorder (`crates/savepoint-fs/src/bundle.rs`):**
   - `FlightRecorder::export_bundle(workspace_root: &Path, output_path: &Path) -> Result<BundleExportStats>`
   - `FlightRecorder::replay_bundle(bundle_path: &Path, target_workspace: &Path, target_step: Option<usize>) -> Result<BundleReplayStats>`
   - Store only touched relative paths across all checkpoints (and their `cp_0` baseline state) so `.spbundle` files remain lightweight and self-contained without external C compression dependencies.
2. **Built-in Secret Scrubber (`bundle.rs`):**
   - Exclude sensitive filenames/extensions: `.env`, `.env.*` (except `.env.example` / `.env.template`), `*.pem`, `*.key`, `id_rsa`, `id_ed25519`, `credentials.json`.
   - Scrub common secret token patterns (`sk-[A-Za-z0-9_-]{16,}`, `ghp_[A-Za-z0-9]{20,}`, `AKIA[0-9A-Z]{16}`, `AIza[0-9A-Za-z_-]{20,}`, `xoxb-[0-9A-Za-z-]+`) in checkpoint prompts, summaries, and UTF-8 file contents, replacing matches with `[REDACTED_SECRET]` and incrementing `scrubbed_secrets_count`.
3. **Hardened MCP Server (`crates/savepoint-mcp/src/handler.rs`):**
   - Handle `"ping"` method returning `JsonRpcResponse::success(id, json!({}))`.
   - Upgrade `"savepoint_speculate"` tool schema and handler to accept optional `"policy"` (`"best_pass_wins"` or `"first_pass_wins"`) and `"max_concurrency"`.
   - Add `"savepoint_export_bundle"` and `"savepoint_replay_bundle"` tools.
4. **CLI `sp share` & `sp replay` Subcommands (`crates/savepoint-cli/src/lib.rs`):**
   - `Commands::Share { output: PathBuf, path: PathBuf }` (`sp share -o issue.spbundle`)
   - `Commands::Replay { bundle: PathBuf, step: Option<usize>, path: PathBuf }` (`sp replay issue.spbundle --step 1 -p ./repro`)

### Non-Goals
- Cloud-hosted bundle storage service (bundles are portable local `.spbundle` files ready for Git/GitHub/Slack sharing).

---

## 3. Architecture & Component Design

### 3.1 `.spbundle` Schema (`crates/savepoint-fs/src/bundle.rs`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlightBundle {
    pub format_version: String, // "savepoint-bundle/v1"
    pub created_at: DateTime<Utc>,
    pub scrubbed_files_count: usize,
    pub scrubbed_secrets_count: usize,
    pub transaction: Transaction,
    /// Map of step_index -> (relative_path_string -> BundleFileState)
    pub step_files: BTreeMap<usize, BTreeMap<String, BundleFileState>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BundleFileState {
    Present {
        /// UTF-8 text (scrubbed) or hex-encoded bytes if non-UTF8
        content: String,
        is_hex: bool,
    },
    Deleted,
}
```

#### Export Algorithm (`FlightRecorder::export_bundle`):
1. Load the active or most recent committed transaction for `workspace_root` via `ShadowEngine::load_latest_transaction(workspace_root)`.
2. Collect the set of all `relative_path`s referenced in `cp.deltas` across all `tx.checkpoints`, and also scan `cp_0` for any sensitive files matching `is_sensitive_secret_path(&rel_path)` so they are counted in `scrubbed_files_count` and stripped from any checkpoint delta list.
3. Filter out any path matching `is_sensitive_secret_path(&rel_path)` (incrementing `scrubbed_files_count` for each unique sensitive path excluded).
4. For each checkpoint `cp` in `&tx.checkpoints`:
   - For each tracked `rel_path`:
     - Check `cp.snapshot_path.join(&rel_path)`:
       - If it exists as a regular file, read its bytes; if valid UTF-8, run `scrub_secrets_in_text(&text)` (incrementing `scrubbed_secrets_count`) and store `BundleFileState::Present { content, is_hex: false }`; otherwise hex-encode and store `is_hex: true`.
       - If it does not exist in `cp.snapshot_path`, store `BundleFileState::Deleted`.
5. Scrub `prompt` and `summary` in the exported `Transaction` copy.
6. Serialize `FlightBundle` to JSON and write to `output_path`.

#### Replay Algorithm (`FlightRecorder::replay_bundle`):
1. Read and deserialize `FlightBundle` from `bundle_path`.
2. Determine `step = target_step.unwrap_or_else(|| bundle.transaction.active_step())`.
3. Look up `step_map = bundle.step_files.get(&step)` (returning `SavepointError::CheckpointNotFound` if missing).
4. Ensure `target_workspace` exists, and for each `(rel_str, state)` in `step_map`:
   - Validate `rel_str` against path traversal (`..` or leading `/`).
   - `BundleFileState::Present { content, is_hex }`: write decoded content to `target_workspace.join(rel_str)`.
   - `BundleFileState::Deleted`: remove `target_workspace.join(rel_str)` if present.
5. Return `BundleReplayStats { transaction_id, replayed_step: step, restored_files_count }`.

---

## 4. Verification & Testing Strategy

1. **`test_flight_recorder_export_scrubs_secrets_and_replays_step_accurately` (`crates/savepoint-fs/tests/bundle_test.rs`):**
   - Create a workspace with `src/app.rs`, `.env` (`SECRET=sk-1234567890abcdefghij`), and a config file containing an inline `sk-proj1234567890abcdefghij` token.
   - Begin transaction, modify `src/app.rs` in Step 1 (`v2`), and corrupt `src/app.rs` in Step 2 (`v3_bug`).
   - Export `.spbundle` via `FlightRecorder::export_bundle`.
   - Verify `.env` is excluded (`scrubbed_files_count >= 1`) and the inline `sk-...` token is replaced with `[REDACTED_SECRET]` (`scrubbed_secrets_count >= 1`), and raw bundle JSON never contains the secret string.
   - Replay Step 1 into a fresh directory via `FlightRecorder::replay_bundle(&bundle_path, &repro_dir, Some(1))` and verify `src/app.rs` equals `v2`.
   - Replay Step 2 and verify `src/app.rs` equals `v3_bug`.
2. **`test_mcp_tools_lifecycle_and_bundle_export_replay` (`crates/savepoint-mcp/tests/mcp_test.rs`):**
   - Exercise `McpHandler::handle_request` for `initialize`, `ping`, `tools/list`, `savepoint_begin`, `savepoint_checkpoint`, `savepoint_speculate` (with `"policy": "first_pass_wins"`), `savepoint_export_bundle`, and `savepoint_replay_bundle`.
