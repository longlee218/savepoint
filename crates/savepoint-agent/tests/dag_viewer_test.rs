use savepoint_agent::dag::{DagGraph, DagNode};

#[test]
fn test_dag_viewer_renders_multi_agent_branches() {
    let mut graph = DagGraph::new();

    // Root step 0
    graph.add_node(DagNode {
        step: 0,
        parent_step: None,
        branch: "main".to_string(),
        prompt: "Baseline workspace".to_string(),
        is_active: false,
    });

    // Agent Alpha branch (steps 1 & 2)
    graph.add_node(DagNode {
        step: 1,
        parent_step: Some(0),
        branch: "agent_alpha".to_string(),
        prompt: "Added SQLite auth schema".to_string(),
        is_active: false,
    });
    graph.add_node(DagNode {
        step: 2,
        parent_step: Some(1),
        branch: "agent_alpha".to_string(),
        prompt: "Implemented JWT token verify".to_string(),
        is_active: false,
    });

    // Agent Beta branch (steps 3 & 4)
    graph.add_node(DagNode {
        step: 3,
        parent_step: Some(0),
        branch: "agent_beta".to_string(),
        prompt: "Tailwind UI Navbar layout".to_string(),
        is_active: false,
    });
    graph.add_node(DagNode {
        step: 4,
        parent_step: Some(3),
        branch: "agent_beta".to_string(),
        prompt: "Dark mode toggle component".to_string(),
        is_active: true,
    });

    let rendered = graph.render_ascii();
    assert!(rendered.contains("Baseline workspace"));
    assert!(rendered.contains("agent_alpha"));
    assert!(rendered.contains("agent_beta"));
    assert!(rendered.contains("Added SQLite auth schema"));
    assert!(rendered.contains("Dark mode toggle component"));
    assert!(rendered.contains("[ACTIVE]"));
}
