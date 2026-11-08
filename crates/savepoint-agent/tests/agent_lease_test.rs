use savepoint_agent::lease::{AgentLeaseManager, LeaseConfig};
use tempfile::tempdir;

#[test]
fn test_agent_lease_manager_allocates_isolated_environments() {
    let dir = tempdir().unwrap();
    let root = dir.path().join(".sp_leases");
    let manager = AgentLeaseManager::new(&root, LeaseConfig::default());

    // 1. Acquire lease for Agent Alpha
    let lease_alpha = manager.acquire("agent_alpha").unwrap();
    assert_eq!(lease_alpha.agent_id, "agent_alpha");
    assert_eq!(lease_alpha.port_offset, 3100);
    assert_eq!(lease_alpha.db_namespace, "app_agent_alpha");
    assert_eq!(lease_alpha.cache_prefix, "sp_agent_alpha:");
    assert!(lease_alpha.sandbox_dir.exists());

    // 2. Acquire lease for Agent Beta
    let lease_beta = manager.acquire("agent_beta").unwrap();
    assert_eq!(lease_beta.agent_id, "agent_beta");
    assert_eq!(lease_beta.port_offset, 3200);
    assert_ne!(lease_alpha.port_offset, lease_beta.port_offset);
    assert_ne!(lease_alpha.sandbox_dir, lease_beta.sandbox_dir);

    // 3. Duplicate acquisition without release should fail
    assert!(manager.acquire("agent_alpha").is_err());

    // 4. Release Agent Alpha lease
    manager.release("agent_alpha").unwrap();
    assert_eq!(manager.list_active().len(), 1);

    // 5. Re-acquire after release should succeed
    let lease_alpha_re = manager.acquire("agent_alpha").unwrap();
    assert_eq!(lease_alpha_re.agent_id, "agent_alpha");
}
