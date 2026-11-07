use clap::{Parser, Subcommand};
use std::fs;
use std::io::{stdout, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use savepoint_core::ChangeKind;
use savepoint_fs::{FlightRecorder, ShadowEngine};
use savepoint_mcp::protocol::JsonRpcRequest;
use savepoint_mcp::McpHandler;
use savepoint_pty::{PtySupervisor, RawTerminalGuard, SupervisorEvent};
use savepoint_speculate::{SpeculativeCandidate, SpeculativeEngine};
use savepoint_tui::{run_timeline, TimelineAction};

#[derive(Parser)]
#[command(
    name = "savepoint",
    author,
    version = "0.1.0",
    about = "The ACID Transaction & Time-Travel Runtime for AI Agents"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Wrap and run an interactive CLI agent under transaction supervision
    Run {
        /// Command to run (e.g. claude, aider, bash)
        #[arg(required = true, trailing_var_arg = true)]
        command: Vec<String>,
    },
    /// Open the interactive TUI timeline to inspect and rewind steps
    #[command(alias = "ui")]
    Timeline {
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Start a new ACID transaction for the current workspace
    Begin {
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Quickly rewind to the previous checkpoint (Undo 1 step)
    #[command(alias = "rollback")]
    Undo {
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Save a checkpoint after an agent turn or manual change
    #[command(alias = "checkpoint")]
    Save {
        #[arg(short, long)]
        message: Option<String>,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// View active transaction status and checkpoint timeline
    Status {
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Rewind workspace to a specific step
    Rewind {
        #[arg(short, long)]
        to: usize,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Commit active transaction to workspace
    Commit {
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Show file deltas since previous checkpoint
    Diff {
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Fork a new speculative branch from the current checkpoint
    Fork {
        /// Name of the new branch
        branch: String,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// List all branches in the active transaction
    #[command(alias = "branch")]
    Branches {
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Switch workspace to a specific branch
    Switch {
        /// Name of branch to switch to
        branch: String,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Merge a speculative branch into another branch
    Merge {
        /// Source branch to merge from
        source: String,
        /// Target branch to merge into (default: active branch)
        #[arg(short, long)]
        target: Option<String>,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Run multi-branch speculative execution across isolated CoW sandboxes
    Speculate {
        /// JSON string or file path containing candidate branches and setup commands
        #[arg(short, long)]
        candidates: String,
        /// Verification command (e.g. 'cargo test', 'npm test')
        #[arg(short, long)]
        validation: String,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Start the Model Context Protocol (MCP) JSON-RPC stdio server for AI agents
    Mcp,
    /// Export a secret-scrubbed portable .spbundle Flight Recorder file for bug reproduction
    Share {
        /// Output .spbundle path
        #[arg(short, long, default_value = "savepoint.spbundle")]
        output: PathBuf,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Replay a .spbundle Flight Recorder file into a target directory at a specific step
    Replay {
        /// Path to the .spbundle file
        bundle: PathBuf,
        /// Checkpoint step index to replay (defaults to active step in bundle)
        #[arg(short, long)]
        step: Option<usize>,
        /// Target directory to replay into
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Install integrations & hooks (e.g. Claude Code slash commands)
    Hook {
        #[command(subcommand)]
        target: HookTarget,
    },
}

#[derive(Subcommand)]
pub enum HookTarget {
    /// Install namespaced /sp:save, /sp:undo, /sp:timeline, /sp:diff, /sp:status slash commands into Claude Code (~/.claude/commands/)
    Claude,
}

pub fn run_cli() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Undo { path } => {
            let mut tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31m✖ No active Savepoint transaction found in {:?}. Run `sp begin` or `sp run` first.\x1b[0m", path);
                    return Ok(());
                }
            };

            let start = Instant::now();
            match ShadowEngine::rewind_previous(&mut tx)? {
                Some(target_step) => {
                    println!(
                        "\x1b[32m✔ [Savepoint]\x1b[0m ⏪ Rewound to Step \x1b[1m#{}\x1b[0m in \x1b[36m{:?}\x1b[0m",
                        target_step,
                        start.elapsed()
                    );
                }
                None => {
                    println!("\x1b[33m[Savepoint] Already at baseline step #0. Nothing to rewind.\x1b[0m");
                }
            }
        }
        Commands::Save { message, path } => {
            let mut tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31m✖ No active Savepoint transaction found in {:?}. Run `sp begin` first.\x1b[0m", path);
                    return Ok(());
                }
            };

            let start = Instant::now();
            let cp = ShadowEngine::checkpoint(&mut tx, message.as_deref(), None)?;
            let elapsed = start.elapsed();

            println!("\x1b[32m✔ [Savepoint]\x1b[0m Checkpoint \x1b[1m#{}\x1b[0m saved in \x1b[36m{:?}\x1b[0m", cp.step_index, elapsed);
            if let Some(msg) = cp.prompt {
                println!("  Prompt    : {}", msg);
            }
            println!("  Changes   : {} files affected", cp.deltas.len());
            for delta in &cp.deltas {
                match delta.kind {
                    ChangeKind::Created => println!("    \x1b[32m+ {}\x1b[0m", delta.relative_path.display()),
                    ChangeKind::Modified => println!("    \x1b[33m~ {}\x1b[0m", delta.relative_path.display()),
                    ChangeKind::Deleted => println!("    \x1b[31m- {}\x1b[0m", delta.relative_path.display()),
                }
            }
        }
        Commands::Hook { target } => match target {
            HookTarget::Claude => {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                let cmd_dir = PathBuf::from(home).join(".claude").join("commands");
                let sp_subdir = cmd_dir.join("sp");
                fs::create_dir_all(&sp_subdir)?;

                // Remove legacy un-namespaced commands if present
                let _ = fs::remove_file(cmd_dir.join("undo.md"));
                let _ = fs::remove_file(cmd_dir.join("save.md"));
                let _ = fs::remove_file(cmd_dir.join("timeline.md"));

                let commands = vec![
                    (
                        "save",
                        "---\ndescription: [Savepoint] Lưu lại điểm mốc code an toàn (Checkpoint)\n---\n!sp save -m \"$ARGUMENTS\"\n",
                    ),
                    (
                        "undo",
                        "---\ndescription: [Savepoint] Hoàn tác nhanh về điểm mốc an toàn gần nhất\n---\n!sp undo\n",
                    ),
                    (
                        "timeline",
                        "---\ndescription: [Savepoint] Mở giao diện Time Machine trực quan để xem lịch sử và tua lại\n---\n!sp ui\n",
                    ),
                    (
                        "diff",
                        "---\ndescription: [Savepoint] Xem các thay đổi trực tiếp (blast radius) chưa lưu mốc\n---\n!sp diff\n",
                    ),
                    (
                        "status",
                        "---\ndescription: [Savepoint] Xem trạng thái phiên giao dịch đang hoạt động\n---\n!sp status\n",
                    ),
                ];

                for (name, content) in &commands {
                    // 1. ~/.claude/commands/sp/<name>.md (Claude Code native namespacing /sp:<name>)
                    fs::write(sp_subdir.join(format!("{}.md", name)), content)?;
                    // 2. ~/.claude/commands/sp:<name>.md (fallback compatibility)
                    fs::write(cmd_dir.join(format!("sp:{}.md", name)), content)?;
                }

                println!("\x1b[32m✔ [Savepoint]\x1b[0m Successfully installed Claude Code namespaced slash commands!");
                println!("  Location : {:?}", cmd_dir);
                println!("  Prefix   : \x1b[33msp:*\x1b[0m (isolated namespace, không đè command khác)");
                println!("  Commands installed:");
                println!("    \x1b[36m/sp:save [ghi chú]\x1b[0m -> Lưu điểm mốc an toàn (Checkpoint)");
                println!("    \x1b[36m/sp:undo\x1b[0m           -> Hoàn tác nhanh về mốc trước đó");
                println!("    \x1b[36m/sp:timeline\x1b[0m       -> Mở Time Machine TUI trực quan");
                println!("    \x1b[36m/sp:diff\x1b[0m           -> Xem các thay đổi trực tiếp chưa lưu");
                println!("    \x1b[36m/sp:status\x1b[0m         -> Xem trạng thái phiên hiện tại");
            }
        },
        Commands::Run { command } => {
            let cwd = std::env::current_dir()?;
            println!("\x1b[36m⚡ [Savepoint]\x1b[0m Starting transactional session for \x1b[1m{:?}\x1b[0m", command);
            let mut tx = ShadowEngine::begin_transaction(&cwd)?;
            println!("\x1b[32m✔ [Savepoint]\x1b[0m Transaction \x1b[1m{}\x1b[0m active.", tx.id);
            println!("\x1b[90m  Hotkeys: [Ctrl+S] Quick Save | [Ctrl+R] Open TUI Timeline\x1b[0m");
            println!("\x1b[90m  Claude Slash Commands: /sp:save | /sp:undo | /sp:timeline | /sp:diff\x1b[0m\r\n");

            let program = &command[0];
            let args: Vec<&str> = command.iter().skip(1).map(|s| s.as_str()).collect();

            // Enter raw terminal mode
            let mut guard = RawTerminalGuard::new().ok();

            let supervisor = PtySupervisor::spawn(program, &args, Some(&cwd))?;

            while supervisor.is_running() {
                match supervisor.events_rx.recv_timeout(Duration::from_millis(1500)) {
                    Ok(event) => match event {
                        SupervisorEvent::CheckpointRequested => {
                            let start = Instant::now();
                            if let Ok(cp) = ShadowEngine::checkpoint(&mut tx, Some("Manual (Ctrl+S)"), None) {
                                print!(
                                    "\r\n\x1b[32m[Savepoint] Checkpoint #{} saved in {:?} ({} deltas)\x1b[0m\r\n",
                                    cp.step_index,
                                    start.elapsed(),
                                    cp.deltas.len()
                                );
                                let _ = stdout().flush();
                            }
                        }
                        SupervisorEvent::RewindRequested => {
                            // Drop raw terminal mode so Ratatui TUI takes over smoothly
                            drop(guard.take());

                            let live_deltas = ShadowEngine::compute_live_deltas(&tx).unwrap_or_default();

                            // Launch the interactive TUI timeline!
                            if let Ok(action) = run_timeline(tx.clone(), live_deltas) {
                                match action {
                                    TimelineAction::Rewind(target_step) => {
                                        let start = Instant::now();
                                        if ShadowEngine::rewind(&mut tx, target_step).is_ok() {
                                            println!(
                                                "\r\n\x1b[33m[Savepoint] ⏪ Rewound workspace to Step #{} in {:?}!\x1b[0m\r\n",
                                                target_step,
                                                start.elapsed()
                                            );
                                        }
                                    }
                                    TimelineAction::Commit => {
                                        let _ = ShadowEngine::commit(&mut tx);
                                        println!("\r\n\x1b[32m[Savepoint] ✅ Transaction committed via TUI.\x1b[0m\r\n");
                                    }
                                    TimelineAction::Cancel => {}
                                }
                            }

                            // Re-enter raw terminal mode for child process
                            guard = RawTerminalGuard::new().ok();
                        }
                        SupervisorEvent::ChildExited(code) => {
                            drop(guard.take());
                            println!("\r\n\x1b[36m⚡ [Savepoint]\x1b[0m Child process exited with status {}.", code);
                            let _ = ShadowEngine::commit(&mut tx);
                            println!("\x1b[32m✔ [Savepoint]\x1b[0m Transaction {} committed.\x1b[0m", tx.id);
                            break;
                        }
                    },
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Background Auto-Save: If agent created/modified/deleted files, take an instant APFS snapshot
                        if let Ok(deltas) = ShadowEngine::compute_live_deltas(&tx) {
                            if !deltas.is_empty() {
                                let _ = ShadowEngine::checkpoint(
                                    &mut tx,
                                    Some("Tự động lưu: Phát hiện thay đổi trong thư mục"),
                                    None,
                                );
                            }
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                }
            }
        }
        Commands::Timeline { path } => {
            let mut tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31m✖ No active Savepoint transaction found in {:?}. Run `sp begin` or `sp run` first.\x1b[0m", path);
                    return Ok(());
                }
            };

            let live_deltas = ShadowEngine::compute_live_deltas(&tx).unwrap_or_default();
            let action = run_timeline(tx.clone(), live_deltas)?;
            match action {
                TimelineAction::Rewind(step) => {
                    let start = Instant::now();
                    ShadowEngine::rewind(&mut tx, step)?;
                    println!("\x1b[32m✔ [Savepoint]\x1b[0m ⏪ Rewound to Step \x1b[1m#{}\x1b[0m in \x1b[36m{:?}\x1b[0m", step, start.elapsed());
                }
                TimelineAction::Commit => {
                    ShadowEngine::commit(&mut tx)?;
                    println!("\x1b[32m✔ [Savepoint]\x1b[0m ✅ Transaction \x1b[1m{}\x1b[0m committed.\x1b[0m", tx.id);
                }
                TimelineAction::Cancel => {
                    println!("TUI dismissed.");
                }
            }
        }
        Commands::Begin { path } => {
            let start = Instant::now();
            let tx = ShadowEngine::begin_transaction(&path)?;
            let elapsed = start.elapsed();
            println!("\x1b[32m✔ [Savepoint]\x1b[0m Transaction \x1b[1m{}\x1b[0m started in \x1b[36m{:?}\x1b[0m", tx.id, elapsed);
            println!("  Workspace : {:?}", tx.workspace_root);
            println!("  Storage   : {:?}", tx.shadow_root);
            println!("  Baseline  : Step #0 captured via APFS Copy-on-Write.");
        }
        Commands::Status { path } => {
            let tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    println!("\x1b[33mNo active Savepoint transaction in {:?}\x1b[0m", path);
                    return Ok(());
                }
            };

            println!("\x1b[1m=== Savepoint Transaction: {} ===\x1b[0m", tx.id);
            println!("Status    : {:?}", tx.status);
            println!("Branch    : \x1b[32m* {}\x1b[0m (All: {})", tx.active_branch, tx.branch_names().join(", "));
            println!("Workspace : {:?}", tx.workspace_root);
            println!("Timeline  : {} checkpoints", tx.checkpoints.len());

            for cp in &tx.checkpoints {
                println!(
                    "  \x1b[36m[Step #{}]\x1b[0m [{}] {} | {} deltas",
                    cp.step_index,
                    cp.branch,
                    cp.prompt.as_deref().unwrap_or("No description"),
                    cp.deltas.len()
                );
            }
        }
        Commands::Rewind { to, path } => {
            let mut tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31m✖ No active Savepoint transaction found.\x1b[0m");
                    return Ok(());
                }
            };

            let prev_step = tx.active_step();
            let start = Instant::now();
            ShadowEngine::rewind(&mut tx, to)?;
            let elapsed = start.elapsed();

            if to >= prev_step {
                println!("\x1b[32m✔ [Savepoint]\x1b[0m ⏩ Fast-forwarded (Redo) to Step \x1b[1m#{}\x1b[0m in \x1b[36m{:?}\x1b[0m", to, elapsed);
            } else {
                println!("\x1b[32m✔ [Savepoint]\x1b[0m ⏪ Rewound (Undo) to Step \x1b[1m#{}\x1b[0m in \x1b[36m{:?}\x1b[0m", to, elapsed);
            }
            println!("  Current step is now #{}.", tx.active_step());
        }
        Commands::Commit { path } => {
            let mut tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31m✖ No active Savepoint transaction found.\x1b[0m");
                    return Ok(());
                }
            };

            let start = Instant::now();
            ShadowEngine::commit(&mut tx)?;
            let elapsed = start.elapsed();

            println!("\x1b[32m✔ [Savepoint]\x1b[0m ✅ Transaction \x1b[1m{}\x1b[0m committed in \x1b[36m{:?}\x1b[0m", tx.id, elapsed);
        }
        Commands::Diff { path } => {
            let tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31m✖ No active Savepoint transaction found.\x1b[0m");
                    return Ok(());
                }
            };

            let live_deltas = ShadowEngine::compute_live_deltas(&tx)?;
            if live_deltas.is_empty() {
                println!("\x1b[32m✔ Thư mục làm việc sạch sẽ (không có thay đổi so với mốc gần nhất).\x1b[0m");
            } else {
                println!("\x1b[1m=== Thay Đổi Trực Tiếp Hiện Tại (Chưa lưu mốc) ===\x1b[0m");
                for delta in &live_deltas {
                    match delta.kind {
                        ChangeKind::Created => println!("  \x1b[32m+ [Tạo mới]  {}\x1b[0m", delta.relative_path.display()),
                        ChangeKind::Modified => println!("  \x1b[33m~ [Sửa đổi]  {}\x1b[0m", delta.relative_path.display()),
                        ChangeKind::Deleted => println!("  \x1b[31m- [Đã xóa]   {}\x1b[0m", delta.relative_path.display()),
                    }
                }
            }
        }
        Commands::Fork { branch, path } => {
            let mut tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31m✖ No active Savepoint transaction found.\x1b[0m");
                    return Ok(());
                }
            };
            let start = Instant::now();
            ShadowEngine::fork_branch(&mut tx, &branch)?;
            println!("\x1b[32m✔ [Savepoint]\x1b[0m Forked new branch \x1b[1m'{}'\x1b[0m from Step #{} in \x1b[36m{:?}\x1b[0m", branch, tx.active_step(), start.elapsed());
        }
        Commands::Branches { path } => {
            let tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31m✖ No active Savepoint transaction found.\x1b[0m");
                    return Ok(());
                }
            };
            println!("\x1b[1m=== Branches in Transaction {} ===\x1b[0m", tx.id);
            for b in tx.branch_names() {
                if b == tx.active_branch {
                    println!("  \x1b[32m* {} (HEAD -> step #{})\x1b[0m", b, tx.branches.get(&b).unwrap_or(&0));
                } else {
                    println!("    {} (step #{})", b, tx.branches.get(&b).unwrap_or(&0));
                }
            }
        }
        Commands::Switch { branch, path } => {
            let mut tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31m✖ No active Savepoint transaction found.\x1b[0m");
                    return Ok(());
                }
            };
            let start = Instant::now();
            let step = ShadowEngine::switch_branch(&mut tx, &branch)?;
            println!("\x1b[32m✔ [Savepoint]\x1b[0m Switched to branch \x1b[1m'{}'\x1b[0m (Step #{}) in \x1b[36m{:?}\x1b[0m", branch, step, start.elapsed());
        }
        Commands::Merge { source, target, path } => {
            let mut tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31m✖ No active Savepoint transaction found.\x1b[0m");
                    return Ok(());
                }
            };
            let target_branch = target.unwrap_or_else(|| tx.active_branch.clone());
            let start = Instant::now();
            let cp = ShadowEngine::merge_branch(&mut tx, &source, &target_branch)?;
            println!("\x1b[32m✔ [Savepoint]\x1b[0m Merged branch \x1b[1m'{}'\x1b[0m into \x1b[1m'{}'\x1b[0m at Checkpoint #{} in \x1b[36m{:?}\x1b[0m", source, target_branch, cp.step_index, start.elapsed());
        }
        Commands::Speculate { candidates, validation, path } => {
            let candidates_data = if candidates.trim().starts_with('[') {
                candidates
            } else {
                fs::read_to_string(&candidates)?
            };
            let parsed_candidates: Vec<SpeculativeCandidate> = serde_json::from_str(&candidates_data)?;
            let start = Instant::now();
            let summary = SpeculativeEngine::evaluate_and_converge(&path, parsed_candidates, &validation)?;
            println!("\x1b[1m=== Savepoint Speculative Execution Engine ===\x1b[0m");
            println!("Candidates evaluated: {}", summary.total_candidates);
            println!("Total time          : {:?}", start.elapsed());
            match summary.winner_branch {
                Some(winner) => {
                    println!("\x1b[32m✔ Winner Branch: '{}' passed validation and was atomically promoted to workspace!\x1b[0m", winner);
                }
                None => {
                    println!("\x1b[31m✖ All speculative candidates failed validation. Workspace left 100% pristine.\x1b[0m");
                }
            }
            for rep in &summary.candidate_reports {
                let status_icon = if rep.passed { "\x1b[32m✔ PASS\x1b[0m" } else { "\x1b[31m✖ FAIL\x1b[0m" };
                println!("  {} [{}] in {:?} (modified {} files)", status_icon, rep.branch_name, Duration::from_millis(rep.duration_ms as u64), rep.modified_files_count);
            }
        }
        Commands::Mcp => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(async {
                use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
                let stdin = tokio::io::stdin();
                let mut stdout = tokio::io::stdout();
                let reader = BufReader::new(stdin);
                let mut lines = reader.lines();

                while let Ok(Some(line)) = lines.next_line().await {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(req) = serde_json::from_str::<JsonRpcRequest>(trimmed) {
                        if let Some(resp) = McpHandler::handle_request(req) {
                            if let Ok(serialized) = serde_json::to_string(&resp) {
                                let _ = stdout.write_all(serialized.as_bytes()).await;
                                let _ = stdout.write_all(b"\n").await;
                                let _ = stdout.flush().await;
                            }
                        }
                    }
                }
                Ok::<(), anyhow::Error>(())
            })?;
        }
        Commands::Share { output, path } => {
            let start = Instant::now();
            let stats = FlightRecorder::export_bundle(&path, &output)?;
            println!(
                "\x1b[32m✔ [Savepoint]\x1b[0m 📦 Exported Flight Recorder bundle to \x1b[1m{:?}\x1b[0m in \x1b[36m{:?}\x1b[0m",
                stats.output_path,
                start.elapsed()
            );
            println!("  Transaction    : {}", stats.transaction_id);
            println!("  Checkpoints    : {}", stats.checkpoints_count);
            println!("  Tracked files  : {}", stats.tracked_files_count);
            println!(
                "  Secret Shield  : {} sensitive file(s) excluded, {} inline token(s) redacted",
                stats.scrubbed_files_count, stats.scrubbed_secrets_count
            );
        }
        Commands::Replay { bundle, step, path } => {
            let start = Instant::now();
            let stats = FlightRecorder::replay_bundle(&bundle, &path, step)?;
            println!(
                "\x1b[32m✔ [Savepoint]\x1b[0m ⏪ Replayed bundle \x1b[1m{}\x1b[0m (Step #{}) into \x1b[1m{:?}\x1b[0m in \x1b[36m{:?}\x1b[0m ({} files restored)",
                stats.transaction_id,
                stats.replayed_step,
                path,
                start.elapsed(),
                stats.restored_files_count
            );
        }
    }

    Ok(())
}
