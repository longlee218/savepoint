use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TransactionId(String);

impl TransactionId {
    pub fn new() -> Self {
        let raw = Uuid::new_v4().simple().to_string();
        Self(format!("tx_{}", &raw[..8]))
    }

    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for TransactionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TransactionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionStatus {
    Active,
    Committed,
    RolledBack,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeKind {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDelta {
    pub relative_path: PathBuf,
    pub kind: ChangeKind,
    pub old_size: Option<u64>,
    pub new_size: Option<u64>,
}

fn default_branch() -> String {
    "main".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub step_index: usize,
    pub timestamp: DateTime<Utc>,
    pub snapshot_path: PathBuf,
    pub prompt: Option<String>,
    pub summary: Option<String>,
    pub deltas: Vec<FileDelta>,
    #[serde(default = "default_branch")]
    pub branch: String,
    #[serde(default)]
    pub parent_step: Option<usize>,
    #[serde(default)]
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: TransactionId,
    pub workspace_root: PathBuf,
    pub shadow_root: PathBuf,
    pub created_at: DateTime<Utc>,
    pub status: TransactionStatus,
    pub checkpoints: Vec<Checkpoint>,
    #[serde(default)]
    pub current_step_index: Option<usize>,
    #[serde(default = "default_branch")]
    pub active_branch: String,
    #[serde(default)]
    pub branches: std::collections::HashMap<String, usize>,
}

impl Transaction {
    pub fn new(id: TransactionId, workspace_root: PathBuf, shadow_root: PathBuf) -> Self {
        let mut branches = std::collections::HashMap::new();
        branches.insert("main".to_string(), 0);
        Self {
            id,
            workspace_root,
            shadow_root,
            created_at: Utc::now(),
            status: TransactionStatus::Active,
            checkpoints: Vec::new(),
            current_step_index: None,
            active_branch: "main".to_string(),
            branches,
        }
    }

    pub fn current_step(&self) -> usize {
        self.checkpoints.len()
    }

    pub fn active_step(&self) -> usize {
        self.current_step_index.unwrap_or_else(|| {
            if let Some(&head) = self.branches.get(&self.active_branch) {
                head
            } else {
                self.checkpoints.last().map(|c| c.step_index).unwrap_or(0)
            }
        })
    }

    pub fn active_checkpoint(&self) -> Option<&Checkpoint> {
        let step = self.active_step();
        self.checkpoints
            .iter()
            .find(|c| c.step_index == step)
            .or_else(|| self.checkpoints.last())
    }

    pub fn next_step_index(&self) -> usize {
        if self.checkpoints.is_empty() {
            0
        } else {
            self.checkpoints.iter().map(|c| c.step_index).max().unwrap_or(0) + 1
        }
    }

    pub fn latest_checkpoint(&self) -> Option<&Checkpoint> {
        self.checkpoints.last()
    }

    pub fn branch_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.branches.keys().cloned().collect();
        names.sort();
        if names.is_empty() {
            names.push("main".to_string());
        }
        names
    }

    pub fn branch_head(&self, branch: &str) -> Option<&Checkpoint> {
        self.branches
            .get(branch)
            .and_then(|&step| self.checkpoints.iter().find(|c| c.step_index == step))
    }

    pub fn fork_branch(&mut self, new_branch: &str) -> crate::error::Result<()> {
        if self.branches.contains_key(new_branch) {
            return Err(crate::error::SavepointError::BranchAlreadyExists(new_branch.to_string()));
        }
        let current_step = self.active_step();
        self.branches.insert(new_branch.to_string(), current_step);
        self.active_branch = new_branch.to_string();
        Ok(())
    }

    pub fn switch_branch(&mut self, branch: &str) -> crate::error::Result<usize> {
        let target_step = self
            .branches
            .get(branch)
            .copied()
            .ok_or_else(|| crate::error::SavepointError::BranchNotFound(branch.to_string()))?;
        self.active_branch = branch.to_string();
        self.current_step_index = Some(target_step);
        Ok(target_step)
    }

    pub fn record_checkpoint_in_branch(&mut self, cp: Checkpoint) {
        let step = cp.step_index;
        let branch = cp.branch.clone();
        self.branches.insert(branch.clone(), step);
        self.active_branch = branch;
        self.current_step_index = Some(step);
        self.checkpoints.push(cp);
    }
}
