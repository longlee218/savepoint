use crate::config::ProviderKind;
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub provider_name: String,
    pub kind: ProviderKind,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub metadata: serde_json::Value,
}

pub trait StateProvider: Send + Sync {
    /// Distinct identifier from `.sp-state.yaml`.
    fn name(&self) -> &str;

    /// Provider classification.
    fn kind(&self) -> ProviderKind;

    /// Verifies provider connectivity and access permissions.
    fn preflight(&self) -> Result<()>;

    /// Captures the state into `snapshot_dir`. Must be non-destructive.
    fn snapshot(&self, snapshot_dir: &Path) -> Result<SnapshotManifest>;

    /// Verifies that the snapshot artifact in `snapshot_dir` is valid and intact before attempting restore.
    fn verify(&self, snapshot_dir: &Path) -> Result<()>;

    /// Restores live state to match `snapshot_dir`.
    fn restore(&self, snapshot_dir: &Path) -> Result<()>;

    /// Cleans up any out-of-tree or external snapshot artifacts (e.g. template databases) if created.
    fn discard(&self, snapshot_dir: &Path) -> Result<()>;
}
