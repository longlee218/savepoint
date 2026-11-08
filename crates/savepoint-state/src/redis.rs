use crate::config::{ProviderKind, ProviderSpec};
use crate::error::{Result, StateError};
use crate::glob::in_scope;
use crate::provider::{SnapshotManifest, StateProvider};
use crate::records::{read_records, RecordWriter};
use chrono::Utc;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

pub struct RedisProvider {
    spec: ProviderSpec,
}

impl RedisProvider {
    pub fn new(spec: ProviderSpec) -> Self {
        Self { spec }
    }

    fn connect(&self) -> Result<redis::Connection> {
        let url = self
            .spec
            .url
            .as_deref()
            .ok_or_else(|| StateError::provider(&self.spec.name, "config", "missing redis url"))?;
        let client = redis::Client::open(url)
            .map_err(|e| StateError::provider(&self.spec.name, "connect", e))?;
        client
            .get_connection()
            .map_err(|e| StateError::provider(&self.spec.name, "connect", e))
    }

    fn scan_all_keys(&self, conn: &mut redis::Connection) -> Result<Vec<Vec<u8>>> {
        let mut cursor = 0u64;
        let mut all_keys = Vec::new();

        loop {
            let (next_cursor, keys): (u64, Vec<Vec<u8>>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("COUNT")
                .arg(500)
                .query(conn)
                .map_err(|e| StateError::provider(&self.spec.name, "scan", e))?;

            for k in keys {
                if in_scope(&self.spec.include, &self.spec.exclude, &k) {
                    all_keys.push(k);
                }
            }

            cursor = next_cursor;
            if cursor == 0 {
                break;
            }
        }

        Ok(all_keys)
    }
}

impl StateProvider for RedisProvider {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Redis
    }

    fn preflight(&self) -> Result<()> {
        let mut conn = self.connect()?;
        let pong: String = redis::cmd("PING")
            .query(&mut conn)
            .map_err(|e| StateError::provider(&self.spec.name, "preflight", e))?;
        if pong != "PONG" {
            return Err(StateError::provider(&self.spec.name, "preflight", "expected PONG"));
        }
        Ok(())
    }

    fn snapshot(&self, snapshot_dir: &Path) -> Result<SnapshotManifest> {
        fs::create_dir_all(snapshot_dir).map_err(|e| StateError::io(snapshot_dir, e))?;
        let mut conn = self.connect()?;

        let keys = self.scan_all_keys(&mut conn)?;
        let mut writer = RecordWriter::new();

        for key in &keys {
            // Get TTL in ms
            let pttl: i64 = redis::cmd("PTTL")
                .arg(key)
                .query(&mut conn)
                .unwrap_or(-1);

            // Dump RDB serialization of key
            let dump: Option<Vec<u8>> = redis::cmd("DUMP")
                .arg(key)
                .query(&mut conn)
                .unwrap_or(None);

            if let Some(dump_bytes) = dump {
                let pttl_bytes = pttl.to_le_bytes();
                writer.push(&[key.as_slice(), dump_bytes.as_slice(), &pttl_bytes]);
            }
        }

        let records_path = snapshot_dir.join("records.bin");
        writer.finish(&records_path)?;

        let manifest = SnapshotManifest {
            provider_name: self.spec.name.clone(),
            kind: ProviderKind::Redis,
            created_at: Utc::now(),
            metadata: serde_json::json!({
                "key_count": keys.len(),
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
        let records_path = snapshot_dir.join("records.bin");
        read_records(&records_path)?;
        Ok(())
    }

    fn restore(&self, snapshot_dir: &Path) -> Result<()> {
        let records_path = snapshot_dir.join("records.bin");
        let records = read_records(&records_path)?;

        let mut conn = self.connect()?;
        let current_keys = self.scan_all_keys(&mut conn)?;

        let mut snap_keys_set = HashSet::new();
        for r in &records {
            if !r.is_empty() {
                snap_keys_set.insert(r[0].clone());
            }
        }

        // Delete keys present in live db that are not in snapshot
        let mut to_delete = Vec::new();
        for k in current_keys {
            if !snap_keys_set.contains(&k) {
                to_delete.push(k);
            }
        }

        if !to_delete.is_empty() {
            // Delete in chunks of 500
            for chunk in to_delete.chunks(500) {
                let mut cmd = redis::cmd("DEL");
                for k in chunk {
                    cmd.arg(k);
                }
                let _: () = cmd
                    .query(&mut conn)
                    .map_err(|e| StateError::provider(&self.spec.name, "restore_del", e))?;
            }
        }

        // Restore each key with its original RDB payload and TTL
        for r in records {
            if r.len() < 3 {
                continue;
            }
            let key = &r[0];
            let dump = &r[1];
            let pttl = if r[2].len() == 8 {
                i64::from_le_bytes(r[2].as_slice().try_into().unwrap())
            } else {
                0
            };

            let ttl_arg = if pttl > 0 { pttl } else { 0 };

            let _: () = redis::cmd("RESTORE")
                .arg(key)
                .arg(ttl_arg)
                .arg(dump)
                .arg("REPLACE")
                .query(&mut conn)
                .map_err(|e| StateError::provider(&self.spec.name, "restore_key", e))?;
        }

        Ok(())
    }

    fn discard(&self, _snapshot_dir: &Path) -> Result<()> {
        Ok(())
    }
}
