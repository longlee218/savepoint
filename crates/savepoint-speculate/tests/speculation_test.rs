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
    assert_eq!(
        fs::read_to_string(ws_path.join("solver.txt"))
            .unwrap()
            .trim(),
        "fast"
    );
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

    let summary =
        SpeculativeEngine::evaluate_with_config(ws_path, candidates, "true", cfg).unwrap();

    assert_eq!(summary.winner_branch, Some("instant_winner".to_string()));
    assert!(!summary.candidate_reports[0].skipped);
    assert!(
        summary.candidate_reports[1].skipped,
        "Second candidate should be skipped"
    );
    assert!(
        summary.candidate_reports[2].skipped,
        "Third candidate should be skipped"
    );
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

#[test]
fn test_first_pass_wins_aborts_mid_flight_before_validation_and_leaves_zero_orphan_sandboxes() {
    let ws_dir = tempdir().unwrap();
    let ws_path = ws_dir.path();
    fs::write(ws_path.join("marker.txt"), "initial").unwrap();

    // Candidate 0 has a 200ms setup step, then would run a 10-second validation command
    // unless FirstPassWins checks winner_found between setup and validation!
    let cand_0 = SpeculativeCandidate {
        name: "slow setup / slash & space!".to_string(),
        description: None,
        setup_commands: vec![
            "sleep 0.25".to_string(),
            "echo 'cand0' > marker.txt".to_string(),
        ],
    };
    // Candidate 1 finishes setup in 10ms and passes validation immediately
    let cand_1 = SpeculativeCandidate {
        name: "fast/winner:branch".to_string(),
        description: None,
        setup_commands: vec!["echo 'cand1' > marker.txt".to_string()],
    };

    let cfg = SpeculationConfig {
        max_concurrency: Some(2),
        policy: ConvergencePolicy::FirstPassWins,
        command_timeout: Some(Duration::from_secs(10)),
        ..Default::default()
    };

    // Validation sleeps 5s if marker.txt == cand0, or exits 0 immediately if marker.txt == cand1
    let summary = SpeculativeEngine::evaluate_with_config(
        ws_path,
        vec![cand_0, cand_1],
        "if grep -q 'cand0' marker.txt; then sleep 5; fi",
        cfg,
    )
    .unwrap();

    assert_eq!(
        summary.winner_branch,
        Some("fast/winner:branch".to_string())
    );
    // Because cand_0 checks winner_found after its 250ms setup, it must skip the 5s validation!
    assert!(
        summary.total_duration_ms < 1500,
        "Expected mid-flight abort before 5s validation, took {}ms",
        summary.total_duration_ms
    );
    assert!(summary.candidate_reports[0].skipped);

    // Verify zero orphan sandbox directories remain under <storage>/sandboxes
    let sandboxes_dir = ShadowEngine::resolve_storage_dir(ws_path).join("sandboxes");
    if sandboxes_dir.exists() {
        let remaining: Vec<_> = fs::read_dir(&sandboxes_dir).unwrap().collect();
        assert!(
            remaining.is_empty(),
            "Expected 0 leftover sandboxes, found: {:?}",
            remaining
        );
    }
}

