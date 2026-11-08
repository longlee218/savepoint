use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNode {
    pub step: u64,
    pub parent_step: Option<u64>,
    pub branch: String,
    pub prompt: String,
    pub is_active: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DagGraph {
    pub nodes: BTreeMap<u64, DagNode>,
}

impl DagGraph {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
        }
    }

    pub fn add_node(&mut self, node: DagNode) {
        self.nodes.insert(node.step, node);
    }

    pub fn render_ascii(&self) -> String {
        if self.nodes.is_empty() {
            return "Empty DAG".to_string();
        }

        // Build adjacency: parent -> children
        let mut children_map: HashMap<Option<u64>, Vec<u64>> = HashMap::new();
        for node in self.nodes.values() {
            children_map
                .entry(node.parent_step)
                .or_default()
                .push(node.step);
        }

        // Sort children by step
        for children in children_map.values_mut() {
            children.sort();
        }

        let mut out = String::new();

        // Roots are nodes with parent_step == None or whose parent is not in self.nodes
        let roots: Vec<u64> = self
            .nodes
            .values()
            .filter(|n| n.parent_step.is_none() || !self.nodes.contains_key(&n.parent_step.unwrap()))
            .map(|n| n.step)
            .collect();

        for (i, &root_step) in roots.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            self.render_subtree(root_step, &children_map, "", true, &mut out);
        }

        out
    }

    fn render_subtree(
        &self,
        step: u64,
        children_map: &HashMap<Option<u64>, Vec<u64>>,
        prefix: &str,
        is_root: bool,
        out: &mut String,
    ) {
        let Some(node) = self.nodes.get(&step) else {
            return;
        };

        let active_tag = if node.is_active { " [ACTIVE]" } else { "" };

        if is_root {
            out.push_str(&format!(
                "[*] Step {} [{}]{}: {}\n",
                node.step, node.branch, active_tag, node.prompt
            ));
        } else {
            out.push_str(&format!(
                "{}Step {} [{}]{}: {}\n",
                prefix, node.step, node.branch, active_tag, node.prompt
            ));
        }

        if let Some(children) = children_map.get(&Some(step)) {
            let child_count = children.len();
            for (idx, &child_step) in children.iter().enumerate() {
                let is_last = idx == child_count - 1;
                let branch_symbol = if is_last { "\\--- " } else { "+--- " };
                let child_prefix = if is_root {
                    format!("    {}", branch_symbol)
                } else {
                    let continuation = if let Some(base) = prefix.strip_suffix("\\--- ") {
                        format!("{}     ", base)
                    } else if let Some(base) = prefix.strip_suffix("+--- ") {
                        format!("{}|    ", base)
                    } else {
                        format!("{}    ", prefix)
                    };
                    format!("{}{}", continuation, branch_symbol)
                };

                self.render_subtree(child_step, children_map, &child_prefix, false, out);
            }
        }
    }
}
