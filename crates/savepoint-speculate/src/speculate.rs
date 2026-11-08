use savepoint_core::{FileDelta, Result, SavepointError};
use savepoint_fs::ShadowEngine;
use savepoint_sandbox::{Sandbox, SandboxConfig};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ConvergencePolicy {
    /// Run all candidates (up to max concurrency) and pick the fastest/cleanest passing candidate.
    #[default]
    BestPassWins,
    /// Stop starting new queued candidates as soon as the first candidate passes validation.
    FirstPassWins,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeculationConfig {
    /// Optional explicit concurrency limit. When `None`, `ResourceGovernor` computes it automatically.
    pub max_concurrency: Option<usize>,
    /// Estimated memory footprint per sandbox in MiB (default: 4096 MiB = 4 GB per Master Plan) used by `ResourceGovernor`.
    pub estimated_mem_per_sandbox_mb: u64,
    /// Timeout per command inside each candidate sandbox (default: 120s).
    pub command_timeout: Option<Duration>,
    /// Winner selection and early-pruning policy.
    pub policy: ConvergencePolicy,
    /// Whether sandboxes share CARGO_TARGET_DIR / NPM_CONFIG_CACHE / PIP_CACHE_DIR.
    pub share_build_cache: bool,
    /// Whether to block external network egress inside speculative sandboxes.
    pub block_network_egress: bool,
}

impl Default for SpeculationConfig {
    fn default() -> Self {
        Self {
            max_concurrency: None,
            estimated_mem_per_sandbox_mb: 4096,
            command_timeout: Some(Duration::from_secs(120)),
            policy: ConvergencePolicy::BestPassWins,
            share_build_cache: true,
            block_network_egress: false,
        }
    }
}

pub struct ResourceGovernor;

impl ResourceGovernor {
    /// Detects logical CPU cores available on the current machine.
    pub fn detect_cpu_cores() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .max(1)
    }

    /// Detects total physical RAM in MiB on Unix systems (fallback 4096 MiB).
    pub fn detect_total_memory_mb() -> u64 {
        #[cfg(unix)]
        unsafe {
            let pages = libc::sysconf(libc::_SC_PHYS_PAGES);
            let page_size = libc::sysconf(libc::_SC_PAGESIZE);
            if pages > 0 && page_size > 0 {
                let total_bytes = (pages as u128).saturating_mul(page_size as u128);
                let mb = (total_bytes / (1024 * 1024)) as u64;
                if mb >= 256 {
                    return mb;
                }
            }
        }
        4096
    }

    /// Computes a safe parallel sandbox limit using the Master Plan formula:
    /// `Max Sandboxes = min(floor(RAM_MB / estimated_mem_per_sandbox_mb), Cores / 2, 3)`
    pub fn recommended_concurrency(
        estimated_mem_per_sandbox_mb: u64,
        candidate_count: usize,
    ) -> usize {
        let cores = Self::detect_cpu_cores();
        let cpu_limit = (cores / 2).max(1);

        let total_mem_mb = Self::detect_total_memory_mb();
        let per_sb = estimated_mem_per_sandbox_mb.max(256);
        let mem_limit = (total_mem_mb / per_sb).max(1) as usize;

        cpu_limit
            .min(mem_limit)
            .min(3)
            .min(candidate_count.max(1))
            .clamp(1, 3)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeculativeCandidate {
    pub name: String,
    pub description: Option<String>,
    pub setup_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateReport {
    pub branch_name: String,
    pub passed: bool,
    #[serde(default)]
    pub skipped: bool,
    #[serde(default)]
    pub timed_out: bool,
    pub exit_code: i32,
    pub duration_ms: u128,
    pub stdout: String,
    pub stderr: String,
    pub modified_files_count: usize,
    pub deltas: Vec<FileDelta>,
    #[serde(default)]
    pub port_offset: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeculationSummary {
    pub total_candidates: usize,
    #[serde(default)]
    pub max_concurrency_used: usize,
    pub winner_branch: Option<String>,
    pub candidate_reports: Vec<CandidateReport>,
    pub total_duration_ms: u128,
}

struct WorkerOutcome {
    idx: usize,
    report: CandidateReport,
    sandbox: Option<Sandbox>,
}

pub struct SpeculativeEngine;

impl SpeculativeEngine {
    /// Executes multiple speculative branches inside ephemeral CoW sandboxes using default `SpeculationConfig`,
    /// validates each against `validation_cmd`, and promotes the winning branch to the workspace.
    pub fn evaluate_and_converge(
        workspace_root: &Path,
        candidates: Vec<SpeculativeCandidate>,
        validation_cmd: &str,
    ) -> Result<SpeculationSummary> {
        Self::evaluate_with_config(
            workspace_root,
            candidates,
            validation_cmd,
            SpeculationConfig::default(),
        )
    }

    /// Executes multiple speculative branches in parallel with bounded concurrency (`ResourceGovernor`)
    /// and configurable `ConvergencePolicy` (`BestPassWins` or `FirstPassWins`).
    pub fn evaluate_with_config(
        workspace_root: &Path,
        candidates: Vec<SpeculativeCandidate>,
        validation_cmd: &str,
        config: SpeculationConfig,
    ) -> Result<SpeculationSummary> {
        let global_start = Instant::now();
        if candidates.is_empty() {
            return Err(SavepointError::Other(
                "No speculative candidates provided".to_string(),
            ));
        }

        let total_candidates = candidates.len();
        let concurrency = config
            .max_concurrency
            .unwrap_or_else(|| {
                ResourceGovernor::recommended_concurrency(
                    config.estimated_mem_per_sandbox_mb,
                    total_candidates,
                )
            })
            .clamp(1, total_candidates);

        let next_idx = AtomicUsize::new(0);
        let winner_found = AtomicBool::new(false);
        let has_error = AtomicBool::new(false);
        let outcomes: Mutex<Vec<WorkerOutcome>> = Mutex::new(Vec::with_capacity(total_candidates));
        let first_err: Mutex<Option<SavepointError>> = Mutex::new(None);

        std::thread::scope(|s| {
            for _ in 0..concurrency {
                s.spawn(|| loop {
                    if has_error.load(Ordering::SeqCst) {
                        break;
                    }
                    let idx = next_idx.fetch_add(1, Ordering::SeqCst);
                    if idx >= total_candidates {
                        break;
                    }

                    let candidate = &candidates[idx];
                    let port_offset = (idx + 1) as u16;

                    // Early pruning for queued candidates under FirstPassWins
                    if config.policy == ConvergencePolicy::FirstPassWins
                        && winner_found.load(Ordering::SeqCst)
                    {
                        let skipped_report = CandidateReport {
                            branch_name: candidate.name.clone(),
                            passed: false,
                            skipped: true,
                            timed_out: false,
                            exit_code: -1,
                            duration_ms: 0,
                            stdout: String::new(),
                            stderr: "Skipped: earlier candidate already passed validation"
                                .to_string(),
                            modified_files_count: 0,
                            deltas: Vec::new(),
                            port_offset,
                        };
                        let mut guard = match outcomes.lock() {
                            Ok(g) => g,
                            Err(p) => p.into_inner(),
                        };
                        guard.push(WorkerOutcome {
                            idx,
                            report: skipped_report,
                            sandbox: None,
                        });
                        continue;
                    }

                    match evaluate_single_candidate(
                        workspace_root,
                        idx,
                        concurrency,
                        candidate,
                        validation_cmd,
                        &config,
                        &winner_found,
                    ) {
                        Ok(outcome) => {
                            let mut guard = match outcomes.lock() {
                                Ok(g) => g,
                                Err(p) => p.into_inner(),
                            };
                            guard.push(outcome);
                        }
                        Err(e) => {
                            has_error.store(true, Ordering::SeqCst);
                            let mut err_guard = match first_err.lock() {
                                Ok(g) => g,
                                Err(p) => p.into_inner(),
                            };
                            if err_guard.is_none() {
                                *err_guard = Some(e);
                            }
                            break;
                        }
                    }
                });
            }
        });

        // If a fatal error occurred, clean up any sandboxes collected so far and return the error
        let maybe_err = match first_err.into_inner() {
            Ok(e) => e,
            Err(p) => p.into_inner(),
        };
        let mut collected = match outcomes.into_inner() {
            Ok(v) => v,
            Err(p) => p.into_inner(),
        };

        if let Some(err) = maybe_err {
            for item in collected {
                if let Some(sb) = item.sandbox {
                    let _ = sb.destroy();
                }
            }
            return Err(err);
        }

        // Sort deterministically by candidate input index
        collected.sort_by_key(|o| o.idx);

        // Determine winning index among passing candidates that retained a live sandbox
        let winner_pos = match config.policy {
            ConvergencePolicy::FirstPassWins => collected
                .iter()
                .position(|o| o.report.passed && o.sandbox.is_some()),
            ConvergencePolicy::BestPassWins => collected
                .iter()
                .enumerate()
                .filter(|(_, o)| o.report.passed && o.sandbox.is_some())
                .min_by_key(|(_, o)| (o.report.duration_ms, o.report.modified_files_count, o.idx))
                .map(|(pos, _)| pos),
        };

        let mut reports = Vec::with_capacity(collected.len());
        let mut winning_sandbox: Option<(Sandbox, String)> = None;

        for (pos, item) in collected.into_iter().enumerate() {
            let branch_name = item.report.branch_name.clone();
            reports.push(item.report);
            if Some(pos) == winner_pos {
                if let Some(sb) = item.sandbox {
                    winning_sandbox = Some((sb, branch_name));
                }
            } else if let Some(sb) = item.sandbox {
                let _ = sb.destroy();
            }
        }

        let total_duration_ms = global_start.elapsed().as_millis();

        // Promote winning sandbox if found
        let winner_branch = if let Some((win_sb, win_name)) = winning_sandbox {
            let promote_res = win_sb.promote_to_workspace(workspace_root);
            let _ = win_sb.destroy();
            promote_res?;

            if let Ok(Some(mut tx)) = ShadowEngine::load_active_transaction(workspace_root) {
                let branch_name = format!("winner_{}", sanitize_id(&win_name));
                let _ = ShadowEngine::fork_branch(&mut tx, &branch_name);
                let msg = format!(
                    "Speculative Execution: Winner branch '{}' passed validation",
                    win_name
                );
                let _ = ShadowEngine::checkpoint(&mut tx, Some(&msg), Some(&msg));
            }

            Some(win_name)
        } else {
            None
        };

        Ok(SpeculationSummary {
            total_candidates: reports.len(),
            max_concurrency_used: concurrency,
            winner_branch,
            candidate_reports: reports,
            total_duration_ms,
        })
    }
}

fn evaluate_single_candidate(
    workspace_root: &Path,
    idx: usize,
    concurrency: usize,
    candidate: &SpeculativeCandidate,
    validation_cmd: &str,
    config: &SpeculationConfig,
    winner_found: &AtomicBool,
) -> Result<WorkerOutcome> {
    let start = Instant::now();
    let port_offset = (idx + 1) as u16;
    let sandbox_id = format!("spec_{}_{}", idx, sanitize_id(&candidate.name));
    let build_jobs = (ResourceGovernor::detect_cpu_cores() / concurrency.max(1)).max(1);

    let sb_cfg = SandboxConfig {
        id: Some(sandbox_id),
        port_offset,
        base_port: 3000,
        default_timeout: config.command_timeout,
        share_build_cache: config.share_build_cache,
        block_network_egress: config.block_network_egress,
        build_jobs_limit: Some(build_jobs),
        ..Default::default()
    };

    // Keep auto_cleanup = true so RAII Drop cleans up the sandbox directory on any early error/unwind!
    let sandbox = Sandbox::create_with_config(workspace_root, sb_cfg)?;

    let mut setup_ok = true;
    let mut skipped_mid_flight = false;
    let mut timed_out = false;
    let mut failure_msg = String::new();
    let mut setup_exit = 0;

    for cmd_str in &candidate.setup_commands {
        if config.policy == ConvergencePolicy::FirstPassWins && winner_found.load(Ordering::SeqCst)
        {
            setup_ok = false;
            skipped_mid_flight = true;
            setup_exit = -1;
            failure_msg =
                "Aborted mid-flight: another candidate already passed validation".to_string();
            break;
        }
        let output = sandbox.run_command("sh", &["-c", cmd_str])?;
        if output.timed_out {
            setup_ok = false;
            timed_out = true;
            setup_exit = output.exit_code;
            failure_msg = format!("Setup timed out: {}", output.stderr);
            break;
        }
        if output.exit_code != 0 {
            setup_ok = false;
            setup_exit = output.exit_code;
            failure_msg = format!(
                "Setup failed (code {}): {}",
                output.exit_code, output.stderr
            );
            break;
        }
    }

    if setup_ok
        && config.policy == ConvergencePolicy::FirstPassWins
        && winner_found.load(Ordering::SeqCst)
    {
        setup_ok = false;
        skipped_mid_flight = true;
        setup_exit = -1;
        failure_msg = "Aborted before validation: another candidate already passed".to_string();
    }

    let (passed, exit_code, stdout, stderr, val_timed_out) = if setup_ok {
        if validation_cmd.trim().is_empty() {
            (true, 0, String::new(), String::new(), false)
        } else {
            let v_output = sandbox.run_command("sh", &["-c", validation_cmd])?;
            (
                v_output.exit_code == 0 && !v_output.timed_out,
                v_output.exit_code,
                v_output.stdout,
                v_output.stderr,
                v_output.timed_out,
            )
        }
    } else {
        (false, setup_exit, String::new(), failure_msg, timed_out)
    };

    let duration_ms = start.elapsed().as_millis();
    let deltas = sandbox.compute_deltas().unwrap_or_default();

    let report = CandidateReport {
        branch_name: candidate.name.clone(),
        passed,
        skipped: skipped_mid_flight,
        timed_out: timed_out || val_timed_out,
        exit_code,
        duration_ms,
        stdout,
        stderr,
        modified_files_count: deltas.len(),
        deltas,
        port_offset,
    };

    let keep_sandbox = if passed {
        match config.policy {
            ConvergencePolicy::FirstPassWins => winner_found
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok(),
            ConvergencePolicy::BestPassWins => true,
        }
    } else {
        false
    };

    let retained_sandbox = if keep_sandbox {
        Some(sandbox)
    } else {
        let _ = sandbox.destroy();
        None
    };

    Ok(WorkerOutcome {
        idx,
        report,
        sandbox: retained_sandbox,
    })
}

fn sanitize_id(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "candidate".to_string()
    } else {
        out
    }
}
