use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Instant;
use savepoint_core::ChangeKind;
use savepoint_fs::ShadowEngine;

#[derive(Parser)]
#[command(name = "savepoint", aliases = ["sp"], author, version = "0.1.0", about = "The ACID Transaction & Time-Travel Runtime for AI Agents")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a new ACID transaction for the current workspace
    Begin {
        #[arg(short, long, default_value = ".")]
        path: PathBuf,
    },
    /// Save a checkpoint after an agent turn or manual change
    Checkpoint {
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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Begin { path } => {
            let start = Instant::now();
            let tx = ShadowEngine::begin_transaction(&path)?;
            let elapsed = start.elapsed();
            println!("\x1b[32m✔ [Savepoint]\x1b[0m Transaction \x1b[1m{}\x1b[0m started in \x1b[36m{:?}\x1b[0m", tx.id, elapsed);
            println!("  Workspace : {:?}", tx.workspace_root);
            println!("  Storage   : {:?}", tx.shadow_root);
            println!("  Baseline  : Step #0 captured via APFS Copy-on-Write.");
        }
        Commands::Checkpoint { message, path } => {
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
            println!("Workspace : {:?}", tx.workspace_root);
            println!("Timeline  : {} checkpoints", tx.checkpoints.len());

            for cp in &tx.checkpoints {
                println!(
                    "  \x1b[36m[Step #{}]\x1b[0m {} | {} deltas",
                    cp.step_index,
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

            let start = Instant::now();
            ShadowEngine::rewind(&mut tx, to)?;
            let elapsed = start.elapsed();

            println!("\x1b[32m✔ [Savepoint]\x1b[0m ⏪ Rewound to Step \x1b[1m#{}\x1b[0m in \x1b[36m{:?}\x1b[0m", to, elapsed);
            println!("  Current step is now #{}.", tx.current_step() - 1);
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

            if let Some(latest) = tx.checkpoints.last() {
                println!("\x1b[1m=== File Deltas for Checkpoint #{} ===\x1b[0m", latest.step_index);
                for delta in &latest.deltas {
                    match delta.kind {
                        ChangeKind::Created => println!("  \x1b[32m+ [Created]\x1b[0m  {}", delta.relative_path.display()),
                        ChangeKind::Modified => println!("  \x1b[33m~ [Modified]\x1b[0m {}", delta.relative_path.display()),
                        ChangeKind::Deleted => println!("  \x1b[31m- [Deleted]\x1b[0m  {}", delta.relative_path.display()),
                    }
                }
            } else {
                println!("No checkpoints recorded yet.");
            }
        }
    }

    Ok(())
}
