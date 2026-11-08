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
        /// Output status as JSON
        #[arg(short, long)]
        json: bool,
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
    /// Squash active Savepoint transaction into a real Git commit
    #[command(alias = "squash")]
    ToGit {
        /// Optional custom Git commit message
        #[arg(short, long)]
        message: Option<String>,
        /// Optional checkpoint step index to squash up to (defaults to active step)
        #[arg(short, long)]
        step: Option<usize>,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Show file deltas between checkpoints or since previous checkpoint
    Diff {
        /// Starting step index (optional)
        #[arg(long)]
        from: Option<usize>,
        /// Target step index (optional)
        #[arg(long)]
        to: Option<usize>,
        /// Output diff as JSON
        #[arg(short, long)]
        json: bool,
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
        /// Optional specific checkpoint step index to pin in the exported bundle
        #[arg(short, long)]
        step: Option<usize>,
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Replay a .spbundle Flight Recorder file or sp://replay/<id> URI into a target directory
    Replay {
        /// Path to the .spbundle file or sp://replay/<id> URI
        bundle: PathBuf,
        /// Checkpoint step index to replay (defaults to pinned or active step in bundle)
        #[arg(short, long)]
        step: Option<usize>,
        /// Inspect and preview bundle contents without modifying workspace
        #[arg(long)]
        dry_run: bool,
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
                    eprintln!("\x1b[31merror: no active Savepoint transaction found in {:?}. Run `sp begin` or `sp run` first.\x1b[0m", path);
                    return Ok(());
                }
            };

            let start = Instant::now();
            match ShadowEngine::rewind_previous(&mut tx)? {
                Some(target_step) => {
                    println!(
                        "\x1b[32m[savepoint]\x1b[0m Rewound to Step \x1b[1m#{}\x1b[0m in \x1b[36m{:?}\x1b[0m",
                        target_step,
                        start.elapsed()
                    );
                }
                None => {
                    println!("\x1b[33m[savepoint] Already at baseline step #0. Nothing to rewind.\x1b[0m");
                }
            }
        }
        Commands::Save { message, path } => {
            let mut tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31merror: no active Savepoint transaction found in {:?}. Run `sp begin` first.\x1b[0m", path);
                    return Ok(());
                }
            };

            let start = Instant::now();
            let cp = ShadowEngine::checkpoint(&mut tx, message.as_deref(), None)?;
            let elapsed = start.elapsed();

            println!("\x1b[32m[savepoint]\x1b[0m Checkpoint \x1b[1m#{}\x1b[0m saved in \x1b[36m{:?}\x1b[0m", cp.step_index, elapsed);
            if let Some(msg) = cp.prompt {
                println!("  Message : {}", msg);
            }
            println!("  Changes : {} file(s)", cp.deltas.len());
            for delta in &cp.deltas {
                match delta.kind {
                    ChangeKind::Created => println!("    \x1b[32mA  {}\x1b[0m", delta.relative_path.display()),
                    ChangeKind::Modified => println!("    \x1b[33mM  {}\x1b[0m", delta.relative_path.display()),
                    ChangeKind::Deleted => println!("    \x1b[31mD  {}\x1b[0m", delta.relative_path.display()),
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
                        "---\ndescription: [Savepoint] Save an atomic CoW workspace checkpoint\n---\n!sp save -m \"$ARGUMENTS\"\n",
                    ),
                    (
                        "undo",
                        "---\ndescription: [Savepoint] Rewind workspace to the previous checkpoint\n---\n!sp undo\n",
                    ),
                    (
                        "timeline",
                        "---\ndescription: [Savepoint] Open interactive TUI timeline inspector\n---\n!sp ui\n",
                    ),
                    (
                        "diff",
                        "---\ndescription: [Savepoint] Show uncommitted workspace file changes since HEAD\n---\n!sp diff\n",
                    ),
                    (
                        "status",
                        "---\ndescription: [Savepoint] Show active transaction status and checkpoint list\n---\n!sp status\n",
                    ),
                    (
                        "fork",
                        "---\ndescription: [Savepoint] Fork a new speculative branch from the current checkpoint\n---\n!sp fork $ARGUMENTS\n",
                    ),
                    (
                        "branches",
                        "---\ndescription: [Savepoint] List all branches in the active transaction\n---\n!sp branches\n",
                    ),
                    (
                        "share",
                        "---\ndescription: [Savepoint] Export a secret-scrubbed .spbundle Flight Recorder file\n---\n!sp share $ARGUMENTS\n",
                    ),
                    (
                        "replay",
                        "---\ndescription: [Savepoint] Replay a .spbundle file or sp://replay/<id> URI into the workspace\n---\n!sp replay $ARGUMENTS\n",
                    ),
                    (
                        "to-git",
                        "---\ndescription: [Savepoint] Squash active transaction steps into a real Git commit\n---\n!sp to-git -m \"$ARGUMENTS\"\n",
                    ),
                ];

                for (name, content) in &commands {
                    // 1. ~/.claude/commands/sp/<name>.md (Claude Code native namespacing /sp:<name>)
                    fs::write(sp_subdir.join(format!("{}.md", name)), content)?;
                    // 2. ~/.claude/commands/sp:<name>.md (fallback compatibility)
                    fs::write(cmd_dir.join(format!("sp:{}.md", name)), content)?;
                }

                println!("\x1b[32m[savepoint]\x1b[0m Installed Claude Code namespaced slash commands:");
                println!("  Directory : {:?}", cmd_dir);
                println!("  Namespace : \x1b[33msp:*\x1b[0m");
                println!("  Commands  :");
                println!("    \x1b[36m/sp:save [msg]\x1b[0m  Save a workspace checkpoint");
                println!("    \x1b[36m/sp:undo\x1b[0m        Rewind to previous checkpoint");
                println!("    \x1b[36m/sp:timeline\x1b[0m    Open interactive TUI timeline");
                println!("    \x1b[36m/sp:diff\x1b[0m        Show uncommitted workspace changes");
                println!("    \x1b[36m/sp:status\x1b[0m      Show active transaction status");
                println!("    \x1b[36m/sp:to-git [msg]\x1b[0m Squash transaction into Git commit");
            }
        },
        Commands::Run { command } => {
            let cwd = std::env::current_dir()?;
            println!("\x1b[36m[savepoint]\x1b[0m Starting supervised session for \x1b[1m{:?}\x1b[0m", command);
            let mut tx = ShadowEngine::begin_transaction(&cwd)?;
            println!("\x1b[32m[savepoint]\x1b[0m Transaction \x1b[1m{}\x1b[0m active.", tx.id);
            println!("\x1b[90m  Hotkeys: [Ctrl+S] Save checkpoint | [Ctrl+R] Open TUI timeline\x1b[0m");
            println!("\x1b[90m  Slash  : /sp:save | /sp:undo | /sp:timeline | /sp:diff\x1b[0m\r\n");

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
                                    "\r\n\x1b[32m[savepoint] Checkpoint #{} saved in {:?} ({} files)\x1b[0m\r\n",
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

                            // Launch the interactive TUI timeline
                            if let Ok(action) = run_timeline(tx.clone(), live_deltas) {
                                match action {
                                    TimelineAction::Rewind(target_step) => {
                                        let start = Instant::now();
                                        if ShadowEngine::rewind(&mut tx, target_step).is_ok() {
                                            println!(
                                                "\r\n\x1b[33m[savepoint] Restored workspace to Step #{} in {:?}.\x1b[0m\r\n",
                                                target_step,
                                                start.elapsed()
                                            );
                                        }
                                    }
                                    TimelineAction::Commit => {
                                        let _ = ShadowEngine::commit(&mut tx);
                                        println!("\r\n\x1b[32m[savepoint] Transaction committed via TUI.\x1b[0m\r\n");
                                    }
                                    TimelineAction::Cancel => {}
                                }
                            }

                            // Re-enter raw terminal mode for child process
                            guard = RawTerminalGuard::new().ok();
                        }
                        SupervisorEvent::ChildExited(code) => {
                            drop(guard.take());
                            println!("\r\n\x1b[36m[savepoint]\x1b[0m Child process exited with status {}.", code);
                            let _ = ShadowEngine::commit(&mut tx);
                            println!("\x1b[32m[savepoint]\x1b[0m Transaction {} committed.\x1b[0m", tx.id);
                            break;
                        }
                    },
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Background Auto-Save: If agent created/modified/deleted files, take an instant APFS snapshot
                        if let Ok(deltas) = ShadowEngine::compute_live_deltas(&tx) {
                            if !deltas.is_empty() {
                                let _ = ShadowEngine::checkpoint(
                                    &mut tx,
                                    Some("Auto-save: workspace changes detected"),
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
                    eprintln!("\x1b[31merror: no active Savepoint transaction found in {:?}. Run `sp begin` or `sp run` first.\x1b[0m", path);
                    return Ok(());
                }
            };

            let live_deltas = ShadowEngine::compute_live_deltas(&tx).unwrap_or_default();
            let action = run_timeline(tx.clone(), live_deltas)?;
            match action {
                TimelineAction::Rewind(step) => {
                    let start = Instant::now();
                    ShadowEngine::rewind(&mut tx, step)?;
                    println!("\x1b[32m[savepoint]\x1b[0m Restored to Step \x1b[1m#{}\x1b[0m in \x1b[36m{:?}\x1b[0m", step, start.elapsed());
                }
                TimelineAction::Commit => {
                    ShadowEngine::commit(&mut tx)?;
                    println!("\x1b[32m[savepoint]\x1b[0m Transaction \x1b[1m{}\x1b[0m committed.\x1b[0m", tx.id);
                }
                TimelineAction::Cancel => {}
            }
        }
        Commands::Begin { path } => {
            let start = Instant::now();
            let tx = ShadowEngine::begin_transaction(&path)?;
            let elapsed = start.elapsed();
            println!("\x1b[32m[savepoint]\x1b[0m Transaction \x1b[1m{}\x1b[0m started in \x1b[36m{:?}\x1b[0m", tx.id, elapsed);
            println!("  Workspace : {:?}", tx.workspace_root);
            println!("  Storage   : {:?}", tx.shadow_root);
            println!("  Baseline  : Step #0 captured via CoW.");
        }
        Commands::Status { json, path } => {
            let tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    if json {
                        println!("null");
                    } else {
                        println!("\x1b[33mNo active Savepoint transaction in {:?}\x1b[0m", path);
                    }
                    return Ok(());
                }
            };

            if json {
                println!("{}", serde_json::to_string_pretty(&tx)?);
                return Ok(());
            }

            println!("\x1b[1mSavepoint Transaction: {}\x1b[0m", tx.id);
            println!("  Status    : {:?}", tx.status);
            println!("  Branch    : \x1b[32m* {}\x1b[0m (all: {})", tx.active_branch, tx.branch_names().join(", "));
            println!("  HEAD      : #{:02}", tx.active_step());
            println!("  Workspace : {:?}", tx.workspace_root);
            println!("  Timeline  : {} checkpoint(s)", tx.checkpoints.len());

            for cp in &tx.checkpoints {
                let marker = if cp.step_index == tx.active_step() { "*" } else { " " };
                println!(
                    "  {} \x1b[36m#{:02}\x1b[0m [{}] {:>2}f  {}",
                    marker,
                    cp.step_index,
                    cp.branch,
                    cp.deltas.len(),
                    cp.prompt.as_deref().unwrap_or("Baseline snapshot")
                );
            }
        }
        Commands::Rewind { to, path } => {
            let mut tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31merror: no active Savepoint transaction found.\x1b[0m");
                    return Ok(());
                }
            };

            let prev_step = tx.active_step();
            let start = Instant::now();
            ShadowEngine::rewind(&mut tx, to)?;
            let elapsed = start.elapsed();

            if to >= prev_step {
                println!("\x1b[32m[savepoint]\x1b[0m Fast-forwarded to Step \x1b[1m#{}\x1b[0m in \x1b[36m{:?}\x1b[0m", to, elapsed);
            } else {
                println!("\x1b[32m[savepoint]\x1b[0m Rewound to Step \x1b[1m#{}\x1b[0m in \x1b[36m{:?}\x1b[0m", to, elapsed);
            }
        }
        Commands::Commit { path } => {
            let mut tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31merror: no active Savepoint transaction found.\x1b[0m");
                    return Ok(());
                }
            };

            let start = Instant::now();
            ShadowEngine::commit(&mut tx)?;
            let elapsed = start.elapsed();

            println!("\x1b[32m[savepoint]\x1b[0m Transaction \x1b[1m{}\x1b[0m committed in \x1b[36m{:?}\x1b[0m", tx.id, elapsed);
        }
        Commands::ToGit { message, step, path } => {
            let mut tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31merror: no active Savepoint transaction found in {:?}\x1b[0m", path);
                    return Ok(());
                }
            };

            let start = Instant::now();
            let res = ShadowEngine::squash_to_git(&mut tx, message.as_deref(), step)?;
            println!(
                "\x1b[32m[savepoint]\x1b[0m Squashed {} step(s) into Git commit \x1b[1m{}\x1b[0m in \x1b[36m{:?}\x1b[0m",
                res.steps_squashed, res.commit_hash, start.elapsed()
            );
            println!("  Summary    : {}", res.commit_message.lines().next().unwrap_or(""));
            println!("  Files      : {} file delta(s)", res.files_changed);
            println!("  Status     : Savepoint transaction committed; Git working tree in sync.");
        }
        Commands::Diff { from, to, json, path } => {
            let tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31merror: no active Savepoint transaction found.\x1b[0m");
                    return Ok(());
                }
            };

            let (title, deltas) = match (from, to) {
                (Some(f), Some(t)) => (
                    format!("Changes from Step #{:02} to Step #{:02}", f, t),
                    ShadowEngine::diff_checkpoints(&tx, f, t)?,
                ),
                (Some(f), None) => (
                    format!("Changes from Step #{:02} to Live Workspace", f),
                    ShadowEngine::diff_checkpoint_to_live(&tx, f)?,
                ),
                (None, Some(t)) => (
                    format!("Changes from Baseline #00 to Step #{:02}", t),
                    ShadowEngine::diff_checkpoints(&tx, 0, t)?,
                ),
                (None, None) => (
                    format!("Uncommitted changes since Step #{:02}", tx.active_step()),
                    ShadowEngine::compute_live_deltas(&tx)?,
                ),
            };

            if json {
                println!("{}", serde_json::to_string_pretty(&deltas)?);
                return Ok(());
            }

            if deltas.is_empty() {
                println!("\x1b[32mWorking tree clean (no changes detected for query).\x1b[0m");
            } else {
                println!("\x1b[1m{} ({} files):\x1b[0m", title, deltas.len());
                for delta in &deltas {
                    match delta.kind {
                        ChangeKind::Created => println!("  \x1b[32mA  {}\x1b[0m", delta.relative_path.display()),
                        ChangeKind::Modified => println!("  \x1b[33mM  {}\x1b[0m", delta.relative_path.display()),
                        ChangeKind::Deleted => println!("  \x1b[31mD  {}\x1b[0m", delta.relative_path.display()),
                    }
                }
            }
        }
        Commands::Fork { branch, path } => {
            let mut tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31merror: no active Savepoint transaction found.\x1b[0m");
                    return Ok(());
                }
            };
            let start = Instant::now();
            ShadowEngine::fork_branch(&mut tx, &branch)?;
            println!("\x1b[32m[savepoint]\x1b[0m Forked new branch \x1b[1m'{}'\x1b[0m from Step #{} in \x1b[36m{:?}\x1b[0m", branch, tx.active_step(), start.elapsed());
        }
        Commands::Branches { path } => {
            let tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31merror: no active Savepoint transaction found.\x1b[0m");
                    return Ok(());
                }
            };
            println!("\x1b[1mBranches in Transaction {}\x1b[0m", tx.id);
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
                    eprintln!("\x1b[31merror: no active Savepoint transaction found.\x1b[0m");
                    return Ok(());
                }
            };
            let start = Instant::now();
            let step = ShadowEngine::switch_branch(&mut tx, &branch)?;
            println!("\x1b[32m[savepoint]\x1b[0m Switched to branch \x1b[1m'{}'\x1b[0m (Step #{}) in \x1b[36m{:?}\x1b[0m", branch, step, start.elapsed());
        }
        Commands::Merge { source, target, path } => {
            let mut tx = match ShadowEngine::load_active_transaction(&path)? {
                Some(tx) => tx,
                None => {
                    eprintln!("\x1b[31merror: no active Savepoint transaction found.\x1b[0m");
                    return Ok(());
                }
            };
            let target_branch = target.unwrap_or_else(|| tx.active_branch.clone());
            let start = Instant::now();
            let cp = ShadowEngine::merge_branch(&mut tx, &source, &target_branch)?;
            println!("\x1b[32m[savepoint]\x1b[0m Merged branch \x1b[1m'{}'\x1b[0m into \x1b[1m'{}'\x1b[0m at Checkpoint #{} in \x1b[36m{:?}\x1b[0m", source, target_branch, cp.step_index, start.elapsed());
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
            println!("\x1b[1mSavepoint Speculative Execution\x1b[0m");
            println!("  Candidates evaluated : {}", summary.total_candidates);
            println!("  Elapsed time         : {:?}", start.elapsed());
            match summary.winner_branch {
                Some(winner) => {
                    println!("  Result               : \x1b[32mPASS (winner '{}' promoted to workspace)\x1b[0m", winner);
                }
                None => {
                    println!("  Result               : \x1b[31mFAIL (all candidates failed; workspace unchanged)\x1b[0m");
                }
            }
            for rep in &summary.candidate_reports {
                let status_icon = if rep.passed { "\x1b[32mPASS\x1b[0m" } else { "\x1b[31mFAIL\x1b[0m" };
                println!("    {} [{}] in {:?} ({} files modified)", status_icon, rep.branch_name, Duration::from_millis(rep.duration_ms as u64), rep.modified_files_count);
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
        Commands::Share { output, step, path } => {
            let start = Instant::now();
            let stats = FlightRecorder::export_bundle_with_step(&path, &output, step)?;
            println!(
                "\x1b[32m[savepoint]\x1b[0m Exported Flight Recorder bundle to \x1b[1m{:?}\x1b[0m in \x1b[36m{:?}\x1b[0m",
                stats.output_path,
                start.elapsed()
            );
            println!("  Transaction    : {}", stats.transaction_id);
            println!("  Replay URI     : \x1b[36m{}\x1b[0m", stats.replay_uri);
            if let Some(pinned) = stats.pinned_step {
                println!("  Pinned Step    : #{}", pinned);
            }
            println!("  Checkpoints    : {}", stats.checkpoints_count);
            println!("  Tracked files  : {}", stats.tracked_files_count);
            println!(
                "  Secret Shield  : {} sensitive file(s) excluded, {} inline token(s) redacted",
                stats.scrubbed_files_count, stats.scrubbed_secrets_count
            );
        }
        Commands::Replay { bundle, step, dry_run, path } => {
            if dry_run {
                let flight_bundle = FlightRecorder::inspect_bundle(&bundle)?;
                println!("\x1b[1m[DRY-RUN] Flight Recorder Bundle Inspection\x1b[0m");
                println!("  Transaction ID  : {}", flight_bundle.transaction.id);
                println!("  Created At      : {}", flight_bundle.created_at);
                println!("  Checkpoints     : {}", flight_bundle.transaction.checkpoints.len());
                if let Some(pinned) = flight_bundle.pinned_step {
                    println!("  Pinned Step     : #{}", pinned);
                }
                let target_step = step
                    .or(flight_bundle.pinned_step)
                    .unwrap_or(flight_bundle.transaction.active_step());
                println!("  Target Step     : #{}", target_step);
                if let Some(files) = flight_bundle.step_files.get(&target_step) {
                    println!("  Files at Step #{:02} ({} files):", target_step, files.len());
                    for (rel, state) in files {
                        match state {
                            savepoint_fs::BundleFileState::Present { .. } => {
                                println!("    \x1b[32m+ {}\x1b[0m", rel);
                            }
                            savepoint_fs::BundleFileState::Deleted => {
                                println!("    \x1b[31m- {}\x1b[0m (deleted)", rel);
                            }
                        }
                    }
                }
                println!("\x1b[33m[DRY-RUN] No files were touched in workspace.\x1b[0m");
                return Ok(());
            }

            let start = Instant::now();
            let stats = FlightRecorder::replay_bundle(&bundle, &path, step)?;
            println!(
                "\x1b[32m[savepoint]\x1b[0m Replayed bundle \x1b[1m{}\x1b[0m (Step #{}) into \x1b[1m{:?}\x1b[0m in \x1b[36m{:?}\x1b[0m ({} files restored)",
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
