use crate::config::{ProviderKind, ProviderSpec};
use crate::error::{Result, StateError};
use crate::provider::{SnapshotManifest, StateProvider};
use chrono::Utc;
use rusqlite::backup::Backup;
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct SqliteProvider {
    spec: ProviderSpec,
    workspace_root: PathBuf,
}

impl SqliteProvider {
    pub fn new(spec: ProviderSpec, workspace_root: PathBuf) -> Self {
        Self {
            spec,
            workspace_root,
        }
    }

    fn resolve_path(&self) -> PathBuf {
        let p = self.spec.path.as_ref().expect("sqlite provider requires path");
        if p.is_absolute() {
            p.clone()
        } else {
            self.workspace_root.join(p)
        }
    }

    fn cleanup_companions(db_path: &Path) {
        let p_str = db_path.to_string_lossy();
        let wal = PathBuf::from(format!("{}-wal", p_str));
        let shm = PathBuf::from(format!("{}-shm", p_str));
        let _ = fs::remove_file(wal);
        let _ = fs::remove_file(shm);
    }
}

impl StateProvider for SqliteProvider {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Sqlite
    }

    fn preflight(&self) -> Result<()> {
        let db_path = self.resolve_path();
        if let Some(parent) = db_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| StateError::io(parent, e))?;
            }
        }
        if db_path.exists() {
            let conn = Connection::open(&db_path)
                .map_err(|e| StateError::provider(&self.spec.name, "preflight", e))?;
            conn.busy_timeout(Duration::from_secs(5))
                .map_err(|e| StateError::provider(&self.spec.name, "preflight", e))?;
            let _ = conn.execute_batch("PRAGMA schema_version;");
        }
        Ok(())
    }

    fn snapshot(&self, snapshot_dir: &Path) -> Result<SnapshotManifest> {
        let db_path = self.resolve_path();
        fs::create_dir_all(snapshot_dir).map_err(|e| StateError::io(snapshot_dir, e))?;

        if !db_path.exists() {
            let manifest = SnapshotManifest {
                provider_name: self.spec.name.clone(),
                kind: ProviderKind::Sqlite,
                created_at: Utc::now(),
                metadata: serde_json::json!({
                    "exists": false,
                    "rel_path": self.spec.path,
                }),
            };
            let manifest_path = snapshot_dir.join("manifest.json");
            fs::write(
                &manifest_path,
                serde_json::to_vec_pretty(&manifest).unwrap(),
            )
            .map_err(|e| StateError::io(&manifest_path, e))?;
            return Ok(manifest);
        }

        let snap_file = snapshot_dir.join("database.sqlite");
        if snap_file.exists() {
            let _ = fs::remove_file(&snap_file);
        }

        // Use SQLite Online Backup API for consistent point-in-time snapshot
        let src_conn = Connection::open(&db_path)
            .map_err(|e| StateError::provider(&self.spec.name, "snapshot_open_src", e))?;
        src_conn
            .busy_timeout(Duration::from_secs(self.spec.lock_timeout_secs))
            .map_err(|e| StateError::provider(&self.spec.name, "snapshot_busy_timeout", e))?;

        let mut dst_conn = Connection::open(&snap_file)
            .map_err(|e| StateError::provider(&self.spec.name, "snapshot_open_dst", e))?;

        {
            let backup = Backup::new(&src_conn, &mut dst_conn)
                .map_err(|e| StateError::provider(&self.spec.name, "snapshot_backup_init", e))?;
            backup
                .step(-1)
                .map_err(|e| StateError::provider(&self.spec.name, "snapshot_backup_step", e))?;
        }

        drop(dst_conn);
        drop(src_conn);

        let manifest = SnapshotManifest {
            provider_name: self.spec.name.clone(),
            kind: ProviderKind::Sqlite,
            created_at: Utc::now(),
            metadata: serde_json::json!({
                "exists": true,
                "rel_path": self.spec.path,
                "file_size": fs::metadata(&snap_file).map(|m| m.len()).unwrap_or(0),
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

        if manifest.metadata["exists"].as_bool() == Some(true) {
            let snap_file = snapshot_dir.join("database.sqlite");
            if !snap_file.exists() {
                return Err(StateError::MissingSnapshot(snap_file));
            }
            let conn = Connection::open(&snap_file)
                .map_err(|e| StateError::provider(&self.spec.name, "verify_open", e))?;
            let mut stmt = conn
                .prepare("PRAGMA quick_check(1);")
                .map_err(|e| StateError::provider(&self.spec.name, "verify_quick_check", e))?;
            let status: String = stmt
                .query_row([], |r| r.get(0))
                .map_err(|e| StateError::provider(&self.spec.name, "verify_status", e))?;
            if status.to_lowercase() != "ok" {
                return Err(StateError::Corrupt {
                    path: snap_file,
                    msg: format!("quick_check failed: {}", status),
                });
            }
        }
        Ok(())
    }

    fn restore(&self, snapshot_dir: &Path) -> Result<()> {
        let db_path = self.resolve_path();
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

        if manifest.metadata["exists"].as_bool() == Some(false) {
            if db_path.exists() {
                let _ = fs::remove_file(&db_path);
            }
            Self::cleanup_companions(&db_path);
            return Ok(());
        }

        let snap_file = snapshot_dir.join("database.sqlite");
        if !snap_file.exists() {
            return Err(StateError::MissingSnapshot(snap_file));
        }

        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent).map_err(|e| StateError::io(parent, e))?;
        }

        // Try Online Backup restore into live database
        let mut restored_via_backup = false;
        if db_path.exists() {
            if let Ok(mut dst_conn) = Connection::open(&db_path) {
                let _ = dst_conn.busy_timeout(Duration::from_secs(self.spec.lock_timeout_secs));
                if let Ok(src_conn) = Connection::open(&snap_file) {
                    if let Ok(backup) = Backup::new(&src_conn, &mut dst_conn) {
                        if backup.step(-1).is_ok() {
                            restored_via_backup = true;
                        }
                    }
                }
            }
        }

        // If live file did not exist, or backup API failed (e.g. live file was corrupt/not SQLite),
        // fallback to atomic file copy
        if !restored_via_backup {
            Self::cleanup_companions(&db_path);
            let tmp_dst = db_path.with_extension("tmp-restore");
            fs::copy(&snap_file, &tmp_dst).map_err(|e| StateError::io(&tmp_dst, e))?;
            fs::rename(&tmp_dst, &db_path).map_err(|e| StateError::io(&db_path, e))?;
        }

        Ok(())
    }

    fn discard(&self, _snapshot_dir: &Path) -> Result<()> {
        // Local files are automatically cleaned up with the checkpoint directory.
        Ok(())
    }
}
