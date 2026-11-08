use clap::Parser;
use savepoint_cli::{Cli, Commands, StateCommands};
use std::path::PathBuf;

#[test]
fn test_cli_parse_core_lifecycle_commands() {
    // 1. sp begin --path /my/ws
    let cli = Cli::try_parse_from(["sp", "begin", "--path", "/my/ws"]).unwrap();
    match cli.command {
        Commands::Begin { path } => assert_eq!(path, PathBuf::from("/my/ws")),
        _ => panic!("Expected Commands::Begin"),
    }

    // 2. sp save -m "Checkpoint A"
    let cli = Cli::try_parse_from(["sp", "save", "-m", "Checkpoint A"]).unwrap();
    match cli.command {
        Commands::Save { message, .. } => assert_eq!(message, Some("Checkpoint A".to_string())),
        _ => panic!("Expected Commands::Save"),
    }

    // 3. sp status --json
    let cli = Cli::try_parse_from(["sp", "status", "--json"]).unwrap();
    match cli.command {
        Commands::Status { json, .. } => assert!(json),
        _ => panic!("Expected Commands::Status"),
    }

    // 4. sp undo
    let cli = Cli::try_parse_from(["sp", "undo"]).unwrap();
    match cli.command {
        Commands::Undo { .. } => (),
        _ => panic!("Expected Commands::Undo"),
    }

    // 5. sp rewind --to 2
    let cli = Cli::try_parse_from(["sp", "rewind", "--to", "2"]).unwrap();
    match cli.command {
        Commands::Rewind { to, .. } => assert_eq!(to, 2),
        _ => panic!("Expected Commands::Rewind"),
    }

    // 6. sp commit
    let cli = Cli::try_parse_from(["sp", "commit"]).unwrap();
    match cli.command {
        Commands::Commit { .. } => (),
        _ => panic!("Expected Commands::Commit"),
    }
}

#[test]
fn test_cli_parse_state_commands() {
    // 1. sp state ls
    let cli = Cli::try_parse_from(["sp", "state", "ls"]).unwrap();
    match cli.command {
        Commands::State { subcommand, .. } => match subcommand {
            StateCommands::List => (),
            _ => panic!("Expected StateCommands::List"),
        },
        _ => panic!("Expected Commands::State"),
    }

    // 2. sp state check
    let cli = Cli::try_parse_from(["sp", "state", "check"]).unwrap();
    match cli.command {
        Commands::State { subcommand, .. } => match subcommand {
            StateCommands::Check => (),
            _ => panic!("Expected StateCommands::Check"),
        },
        _ => panic!("Expected Commands::State"),
    }
}

#[test]
fn test_cli_parse_branch_and_diff_commands() {
    // 1. sp fork feature-x
    let cli = Cli::try_parse_from(["sp", "fork", "feature-x"]).unwrap();
    match cli.command {
        Commands::Fork { branch, .. } => assert_eq!(branch, "feature-x"),
        _ => panic!("Expected Commands::Fork"),
    }

    // 2. sp switch feature-x
    let cli = Cli::try_parse_from(["sp", "switch", "feature-x"]).unwrap();
    match cli.command {
        Commands::Switch { branch, .. } => assert_eq!(branch, "feature-x"),
        _ => panic!("Expected Commands::Switch"),
    }

    // 3. sp diff --from 1 --to 2 --json
    let cli = Cli::try_parse_from(["sp", "diff", "--from", "1", "--to", "2", "--json"]).unwrap();
    match cli.command {
        Commands::Diff { from, to, json, .. } => {
            assert_eq!(from, Some(1));
            assert_eq!(to, Some(2));
            assert!(json);
        }
        _ => panic!("Expected Commands::Diff"),
    }

    // 4. sp gc --all
    let cli = Cli::try_parse_from(["sp", "gc", "--all"]).unwrap();
    match cli.command {
        Commands::Gc { all, path } => {
            assert!(all);
            assert_eq!(path, PathBuf::from("."));
        }
        _ => panic!("Expected Commands::Gc"),
    }

    // 5. sp clean alias
    let cli = Cli::try_parse_from(["sp", "clean"]).unwrap();
    match cli.command {
        Commands::Gc { all, .. } => assert!(!all),
        _ => panic!("Expected Commands::Gc via clean alias"),
    }
}
