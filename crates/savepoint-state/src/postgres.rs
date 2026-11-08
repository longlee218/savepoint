use crate::config::{ProviderKind, ProviderSpec};
use crate::error::{Result, StateError};
use crate::provider::{SnapshotManifest, StateProvider};
use chrono::Utc;
use postgres::{Client, NoTls};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

pub struct PostgresProvider {
    spec: ProviderSpec,
}

impl PostgresProvider {
    pub fn new(spec: ProviderSpec) -> Self {
        Self { spec }
    }

    fn quote_ident(ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }

    fn parse_url(&self) -> Result<(String, String, String)> {
        let url_str = self
            .spec
            .url
            .as_deref()
            .ok_or_else(|| StateError::provider(&self.spec.name, "config", "missing url"))?;

        let mut parts: Vec<&str> = url_str.split('/').collect();
        let target_db = parts
            .pop()
            .unwrap_or("")
            .split('?')
            .next()
            .unwrap_or("")
            .to_string();
        if target_db.is_empty() {
            return Err(StateError::provider(
                &self.spec.name,
                "config",
                "url missing database name",
            ));
        }

        let maint_db = self
            .spec
            .maintenance_db
            .as_deref()
            .unwrap_or("postgres")
            .to_string();

        let base_no_db = parts.join("/");
        let maint_url = format!("{}/{}", base_no_db, maint_db);

        Ok((url_str.to_string(), maint_url, target_db))
    }

    fn make_snapshot_db_name(provider_name: &str, snapshot_dir: &Path) -> String {
        let mut hasher = Sha256::new();
        hasher.update(provider_name.as_bytes());
        hasher.update(snapshot_dir.to_string_lossy().as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        format!("sp_snap_{}", &hash[..20])
    }

    fn terminate_db_connections(client: &mut Client, db_name: &str) -> Result<()> {
        let query = "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = $1 AND pid <> pg_backend_pid();";
        let _ = client.execute(query, &[&db_name]);
        Ok(())
    }
}

impl StateProvider for PostgresProvider {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Postgres
    }

    fn preflight(&self) -> Result<()> {
        let (_, maint_url, _) = self.parse_url()?;
        let mut client = Client::connect(&maint_url, NoTls)
            .map_err(|e| StateError::provider(&self.spec.name, "preflight", e))?;
        client
            .execute("SELECT 1;", &[])
            .map_err(|e| StateError::provider(&self.spec.name, "preflight", e))?;
        Ok(())
    }

    fn snapshot(&self, snapshot_dir: &Path) -> Result<SnapshotManifest> {
        let (_, maint_url, target_db) = self.parse_url()?;
        fs::create_dir_all(snapshot_dir).map_err(|e| StateError::io(snapshot_dir, e))?;

        let mut client = Client::connect(&maint_url, NoTls)
            .map_err(|e| StateError::provider(&self.spec.name, "snapshot_connect", e))?;

        // Check active connections if termination not allowed
        if !self.spec.terminate_connections {
            let count: i64 = client
                .query_one(
                    "SELECT count(*) FROM pg_stat_activity WHERE datname = $1 AND pid <> pg_backend_pid();",
                    &[&target_db],
                )
                .map_err(|e| StateError::provider(&self.spec.name, "check_connections", e))?
                .get(0);

            if count > 0 {
                return Err(StateError::provider(
                    &self.spec.name,
                    "snapshot",
                    format!(
                        "cannot snapshot: {} active connection(s) on database {:?} and terminate_connections is false",
                        count, target_db
                    ),
                ));
            }
        }

        let snap_db = Self::make_snapshot_db_name(&self.spec.name, snapshot_dir);
        let quoted_snap = Self::quote_ident(&snap_db);
        let quoted_target = Self::quote_ident(&target_db);

        // Drop any stale snapshot DB with the same name if leftover
        let _ = client.execute(&format!("DROP DATABASE IF EXISTS {} WITH (FORCE);", quoted_snap), &[]);

        // Try creating template DB, retrying if concurrent sessions briefly reconnected
        let mut last_err = None;
        let mut success = false;
        for attempt in 0..10 {
            if self.spec.terminate_connections {
                let _ = Self::terminate_db_connections(&mut client, &target_db);
            }
            let sql = format!(
                "CREATE DATABASE {} WITH TEMPLATE {} ALLOW_CONNECTIONS false;",
                quoted_snap, quoted_target
            );
            match client.execute(&sql, &[]) {
                Ok(_) => {
                    success = true;
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    thread::sleep(Duration::from_millis(50 * (attempt + 1)));
                }
            }
        }

        if !success {
            let err_msg = last_err
                .map(|e| e.to_string())
                .unwrap_or_else(|| "unknown error".to_string());
            return Err(StateError::provider(
                &self.spec.name,
                "snapshot_create_template",
                format!("failed to create template database: {}", err_msg),
            ));
        }

        let manifest = SnapshotManifest {
            provider_name: self.spec.name.clone(),
            kind: ProviderKind::Postgres,
            created_at: Utc::now(),
            metadata: serde_json::json!({
                "snapshot_db": snap_db,
                "source_db": target_db,
            }),
        };

        let manifest_path = snapshot_dir.join("manifest.json");
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .map_err(|e| StateError::io(&manifest_path, e))?;

        Ok(manifest)
    }

    fn verify(&self, snapshot_dir: &Path) -> Result<()> {
        let manifest_path = snapshot_dir.join("manifest.json");
        if !manifest_path.exists() {
            return Err(StateError::MissingSnapshot(snapshot_dir.to_path_buf()));
        }
        let manifest_data = fs::read(&manifest_path).map_err(|e| StateError::io(&manifest_path, e))?;
        let manifest: SnapshotManifest = serde_json::from_slice(&manifest_data)
            .map_err(|e| StateError::Corrupt {
                path: manifest_path,
                msg: e.to_string(),
            })?;

        let snap_db = manifest.metadata["snapshot_db"].as_str().ok_or_else(|| {
            StateError::Corrupt {
                path: snapshot_dir.to_path_buf(),
                msg: "missing snapshot_db in metadata".to_string(),
            }
        })?;

        let (_, maint_url, _) = self.parse_url()?;
        let mut client = Client::connect(&maint_url, NoTls)
            .map_err(|e| StateError::provider(&self.spec.name, "verify_connect", e))?;

        let exists: bool = client
            .query_one(
                "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname = $1);",
                &[&snap_db],
            )
            .map_err(|e| StateError::provider(&self.spec.name, "verify_query", e))?
            .get(0);

        if !exists {
            return Err(StateError::provider(
                &self.spec.name,
                "verify",
                format!("snapshot database {:?} does not exist", snap_db),
            ));
        }

        Ok(())
    }

    fn restore(&self, snapshot_dir: &Path) -> Result<()> {
        let manifest_path = snapshot_dir.join("manifest.json");
        if !manifest_path.exists() {
            return Err(StateError::MissingSnapshot(snapshot_dir.to_path_buf()));
        }
        let manifest_data = fs::read(&manifest_path).map_err(|e| StateError::io(&manifest_path, e))?;
        let manifest: SnapshotManifest = serde_json::from_slice(&manifest_data)
            .map_err(|e| StateError::Corrupt {
                path: manifest_path,
                msg: e.to_string(),
            })?;

        let snap_db = manifest.metadata["snapshot_db"].as_str().ok_or_else(|| {
            StateError::Corrupt {
                path: snapshot_dir.to_path_buf(),
                msg: "missing snapshot_db in metadata".to_string(),
            }
        })?;

        let (_, maint_url, target_db) = self.parse_url()?;
        let mut client = Client::connect(&maint_url, NoTls)
            .map_err(|e| StateError::provider(&self.spec.name, "restore_connect", e))?;

        let quoted_snap = Self::quote_ident(snap_db);
        let quoted_target = Self::quote_ident(&target_db);

        // Terminate any active connections on live target database
        let _ = Self::terminate_db_connections(&mut client, &target_db);

        // Drop target database
        client
            .execute(&format!("DROP DATABASE IF EXISTS {} WITH (FORCE);", quoted_target), &[])
            .map_err(|e| StateError::provider(&self.spec.name, "restore_drop_target", e))?;

        // Recreate target database from snapshot template
        client
            .execute(
                &format!("CREATE DATABASE {} WITH TEMPLATE {};", quoted_target, quoted_snap),
                &[],
            )
            .map_err(|e| StateError::provider(&self.spec.name, "restore_create_from_template", e))?;

        Ok(())
    }

    fn discard(&self, snapshot_dir: &Path) -> Result<()> {
        let manifest_path = snapshot_dir.join("manifest.json");
        if !manifest_path.exists() {
            return Ok(());
        }
        let manifest_data = match fs::read(&manifest_path) {
            Ok(d) => d,
            Err(_) => return Ok(()),
        };
        let manifest: SnapshotManifest = match serde_json::from_slice(&manifest_data) {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };

        if let Some(snap_db) = manifest.metadata["snapshot_db"].as_str() {
            let (_, maint_url, _) = self.parse_url()?;
            if let Ok(mut client) = Client::connect(&maint_url, NoTls) {
                let quoted_snap = Self::quote_ident(snap_db);
                let _ = client.execute(&format!("DROP DATABASE IF EXISTS {} WITH (FORCE);", quoted_snap), &[]);
            }
        }

        Ok(())
    }
}
