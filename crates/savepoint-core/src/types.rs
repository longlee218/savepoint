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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub step_index: usize,
    pub timestamp: DateTime<Utc>,
    pub snapshot_path: PathBuf,
    pub prompt: Option<String>,
    pub summary: Option<String>,
    pub deltas: Vec<FileDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transaction {
    pub id: TransactionId,
    pub workspace_root: PathBuf,
    pub shadow_root: PathBuf,
    pub created_at: DateTime<Utc>,
    pub status: TransactionStatus,
    pub checkpoints: Vec<Checkpoint>,
}

impl Transaction {
    pub fn new(id: TransactionId, workspace_root: PathBuf, shadow_root: PathBuf) -> Self {
        Self {
            id,
            workspace_root,
            shadow_root,
            created_at: Utc::now(),
            status: TransactionStatus::Active,
            checkpoints: Vec::new(),
        }
    }

    pub fn current_step(&self) -> usize {
        self.checkpoints.len()
    }

    pub fn latest_checkpoint(&self) -> Option<&Checkpoint> {
        self.checkpoints.last()
    }
}
