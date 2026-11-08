use savepoint_agent::{AgentError, AgentLeaseManager, DagGraph, DagNode, LeaseConfig};
use tempfile::tempdir;

#[test]
fn test_agent_lease_port_exhaustion_error() {
    let dir = tempdir().unwrap();
    let root = dir.path().join(".sp_leases");

    // Configure max_agents = 2
    let config = LeaseConfig {
        max_agents: 2,
        ..Default::default()
    };

    let manager = AgentLeaseManager::new(&root, config);
    let l1 = manager.acquire("agent_1").unwrap();
    let l2 = manager.acquire("agent_2").unwrap();
    assert_ne!(l1.port_offset, l2.port_offset);

    // 3rd acquisition must fail with PortExhaustion
    let err = manager.acquire("agent_3").unwrap_err();
    match err {
        AgentError::PortExhaustion(max) => assert_eq!(max, 2),
        other => panic!("Expected PortExhaustion, got {:?}", other),
    }

    // Release agent_1 -> now agent_3 can acquire!
    manager.release("agent_1").unwrap();
    let l3 = manager.acquire("agent_3").unwrap();
    assert_eq!(l3.port_offset, l1.port_offset);
}

#[test]
fn test_agent_lease_not_found_on_release() {
    let dir = tempdir().unwrap();
    let manager = AgentLeaseManager::new(dir.path(), LeaseConfig::default());

    let err = manager.release("ghost_agent").unwrap_err();
    match err {
        AgentError::LeaseNotFound(id) => assert_eq!(id, "ghost_agent"),
        other => panic!("Expected LeaseNotFound, got {:?}", other),
    }
}

#[test]
fn test_agent_lease_get_and_persistence() {
    let dir = tempdir().unwrap();
    let manager = AgentLeaseManager::new(dir.path(), LeaseConfig::default());

    let created = manager.acquire("agent_worker").unwrap();
    let fetched = manager.get_lease("agent_worker").unwrap();

    assert_eq!(created.agent_id, fetched.agent_id);
    assert_eq!(created.port_offset, fetched.port_offset);
    assert_eq!(created.db_namespace, fetched.db_namespace);
    assert_eq!(created.cache_prefix, fetched.cache_prefix);

    // Non-existent get
    assert!(manager.get_lease("missing").is_err());
}

#[test]
fn test_dag_empty_graph() {
    let graph = DagGraph::new();
    assert_eq!(graph.render_ascii(), "Empty DAG");
}

#[test]
fn test_dag_single_node() {
    let mut graph = DagGraph::new();
    graph.add_node(DagNode {
        step: 0,
        parent_step: None,
        branch: "main".to_string(),
        prompt: "Single root".to_string(),
        is_active: true,
    });
    let rendered = graph.render_ascii();
    assert!(rendered.contains("[*] Step 0 [main] [ACTIVE]: Single root"));
}

#[test]
fn test_dag_deep_linear_and_orphan_handling() {
    let mut graph = DagGraph::new();
    // Chain: 0 -> 1 -> 2
    graph.add_node(DagNode {
        step: 0,
        parent_step: None,
        branch: "main".to_string(),
        prompt: "Root".to_string(),
        is_active: false,
    });
    graph.add_node(DagNode {
        step: 1,
        parent_step: Some(0),
        branch: "main".to_string(),
        prompt: "Step 1".to_string(),
        is_active: false,
    });
    graph.add_node(DagNode {
        step: 2,
        parent_step: Some(1),
        branch: "main".to_string(),
        prompt: "Step 2".to_string(),
        is_active: true,
    });

    // Orphan node whose parent 99 does not exist
    graph.add_node(DagNode {
        step: 100,
        parent_step: Some(99),
        branch: "orphan_branch".to_string(),
        prompt: "Divergent detached commit".to_string(),
        is_active: false,
    });

    let rendered = graph.render_ascii();
    assert!(rendered.contains("Step 0"));
    assert!(rendered.contains("Step 1"));
    assert!(rendered.contains("Step 2"));
    assert!(rendered.contains("Divergent detached commit"));
}
