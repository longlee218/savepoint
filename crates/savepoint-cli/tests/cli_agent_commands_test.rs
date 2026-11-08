use clap::Parser;
use savepoint_cli::{Cli, Commands, LeaseCommands};
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn test_cli_parse_graph_and_lease_commands() {
    // 1. Parse 'sp graph --path /tmp/test'
    let args = vec!["sp", "graph", "--path", "/tmp/test"];
    let cli = Cli::try_parse_from(args).expect("Failed to parse sp graph");
    match cli.command {
        Commands::Graph { path } => {
            assert_eq!(path, PathBuf::from("/tmp/test"));
        }
        _ => panic!("Expected Commands::Graph"),
    }

    // 2. Parse 'sp lease acquire agent_gamma'
    let args = vec!["sp", "lease", "acquire", "agent_gamma"];
    let cli = Cli::try_parse_from(args).expect("Failed to parse sp lease acquire");
    match cli.command {
        Commands::Lease { subcommand, path } => {
            assert_eq!(path, PathBuf::from("."));
            match subcommand {
                LeaseCommands::Acquire { agent_id } => {
                    assert_eq!(agent_id, "agent_gamma");
                }
                _ => panic!("Expected LeaseCommands::Acquire"),
            }
        }
        _ => panic!("Expected Commands::Lease"),
    }

    // 3. Parse 'sp lease ls'
    let args = vec!["sp", "lease", "ls"];
    let cli = Cli::try_parse_from(args).expect("Failed to parse sp lease ls");
    match cli.command {
        Commands::Lease { subcommand, .. } => match subcommand {
            LeaseCommands::List => (),
            _ => panic!("Expected LeaseCommands::List"),
        },
        _ => panic!("Expected Commands::Lease"),
    }
}

#[test]
fn test_cli_lease_lifecycle_filesystem_integration() {
    let dir = tempdir().unwrap();
    let lease_dir = dir.path().join(".sp").join("leases");
    let manager = savepoint_agent::AgentLeaseManager::new(
        &lease_dir,
        savepoint_agent::LeaseConfig::default(),
    );

    // Acquire
    let lease = manager.acquire("agent_delta").unwrap();
    assert_eq!(lease.agent_id, "agent_delta");
    assert_eq!(lease.port_offset, 3100);
    assert!(lease_dir.join("agent_delta.json").exists());

    // List
    let active = manager.list_active();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].agent_id, "agent_delta");

    // Release
    manager.release("agent_delta").unwrap();
    assert!(!lease_dir.join("agent_delta.json").exists());
    assert_eq!(manager.list_active().len(), 0);
}
