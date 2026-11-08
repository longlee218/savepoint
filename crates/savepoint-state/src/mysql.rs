use crate::config::{ProviderKind, ProviderSpec};
use crate::error::{Result, StateError};
use crate::provider::{SnapshotManifest, StateProvider};
use chrono::Utc;
use mysql::prelude::*;
use mysql::{Conn, Opts};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

pub struct MysqlProvider {
    spec: ProviderSpec,
}

impl MysqlProvider {
    pub fn new(spec: ProviderSpec) -> Self {
        Self { spec }
    }

    fn quote_ident(ident: &str) -> String {
        format!("`{}`", ident.replace('`', "``"))
    }

    fn parse_opts(&self) -> Result<(Opts, Opts, String)> {
        let url_str = self
            .spec
            .url
            .as_deref()
            .ok_or_else(|| StateError::provider(&self.spec.name, "config", "missing url"))?;

        let opts = Opts::from_url(url_str)
            .map_err(|e| StateError::provider(&self.spec.name, "config", e))?;

        let db_name = opts
            .get_db_name()
            .unwrap_or("")
            .to_string();
        if db_name.is_empty() {
            return Err(StateError::provider(
                &self.spec.name,
                "config",
                "missing database name in url",
            ));
        }

        let root_url = format!(
            "mysql://{}:{}@{}:{}/mysql",
            opts.get_user().unwrap_or("root"),
            opts.get_pass().unwrap_or(""),
            opts.get_ip_or_hostname(),
            opts.get_tcp_port()
        );
        let root_opts = Opts::from_url(&root_url)
            .map_err(|e| StateError::provider(&self.spec.name, "config", e))?;

        Ok((opts, root_opts, db_name))
    }

    fn make_snapshot_db_name(provider_name: &str, snapshot_dir: &Path) -> String {
        let mut hasher = Sha256::new();
        hasher.update(provider_name.as_bytes());
        hasher.update(snapshot_dir.to_string_lossy().as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        format!("sp_snap_{}", &hash[..20])
    }
}

impl StateProvider for MysqlProvider {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Mysql
    }

    fn preflight(&self) -> Result<()> {
        let (_, root_opts, _) = self.parse_opts()?;
        let mut conn = Conn::new(root_opts)
            .map_err(|e| StateError::provider(&self.spec.name, "preflight", e))?;
        let _ = conn.query_drop("SELECT 1;");
        Ok(())
    }

    fn snapshot(&self, snapshot_dir: &Path) -> Result<SnapshotManifest> {
        let (_, root_opts, target_db) = self.parse_opts()?;
        fs::create_dir_all(snapshot_dir).map_err(|e| StateError::io(snapshot_dir, e))?;

        let mut conn = Conn::new(root_opts)
            .map_err(|e| StateError::provider(&self.spec.name, "snapshot_connect", e))?;

        let snap_db = Self::make_snapshot_db_name(&self.spec.name, snapshot_dir);
        let quoted_snap = Self::quote_ident(&snap_db);
        let quoted_target = Self::quote_ident(&target_db);

        // Drop existing snapshot db if leftover
        conn.query_drop(format!("DROP DATABASE IF EXISTS {};", quoted_snap))
            .map_err(|e| StateError::provider(&self.spec.name, "snapshot_drop_existing", e))?;

        // Create snapshot schema
        conn.query_drop(format!(
            "CREATE DATABASE {} CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;",
            quoted_snap
        ))
        .map_err(|e| StateError::provider(&self.spec.name, "snapshot_create_schema", e))?;

        // 1. Snapshot all BASE TABLES
        let tables: Vec<String> = conn
            .query_map(
                format!(
                    "SELECT table_name FROM information_schema.tables WHERE table_schema = '{}' AND table_type = 'BASE TABLE';",
                    target_db.replace('\'', "''")
                ),
                |t: String| t,
            )
            .map_err(|e| StateError::provider(&self.spec.name, "snapshot_list_tables", e))?;

        // Disable FK checks during snapshot copy to avoid order-of-insertion issues
        conn.query_drop("SET FOREIGN_KEY_CHECKS = 0;").unwrap();

        for tbl in &tables {
            let quoted_tbl = Self::quote_ident(tbl);
            conn.query_drop(format!(
                "CREATE TABLE {}.{} LIKE {}.{};",
                quoted_snap, quoted_tbl, quoted_target, quoted_tbl
            ))
            .map_err(|e| StateError::provider(&self.spec.name, "snapshot_clone_tbl", e))?;

            conn.query_drop(format!(
                "INSERT INTO {}.{} SELECT * FROM {}.{};",
                quoted_snap, quoted_tbl, quoted_target, quoted_tbl
            ))
            .map_err(|e| StateError::provider(&self.spec.name, "snapshot_copy_data", e))?;
        }

        // 2. Snapshot Views
        let view_names: Vec<String> = conn
            .query_map(
                format!(
                    "SELECT table_name FROM information_schema.views WHERE table_schema = '{}';",
                    target_db.replace('\'', "''")
                ),
                |v: String| v,
            )
            .unwrap_or_default();

        let mut view_ddls = Vec::new();
        for v in view_names {
            let quoted_v = Self::quote_ident(&v);
            let row: Option<mysql::Row> = conn
                .query_first(format!("SHOW CREATE VIEW {}.{};", quoted_target, quoted_v))
                .unwrap_or(None);
            if let Some(r) = row {
                if let Some(ddl) = r.get::<String, _>(1) {
                    view_ddls.push((v, ddl));
                }
            }
        }

        // 3. Snapshot Triggers
        let trigger_names: Vec<String> = conn
            .query_map(
                format!(
                    "SELECT trigger_name FROM information_schema.triggers WHERE trigger_schema = '{}';",
                    target_db.replace('\'', "''")
                ),
                |t: String| t,
            )
            .unwrap_or_default();

        let mut trigger_ddls = Vec::new();
        for trg in trigger_names {
            let row: Option<mysql::Row> = conn
                .query_first(format!("SHOW CREATE TRIGGER {}.{};", quoted_target, Self::quote_ident(&trg)))
                .unwrap_or(None);
            if let Some(r) = row {
                // Column 2 is "SQL Original Statement"
                if let Some(ddl) = r.get::<String, _>(2) {
                    trigger_ddls.push((trg, ddl));
                }
            }
        }

        conn.query_drop("SET FOREIGN_KEY_CHECKS = 1;").unwrap();

        let manifest = SnapshotManifest {
            provider_name: self.spec.name.clone(),
            kind: ProviderKind::Mysql,
            created_at: Utc::now(),
            metadata: serde_json::json!({
                "snapshot_db": snap_db,
                "target_db": target_db,
                "tables": tables,
                "views": view_ddls,
                "triggers": trigger_ddls,
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

        let (_, root_opts, _) = self.parse_opts()?;
        let mut conn = Conn::new(root_opts)
            .map_err(|e| StateError::provider(&self.spec.name, "verify_connect", e))?;

        let exists: Option<String> = conn
            .query_first(format!(
                "SELECT schema_name FROM information_schema.schemata WHERE schema_name = '{}';",
                snap_db.replace('\'', "''")
            ))
            .map_err(|e| StateError::provider(&self.spec.name, "verify_query", e))?;

        if exists.is_none() {
            return Err(StateError::provider(
                &self.spec.name,
                "verify",
                format!("snapshot database {:?} not found", snap_db),
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

        let snap_db = manifest.metadata["snapshot_db"].as_str().unwrap();
        let target_db = manifest.metadata["target_db"].as_str().unwrap();

        let (_, root_opts, _) = self.parse_opts()?;
        let mut conn = Conn::new(root_opts)
            .map_err(|e| StateError::provider(&self.spec.name, "restore_connect", e))?;

        let quoted_snap = Self::quote_ident(snap_db);
        let quoted_target = Self::quote_ident(target_db);

        conn.query_drop("SET FOREIGN_KEY_CHECKS = 0;").unwrap();

        // Drop and recreate target database
        conn.query_drop(format!("DROP DATABASE IF EXISTS {};", quoted_target))
            .map_err(|e| StateError::provider(&self.spec.name, "restore_drop", e))?;

        conn.query_drop(format!(
            "CREATE DATABASE {} CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;",
            quoted_target
        ))
        .map_err(|e| StateError::provider(&self.spec.name, "restore_recreate", e))?;

        // 1. Restore Tables and Data
        if let Some(tables) = manifest.metadata["tables"].as_array() {
            for t_val in tables {
                let tbl = t_val.as_str().unwrap();
                let quoted_tbl = Self::quote_ident(tbl);
                conn.query_drop(format!(
                    "CREATE TABLE {}.{} LIKE {}.{};",
                    quoted_target, quoted_tbl, quoted_snap, quoted_tbl
                ))
                .map_err(|e| StateError::provider(&self.spec.name, "restore_create_tbl", e))?;

                conn.query_drop(format!(
                    "INSERT INTO {}.{} SELECT * FROM {}.{};",
                    quoted_target, quoted_tbl, quoted_snap, quoted_tbl
                ))
                .map_err(|e| StateError::provider(&self.spec.name, "restore_insert_data", e))?;
            }
        }

        // Switch default schema to target_db for views/triggers execution
        conn.query_drop(format!("USE {};", quoted_target)).unwrap();

        // 2. Restore Views
        if let Some(views) = manifest.metadata["views"].as_array() {
            for v_pair in views {
                if let Some(arr) = v_pair.as_array() {
                    let ddl = arr[1].as_str().unwrap();
                    let _ = conn.query_drop(ddl);
                }
            }
        }

        // 3. Restore Triggers (Recreated AFTER data is restored so triggers don't double fire!)
        if let Some(triggers) = manifest.metadata["triggers"].as_array() {
            for trg_pair in triggers {
                if let Some(arr) = trg_pair.as_array() {
                    let ddl = arr[1].as_str().unwrap();
                    let _ = conn.query_drop(ddl);
                }
            }
        }

        conn.query_drop("SET FOREIGN_KEY_CHECKS = 1;").unwrap();

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
            let (_, root_opts, _) = self.parse_opts()?;
            if let Ok(mut conn) = Conn::new(root_opts) {
                let quoted_snap = Self::quote_ident(snap_db);
                let _ = conn.query_drop(format!("DROP DATABASE IF EXISTS {};", quoted_snap));
            }
        }

        Ok(())
    }
}
