# Savepoint 2026 — Sub-Project 3 Design Spec: Parallel Speculative Engine & Resource Governor (`savepoint-speculate`)

**Date:** 2026-11-07
**Status:** Approved
**Target Crate:** `savepoint-speculate`

---

## 1. Context & Problem Statement

In Savepoint 2026 (Agentic Transaction Hypervisor), `savepoint-speculate` allows an AI agent to dispatch multiple candidate implementation branches (`SpeculativeCandidate`) simultaneously, validate each against a test suite (`validation_cmd`), and automatically promote the winning branch back to the live workspace while recording a transaction checkpoint.

The initial spike in `crates/savepoint-speculate/src/speculate.rs` has four major limitations identified during our adversarial review:
1. **Sequential Execution Bottleneck:** `evaluate_and_converge` iterates over `candidates` in a synchronous `for` loop, running one candidate at a time instead of executing them concurrently across CPU cores.
2. **No Resource Governor (`CPU` / `RAM` Thrashing):** Spawning $N$ parallel test suites (`cargo test`, `jest`, `pytest`) without bounding concurrency against available CPU cores and system memory can exhaust host RAM and trigger OS swap thrashing or OOM kills.
3. **Port Collision Across Parallel Candidates:** Every candidate used `Sandbox::create` with `port_offset = 0`, causing `EADDRINUSE` port collisions when multiple candidates run integration tests concurrently.
4. **Rigid Winner Selection & No Early Pruning:** There is no support for choosing between:
   - `BestPassWins` (evaluate all candidates in parallel, rank passing candidates by fastest execution duration and fewest modified files, and promote the best one), or
   - `FirstPassWins` (as soon as one candidate passes validation, skip remaining queued candidates and converge immediately).

---

## 2. Goals & Non-Goals

### Goals
1. **Adaptive Resource Governor (`ResourceGovernor`):**
   - Inspect logical CPU count (`std::thread::available_parallelism`) and available/total system memory (`libc::sysconf` on Unix) to compute a safe concurrency ceiling `recommended_concurrency(mem_per_sandbox_mb: u64) -> usize` (clamped between `1` and `max_sandboxes`, default cap `8`).
2. **True Parallel Speculative Execution (`std::thread::scope` + Bounded Worker Queue):**
   - Execute candidates concurrently across worker threads up to `effective_concurrency`.
   - Assign each candidate $i$ a unique sanitized sandbox ID (`spec_{index}_{sanitized_name}`) and a unique `port_offset = (index + 1) as u16` via `SandboxConfig` so parallel test servers never collide on `$PORT`.
3. **Dual Convergence Policies (`ConvergencePolicy`):**
   - `ConvergencePolicy::BestPassWins` (Default): Run all candidates in parallel (bounded by `effective_concurrency`), collect all `CandidateReport`s in deterministic candidate order, and select the passing candidate with the lowest `(duration_ms, modified_files_count)` tuple.
   - `ConvergencePolicy::FirstPassWins`: Share an `Arc<AtomicBool>` (`winner_found`) across workers. As soon as any candidate passes validation, set `winner_found = true` so queued candidates that have not started yet are skipped (`skipped: true` in `CandidateReport`), and promote the first winner once active workers finish.
4. **Backward Compatibility & Clean Sandbox Lifecycle:**
   - Preserve `SpeculativeEngine::evaluate_and_converge(workspace_root, candidates, validation_cmd)` as a convenience wrapper around `SpeculativeEngine::evaluate_with_config(workspace_root, candidates, validation_cmd, SpeculationConfig::default())`.
   - Guarantee that all losing sandboxes are destroyed immediately upon completion and the single winning sandbox is destroyed right after `promote_to_workspace`.

### Non-Goals
- Distributed multi-machine execution (all sandboxes run on the local host volume).
- AST-level 3-way merging of multiple passing candidates (only the single winning candidate is promoted to the workspace).

---

## 3. Architecture & Component Design

### 3.1 Data Structures (`crates/savepoint-speculate/src/speculate.rs`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConvergencePolicy {
    /// Run all candidates (up to max concurrency) and pick the fastest/cleanest passing candidate.
    BestPassWins,
    /// Stop starting new queued candidates as soon as the first candidate passes validation.
    FirstPassWins,
}

impl Default for ConvergencePolicy {
    fn default() -> Self {
        Self::BestPassWins
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeculationConfig {
    /// Optional explicit concurrency limit. When `None`, `ResourceGovernor` computes it automatically.
    pub max_concurrency: Option<usize>,
    /// Estimated memory footprint per sandbox in MiB (default: 1024 MiB) used by `ResourceGovernor`.
    pub estimated_mem_per_sandbox_mb: u64,
    /// Timeout per command inside each candidate sandbox (default: 120s).
    pub command_timeout: Option<Duration>,
    /// Winner selection and early-pruning policy.
    pub policy: ConvergencePolicy,
    /// Whether sandboxes share CARGO_TARGET_DIR / NPM_CONFIG_CACHE / PIP_CACHE_DIR.
    pub share_build_cache: bool,
}

impl Default for SpeculationConfig {
    fn default() -> Self {
        Self {
            max_concurrency: None,
            estimated_mem_per_sandbox_mb: 1024,
            command_timeout: Some(Duration::from_secs(120)),
            policy: ConvergencePolicy::BestPassWins,
            share_build_cache: true,
        }
    }
}
```

`CandidateReport` is extended with backward-compatible fields:
- `pub branch_name: String`
- `pub passed: bool`
- `pub skipped: bool` (`#[serde(default)]`)
- `pub timed_out: bool` (`#[serde(default)]`)
- `pub exit_code: i32`
- `pub duration_ms: u128`
- `pub stdout: String`
- `pub stderr: String`
- `pub modified_files_count: usize`
- `pub deltas: Vec<FileDelta>`
- `pub port_offset: u16` (`#[serde(default)]`)

`SpeculationSummary` includes:
- `pub total_candidates: usize`
- `pub max_concurrency_used: usize`
- `pub winner_branch: Option<String>`
- `pub candidate_reports: Vec<CandidateReport>`
- `pub total_duration_ms: u128`

### 3.2 Adaptive Resource Governor (`ResourceGovernor`)
- `ResourceGovernor::detect_cpu_cores() -> usize`: Calls `std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2)`.
- `ResourceGovernor::detect_total_memory_mb() -> u64`: On Unix, queries `libc::sysconf(libc::_SC_PHYS_PAGES)` and `libc::sysconf(libc::_SC_PAGESIZE)` to compute total physical RAM in MiB (fallback `4096` MiB if unavailable).
- `ResourceGovernor::recommended_concurrency(estimated_mem_per_sandbox_mb: u64, candidate_count: usize) -> usize`:
  - `cpu_limit = (cores / 2).max(1)` (or `cores` if `cores <= 2`).
  - ` usable_mem_mb = total_mem_mb / 2` (reserving 50% RAM for OS & host IDE).
  - `mem_limit = (usable_mem_mb / estimated_mem_per_sandbox_mb.max(128)).max(1) as usize`.
  - Returns `cpu_limit.min(mem_limit).min(candidate_count.max(1)).clamp(1, 8)`.

### 3.3 Parallel Worker Pool & Execution Flow
1. Validate `!candidates.is_empty()`.
2. Compute `concurrency = config.max_concurrency.unwrap_or_else(|| ResourceGovernor::recommended_concurrency(config.estimated_mem_per_sandbox_mb, candidates.len())).clamp(1, candidates.len())`.
3. Use an `AtomicUsize` work-stealing index (`next_idx = 0`) over `&candidates` inside `std::thread::scope`, spawning `concurrency` worker threads:
   - Each worker atomically fetches `idx = next_idx.fetch_add(1, Ordering::SeqCst)`. If `idx >= candidates.len()`, the worker exits.
   - If `config.policy == ConvergencePolicy::FirstPassWins` and `winner_found.load(Ordering::SeqCst)` is true, record a skipped `CandidateReport` (`passed: false, skipped: true, exit_code: -1, duration_ms: 0`) and continue.
   - Otherwise, construct `SandboxConfig`:
     - `id = Some(format!("spec_{}_{}", idx, sanitize_id(&candidate.name)))`
     - `port_offset = (idx + 1) as u16`
     - `default_timeout = config.command_timeout`
     - `share_build_cache = config.share_build_cache`
   - Create `Sandbox::create_with_config(workspace_root, sb_cfg)`.
   - Run `candidate.setup_commands` sequentially via `sandbox.run_command("sh", &["-c", cmd_str])`. Stop on first non-zero exit code or timeout.
   - If setup succeeded and `!validation_cmd.trim().is_empty()`, run `sandbox.run_command("sh", &["-c", validation_cmd])`.
   - Compute `deltas = sandbox.compute_deltas().unwrap_or_default()`.
   - If `passed`:
     - Under `FirstPassWins`: if `winner_found.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok()`, keep `sandbox` alive in the worker result; otherwise destroy `sandbox`.
     - Under `BestPassWins`: keep `sandbox` alive in the worker result so the coordinator can compare all passing candidates and pick the best one, destroying the rest.
   - If `!passed`: destroy `sandbox` immediately (`let _ = sandbox.destroy();`).
4. After `std::thread::scope` joins all workers:
   - Sort results by original candidate index `idx` for deterministic reporting.
   - Select the winning `(Sandbox, CandidateReport)`:
     - Under `FirstPassWins`: the passing candidate that won the atomic flag (or first passing by completion).
     - Under `BestPassWins`: the passing candidate with minimum `(report.duration_ms, report.modified_files_count, idx)`.
   - Destroy all non-winning sandboxes that were kept alive.
   - For the winning sandbox:
     - Call `win_sb.promote_to_workspace(workspace_root)?;`
     - Call `let _ = win_sb.destroy();`
     - If an active `ShadowEngine` transaction exists on `workspace_root`, fork branch `winner_<sanitized_name>` and record a checkpoint.

---

## 4. Verification & Testing Strategy

1. **`test_parallel_speculation_speedup_and_unique_port_offsets`:**
   - Run 3 candidates in parallel where each candidate sleeps for `250ms` and echoes `$PORT|$SAVEPOINT_PORT_OFFSET`.
   - Verify wall-clock `total_duration_ms` is well under `600ms` (proving parallel execution rather than `3 * 250ms = 750ms+` sequential execution) and each candidate received a distinct `port_offset` (`1`, `2`, `3` -> ports `3001`, `3002`, `3003`).
2. **`test_best_pass_wins_selects_fastest_and_cleanest_candidate`:**
   - Run 3 passing candidates under `ConvergencePolicy::BestPassWins`: one slow (`300ms`), one fast (`20ms`), and one failing.
   - Verify `BestPassWins` promotes the fast passing candidate regardless of input array order.
3. **`test_first_pass_wins_prunes_queued_candidates`:**
   - Configure `max_concurrency: Some(1)` and `ConvergencePolicy::FirstPassWins` with 3 candidates where Candidate 0 passes immediately and Candidates 1 and 2 would fail/sleep.
   - Verify Candidate 0 wins and Candidates 1 and 2 are marked `skipped == true` with `duration_ms == 0`.
4. **`test_resource_governor_bounds_concurrency`:**
   - Verify `ResourceGovernor::recommended_concurrency` returns values within `1..=8` and respects high per-sandbox memory estimates by scaling down concurrency to `1`.
