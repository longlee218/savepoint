# Savepoint 2026 — Sub-Project 3 Implementation Plan: Parallel Speculative Engine & Resource Governor (`savepoint-speculate`)

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade `savepoint-speculate` from sequential candidate execution to a true parallel speculative engine with an adaptive CPU/RAM `ResourceGovernor`, unique `port_offset` isolation per candidate, and dual convergence policies (`BestPassWins` and `FirstPassWins`).

**Architecture:**
- Add `libc.workspace = true` to `crates/savepoint-speculate/Cargo.toml` so `ResourceGovernor` can query physical RAM (`_SC_PHYS_PAGES`, `_SC_PAGESIZE`) alongside logical CPU cores (`std::thread::available_parallelism`).
- Implement `ResourceGovernor`, `ConvergencePolicy`, `SpeculationConfig`, extended `CandidateReport` (`skipped`, `timed_out`, `port_offset`), extended `SpeculationSummary` (`max_concurrency_used`), and `SpeculativeEngine::evaluate_with_config` using `std::thread::scope` and an `AtomicUsize` work queue in `crates/savepoint-speculate/src/speculate.rs`.
- Re-export `ConvergencePolicy`, `ResourceGovernor`, and `SpeculationConfig` in `crates/savepoint-speculate/src/lib.rs`.
- Verify parallel speedup, port offset uniqueness, `BestPassWins` ranking, `FirstPassWins` early pruning, and `ResourceGovernor` scaling in `crates/savepoint-speculate/tests/speculation_test.rs`.

**Tech Stack:** Rust 2021, `std::thread::scope`, `std::sync::atomic::{AtomicBool, AtomicUsize, Ordering}`, `libc`, `savepoint-core`, `savepoint-fs`, `savepoint-sandbox`.

---

## File Structure & Responsibilities

1. `crates/savepoint-speculate/Cargo.toml`
   - Add `libc.workspace = true`.
2. `crates/savepoint-speculate/src/lib.rs`
   - Re-export `CandidateReport`, `ConvergencePolicy`, `ResourceGovernor`, `SpeculationConfig`, `SpeculationSummary`, `SpeculativeCandidate`, `SpeculativeEngine`.
3. `crates/savepoint-speculate/src/speculate.rs`
   - Implement `ResourceGovernor`, `ConvergencePolicy`, `SpeculationConfig`, parallel worker execution (`evaluate_with_config`), and winner promotion.
4. `crates/savepoint-speculate/tests/speculation_test.rs`
   - Integration tests covering parallel speedup + unique port offsets, `BestPassWins` selection, `FirstPassWins` pruning, and `ResourceGovernor` bounds.

---

### Task 1: Adaptive Resource Governor, Bounded Parallel Worker Pool & Dual Convergence Policy (`savepoint-speculate`)

**Files:**
- Modify: `crates/savepoint-speculate/Cargo.toml`
- Modify: `crates/savepoint-speculate/src/lib.rs`
- Modify: `crates/savepoint-speculate/src/speculate.rs`
- Modify: `crates/savepoint-speculate/tests/speculation_test.rs`

- [ ] **Step 1: Write failing integration tests in `crates/savepoint-speculate/tests/speculation_test.rs`**

```rust
use savepoint_fs::ShadowEngine;
use savepoint_speculate::{
    ConvergencePolicy, ResourceGovernor, SpeculationConfig, SpeculativeCandidate, SpeculativeEngine,
};
use std::fs;
use std::time::Duration;
use tempfile::tempdir;

#[test]
fn test_speculative_execution_auto_convergence() {
    let ws_dir = tempdir().expect("tempdir");
    let ws_path = ws_dir.path();

    let calc_file = ws_path.join("calc.sh");
    fs::write(&calc_file, "#!/bin/sh\nexit 1\n").expect("write calc.sh");

    let _tx = ShadowEngine::begin_transaction(ws_path).expect("begin");

    let cand_a = SpeculativeCandidate {
        name: "strategy_flawed".to_string(),
        description: Some("Flawed fix that fails".to_string()),
        setup_commands: vec!["echo 'exit 1' > calc.sh".to_string()],
    };

    let cand_b = SpeculativeCandidate {
        name: "strategy_optimal".to_string(),
        description: Some("Working fix that passes".to_string()),
        setup_commands: vec!["echo 'exit 0' > calc.sh".to_string()],
    };

    let summary = SpeculativeEngine::evaluate_and_converge(
        ws_path,
        vec![cand_a, cand_b],
        "sh calc.sh",
    )
    .expect("speculate");

    assert_eq!(summary.total_candidates, 2);
    assert_eq!(summary.winner_branch, Some("strategy_optimal".to_string()));

    let content = fs::read_to_string(&calc_file).unwrap();
    assert!(content.contains("exit 0"));
}

#[test]
fn test_parallel_speculation_speedup_and_unique_port_offsets() {
    let ws_dir = tempdir().unwrap();
    let ws_path = ws_dir.path();
    fs::write(ws_path.join("app.txt"), "base").unwrap();

    let candidates: Vec<SpeculativeCandidate> = (0..3)
        .map(|i| SpeculativeCandidate {
            name: format!("parallel_cand_{}", i),
            description: None,
            setup_commands: vec![format!("echo 'cand_{}' > app.txt", i)],
        })
        .collect();

    let cfg = SpeculationConfig {
        max_concurrency: Some(3),
        policy: ConvergencePolicy::BestPassWins,
        ..Default::default()
    };

    let summary = SpeculativeEngine::evaluate_with_config(
        ws_path,
        candidates,
        "sleep 0.25 && printf \"%s|%s\" \"$PORT\" \"$SAVEPOINT_PORT_OFFSET\"",
        cfg,
    )
    .unwrap();

    assert_eq!(summary.total_candidates, 3);
    assert_eq!(summary.max_concurrency_used, 3);
    // 3 * 250ms sequentially would take >= 750ms; parallel must complete much faster (< 650ms)
    assert!(
        summary.total_duration_ms < 650,
        "Expected parallel execution < 650ms, took {}ms",
        summary.total_duration_ms
    );

    // Verify each candidate received a distinct port_offset (1, 2, 3) and PORT (3001, 3002, 3003)
    assert_eq!(summary.candidate_reports[0].port_offset, 1);
    assert_eq!(summary.candidate_reports[0].stdout, "3001|1");
    assert_eq!(summary.candidate_reports[1].port_offset, 2);
    assert_eq!(summary.candidate_reports[1].stdout, "3002|2");
    assert_eq!(summary.candidate_reports[2].port_offset, 3);
    assert_eq!(summary.candidate_reports[2].stdout, "3003|3");
}

#[test]
fn test_best_pass_wins_selects_fastest_and_cleanest_candidate() {
    let ws_dir = tempdir().unwrap();
    let ws_path = ws_dir.path();
    fs::write(ws_path.join("solver.txt"), "init").unwrap();

    let cand_slow = SpeculativeCandidate {
        name: "slow_pass".to_string(),
        description: None,
        setup_commands: vec!["sleep 0.25 && echo 'slow' > solver.txt".to_string()],
    };
    let cand_fast = SpeculativeCandidate {
        name: "fast_pass".to_string(),
        description: None,
        setup_commands: vec!["echo 'fast' > solver.txt".to_string()],
    };
    let cand_fail = SpeculativeCandidate {
        name: "fail_branch".to_string(),
        description: None,
        setup_commands: vec!["exit 1".to_string()],
    };

    let cfg = SpeculationConfig {
        max_concurrency: Some(3),
        policy: ConvergencePolicy::BestPassWins,
        ..Default::default()
    };

    let summary = SpeculativeEngine::evaluate_with_config(
        ws_path,
        vec![cand_slow, cand_fast, cand_fail],
        "grep -q 'fast\\|slow' solver.txt",
        cfg,
    )
    .unwrap();

    assert_eq!(summary.winner_branch, Some("fast_pass".to_string()));
    assert_eq!(fs::read_to_string(ws_path.join("solver.txt")).unwrap().trim(), "fast");
}

#[test]
fn test_first_pass_wins_prunes_queued_candidates() {
    let ws_dir = tempdir().unwrap();
    let ws_path = ws_dir.path();
    fs::write(ws_path.join("result.txt"), "0").unwrap();

    let candidates = vec![
        SpeculativeCandidate {
            name: "instant_winner".to_string(),
            description: None,
            setup_commands: vec!["echo 'winner' > result.txt".to_string()],
        },
        SpeculativeCandidate {
            name: "queued_second".to_string(),
            description: None,
            setup_commands: vec!["sleep 5 && echo 'second' > result.txt".to_string()],
        },
        SpeculativeCandidate {
            name: "queued_third".to_string(),
            description: None,
            setup_commands: vec!["sleep 5 && echo 'third' > result.txt".to_string()],
        },
    ];

    let cfg = SpeculationConfig {
        max_concurrency: Some(1),
        policy: ConvergencePolicy::FirstPassWins,
        command_timeout: Some(Duration::from_secs(10)),
        ..Default::default()
    };

    let summary = SpeculativeEngine::evaluate_with_config(ws_path, candidates, "true", cfg).unwrap();

    assert_eq!(summary.winner_branch, Some("instant_winner".to_string()));
    assert!(!summary.candidate_reports[0].skipped);
    assert!(summary.candidate_reports[1].skipped, "Second candidate should be skipped");
    assert!(summary.candidate_reports[2].skipped, "Third candidate should be skipped");
    assert!(summary.total_duration_ms < 1000);
}

#[test]
fn test_resource_governor_bounds_concurrency() {
    let cores = ResourceGovernor::detect_cpu_cores();
    let mem_mb = ResourceGovernor::detect_total_memory_mb();
    assert!(cores >= 1);
    assert!(mem_mb >= 512);

    let normal = ResourceGovernor::recommended_concurrency(512, 10);
    assert!((1..=8).contains(&normal));

    // Requesting an enormous memory footprint per sandbox must clamp concurrency down to 1
    let constrained = ResourceGovernor::recommended_concurrency(10_000_000, 10);
    assert_eq!(constrained, 1);
}
```

- [ ] **Step 2: Run test to verify it fails (RED)**

Run: `cargo test --package savepoint-speculate`
Expected: Compilation failure (`unresolved imports ConvergencePolicy, ResourceGovernor, SpeculationConfig`, `no function evaluate_with_config`).

- [ ] **Step 3: Implement `ResourceGovernor`, `SpeculationConfig`, `ConvergencePolicy`, and parallel worker pool in `savepoint-speculate`**

1. Add `libc.workspace = true` to `crates/savepoint-speculate/Cargo.toml`.
2. Export `ConvergencePolicy`, `ResourceGovernor`, and `SpeculationConfig` in `crates/savepoint-speculate/src/lib.rs`.
3. Implement `ResourceGovernor`, `ConvergencePolicy`, `SpeculationConfig`, and parallel `evaluate_with_config` in `crates/savepoint-speculate/src/speculate.rs`.

- [ ] **Step 4: Run tests to verify they pass (GREEN)**

Run: `cargo test --package savepoint-speculate`
Expected: PASS (all 5 integration tests in `speculation_test.rs` pass).

- [ ] **Step 5: Commit**

```bash
git add crates/savepoint-speculate/Cargo.toml crates/savepoint-speculate/src/lib.rs crates/savepoint-speculate/src/speculate.rs crates/savepoint-speculate/tests/speculation_test.rs
git commit -m "feat(speculate): add parallel worker pool, adaptive CPU/RAM resource governor, and FirstPassWins/BestPassWins policies"
```

---

## Review Focus

- **Sandbox Cleanup Invariant Under Concurrency:** Verify that every `Sandbox` created across parallel worker threads is either destroyed immediately when failing/losing or destroyed right after `promote_to_workspace` when winning — zero orphaned sandbox directories may remain on disk even if multiple candidates pass simultaneously.
- **Deterministic Candidate Ordering & Tie-Breaking:** Verify `candidate_reports` in `SpeculationSummary` are always returned in the exact input candidate order regardless of thread completion order, and `BestPassWins` deterministically selects the passing candidate with minimum `(duration_ms, modified_files_count, candidate_index)`.
- **Sanitized Sandbox & Branch Identifiers:** Verify candidate names containing spaces, slashes, or special characters are sanitized before being passed as `SandboxConfig.id` or `ShadowEngine::fork_branch`.
