use crate::config::{ProviderKind, StateConfig};
use crate::error::{Result, StateError};
use crate::memcached::MemcachedProvider;
use crate::mysql::MysqlProvider;
use crate::postgres::PostgresProvider;
use crate::provider::{SnapshotManifest, StateProvider};
use crate::redis::RedisProvider;
use crate::sqlite::SqliteProvider;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct StateManager {
    workspace_root: PathBuf,
    providers: Vec<Box<dyn StateProvider>>,
}

impl StateManager {
    pub fn new(workspace_root: PathBuf, providers: Vec<Box<dyn StateProvider>>) -> Self {
        Self {
            workspace_root,
            providers,
        }
    }

    pub fn load(workspace_root: &Path) -> Result<Option<Self>> {
        let config_opt = StateConfig::load(workspace_root)?;
        let Some(config) = config_opt else {
            return Ok(None);
        };

        let mut providers: Vec<Box<dyn StateProvider>> = Vec::new();
        for spec in config.providers {
            match spec.kind {
                ProviderKind::Sqlite => {
                    providers.push(Box::new(SqliteProvider::new(
                        spec,
                        workspace_root.to_path_buf(),
                    )));
                }
                ProviderKind::Postgres => {
                    providers.push(Box::new(PostgresProvider::new(spec)));
                }
                ProviderKind::Mysql => {
                    providers.push(Box::new(MysqlProvider::new(spec)));
                }
                ProviderKind::Redis => {
                    providers.push(Box::new(RedisProvider::new(spec)));
                }
                ProviderKind::Memcached => {
                    providers.push(Box::new(MemcachedProvider::new(spec)));
                }
            }
        }

        Ok(Some(Self {
            workspace_root: workspace_root.to_path_buf(),
            providers,
        }))
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn providers(&self) -> &[Box<dyn StateProvider>] {
        &self.providers
    }

    pub fn preflight_all(&self) -> Result<()> {
        for p in &self.providers {
            p.preflight()?;
        }
        Ok(())
    }

    /// All-or-nothing snapshot: If any provider fails, all preceding snapshots are discarded.
    pub fn snapshot_all(
        &self,
        checkpoint_state_dir: &Path,
    ) -> Result<HashMap<String, SnapshotManifest>> {
        fs::create_dir_all(checkpoint_state_dir)
            .map_err(|e| StateError::io(checkpoint_state_dir, e))?;

        let mut manifests = HashMap::new();
        let mut completed: Vec<(&Box<dyn StateProvider>, PathBuf)> = Vec::new();

        for p in &self.providers {
            let p_dir = checkpoint_state_dir.join(p.name());
            match p.snapshot(&p_dir) {
                Ok(m) => {
                    manifests.insert(p.name().to_string(), m);
                    completed.push((p, p_dir));
                }
                Err(e) => {
                    // Rollback snapshots taken so far during this turn
                    for (prev_p, prev_dir) in completed {
                        let _ = prev_p.discard(&prev_dir);
                    }
                    let _ = fs::remove_dir_all(checkpoint_state_dir);
                    return Err(e);
                }
            }
        }

        Ok(manifests)
    }

    pub fn verify_all(&self, checkpoint_state_dir: &Path) -> Result<()> {
        if !checkpoint_state_dir.exists() {
            return Err(StateError::MissingSnapshot(
                checkpoint_state_dir.to_path_buf(),
            ));
        }
        for p in &self.providers {
            let p_dir = checkpoint_state_dir.join(p.name());
            p.verify(&p_dir)?;
        }
        Ok(())
    }

    pub fn restore_all(&self, checkpoint_state_dir: &Path) -> Result<()> {
        // Step 1: Verify all providers before touching live state
        self.verify_all(checkpoint_state_dir)?;

        // Step 2: Restore all
        let mut restored = Vec::new();
        let mut failures = Vec::new();

        for p in &self.providers {
            let p_dir = checkpoint_state_dir.join(p.name());
            match p.restore(&p_dir) {
                Ok(_) => restored.push(p.name().to_string()),
                Err(e) => failures.push((p.name().to_string(), e.to_string())),
            }
        }

        if !failures.is_empty() {
            return Err(StateError::Partial { restored, failures });
        }

        Ok(())
    }

    pub fn discard_all(&self, checkpoint_state_dir: &Path) -> Result<()> {
        if !checkpoint_state_dir.exists() {
            return Ok(());
        }
        for p in &self.providers {
            let p_dir = checkpoint_state_dir.join(p.name());
            let _ = p.discard(&p_dir);
        }
        let _ = fs::remove_dir_all(checkpoint_state_dir);
        Ok(())
    }
}
