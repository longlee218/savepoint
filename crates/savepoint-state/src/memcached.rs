use crate::config::{ProviderKind, ProviderSpec};
use crate::error::{Result, StateError};
use crate::glob::in_scope;
use crate::provider::{SnapshotManifest, StateProvider};
use crate::records::{read_records, RecordWriter};
use chrono::Utc;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

pub struct MemcachedProvider {
    spec: ProviderSpec,
}

impl MemcachedProvider {
    pub fn new(spec: ProviderSpec) -> Self {
        Self { spec }
    }

    fn resolve_addr(&self) -> Result<String> {
        let raw = self
            .spec
            .url
            .as_deref()
            .ok_or_else(|| StateError::provider(&self.spec.name, "config", "missing url"))?;

        let addr = raw
            .strip_prefix("memcached://")
            .unwrap_or(raw)
            .split('/')
            .next()
            .unwrap_or("");

        if addr.is_empty() {
            return Err(StateError::provider(&self.spec.name, "config", "invalid memcached address"));
        }
        Ok(addr.to_string())
    }

    fn connect(&self) -> Result<TcpStream> {
        let addr = self.resolve_addr()?;
        let stream = TcpStream::connect(&addr)
            .map_err(|e| StateError::provider(&self.spec.name, "connect", e))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(self.spec.lock_timeout_secs)))
            .map_err(|e| StateError::provider(&self.spec.name, "set_timeout", e))?;
        stream
            .set_write_timeout(Some(Duration::from_secs(self.spec.lock_timeout_secs)))
            .map_err(|e| StateError::provider(&self.spec.name, "set_timeout", e))?;
        Ok(stream)
    }

    fn scan_keys(&self) -> Result<Vec<(String, u32, i64)>> {
        for attempt in 0..10 {
            let mut stream = self.connect()?;
            stream
                .write_all(b"lru_crawler metadump all\r\n")
                .map_err(|e| StateError::provider(&self.spec.name, "metadump", e))?;

            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader
                .read_line(&mut line)
                .map_err(|e| StateError::provider(&self.spec.name, "read_metadump", e))?;

            if line.starts_with("BUSY") {
                std::thread::sleep(Duration::from_millis(50 * (attempt + 1)));
                continue;
            }

            if !line.starts_with("OK") {
                return Err(StateError::provider(
                    &self.spec.name,
                    "metadump",
                    format!("expected OK, got {}", line.trim()),
                ));
            }

            let mut keys = Vec::new();
            loop {
                line.clear();
                let n = reader
                    .read_line(&mut line)
                    .map_err(|e| StateError::provider(&self.spec.name, "read_metadump_line", e))?;
                if n == 0 || line.starts_with("END") {
                    break;
                }

                let mut key_str = None;
                let mut exp = 0i64;
                let mut flags = 0u32;

                for token in line.split_whitespace() {
                    if let Some(k) = token.strip_prefix("key=") {
                        let decoded = percent_encoding::percent_decode_str(k).decode_utf8_lossy();
                        key_str = Some(decoded.to_string());
                    } else if let Some(e) = token.strip_prefix("exp=") {
                        exp = e.parse().unwrap_or(0);
                    } else if let Some(f) = token.strip_prefix("flags=") {
                        flags = f.parse().unwrap_or(0);
                    }
                }

                if let Some(k) = key_str {
                    let now = Utc::now().timestamp();
                    if exp > 0 && exp <= now {
                        continue;
                    }
                    if in_scope(&self.spec.include, &self.spec.exclude, k.as_bytes()) {
                        keys.push((k, flags, exp));
                    }
                }
            }

            return Ok(keys);
        }

        Err(StateError::provider(
            &self.spec.name,
            "metadump",
            "crawler busy after 10 attempts",
        ))
    }

    fn get_value(&self, stream: &mut TcpStream, key: &str) -> Result<Option<(Vec<u8>, u32)>> {
        let cmd = format!("get {}\r\n", key);
        stream
            .write_all(cmd.as_bytes())
            .map_err(|e| StateError::provider(&self.spec.name, "get_write", e))?;

        let mut reader = BufReader::new(stream);
        let mut header = String::new();
        reader
            .read_line(&mut header)
            .map_err(|e| StateError::provider(&self.spec.name, "get_read_header", e))?;

        if header.starts_with("END") {
            return Ok(None);
        }

        if !header.starts_with("VALUE") {
            return Ok(None);
        }

        // Header: VALUE <key> <flags> <bytes> [<cas>]
        let parts: Vec<&str> = header.split_whitespace().collect();
        if parts.len() < 4 {
            return Ok(None);
        }

        let flags: u32 = parts[2].parse().unwrap_or(0);
        let bytes_len: usize = parts[3].parse().unwrap_or(0);

        let mut val_buf = vec![0u8; bytes_len];
        reader
            .read_exact(&mut val_buf)
            .map_err(|e| StateError::provider(&self.spec.name, "get_read_bytes", e))?;

        // Read trailing \r\n and END\r\n
        let mut end_line = String::new();
        let _ = reader.read_line(&mut end_line); // trailing \r\n
        end_line.clear();
        let _ = reader.read_line(&mut end_line); // END\r\n

        Ok(Some((val_buf, flags)))
    }
}

impl StateProvider for MemcachedProvider {
    fn name(&self) -> &str {
        &self.spec.name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Memcached
    }

    fn preflight(&self) -> Result<()> {
        let mut stream = self.connect()?;
        stream
            .write_all(b"version\r\n")
            .map_err(|e| StateError::provider(&self.spec.name, "preflight", e))?;

        let mut reader = BufReader::new(stream);
        let mut resp = String::new();
        reader
            .read_line(&mut resp)
            .map_err(|e| StateError::provider(&self.spec.name, "preflight", e))?;

        if !resp.starts_with("VERSION") {
            return Err(StateError::provider(&self.spec.name, "preflight", "expected VERSION"));
        }
        Ok(())
    }

    fn snapshot(&self, snapshot_dir: &Path) -> Result<SnapshotManifest> {
        fs::create_dir_all(snapshot_dir).map_err(|e| StateError::io(snapshot_dir, e))?;
        let keys_meta = self.scan_keys()?;

        let mut writer = RecordWriter::new();
        let mut key_count = 0;

        for (key, flags, exp) in keys_meta {
            let mut stream = self.connect()?;
            if let Some((val, actual_flags)) = self.get_value(&mut stream, &key)? {
                let flags_use = if actual_flags != 0 { actual_flags } else { flags };
                let flags_bytes = flags_use.to_le_bytes();
                let exp_bytes = exp.to_le_bytes();
                writer.push(&[key.as_bytes(), &val, &flags_bytes, &exp_bytes]);
                key_count += 1;
            }
        }

        let records_path = snapshot_dir.join("records.bin");
        writer.finish(&records_path)?;

        let manifest = SnapshotManifest {
            provider_name: self.spec.name.clone(),
            kind: ProviderKind::Memcached,
            created_at: Utc::now(),
            metadata: serde_json::json!({
                "key_count": key_count,
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

        let live_keys_meta = self.scan_keys()?;
        let mut live_keys = HashSet::new();
        for (k, _, _) in live_keys_meta {
            live_keys.insert(k);
        }

        let mut snap_keys = HashSet::new();
        for r in &records {
            if !r.is_empty() {
                if let Ok(s) = String::from_utf8(r[0].clone()) {
                    snap_keys.insert(s);
                }
            }
        }

        // Delete keys present in live instance that are not in snapshot
        let mut stream = self.connect()?;
        for k in live_keys {
            if !snap_keys.contains(&k) {
                let cmd = format!("delete {}\r\n", k);
                let _ = stream.write_all(cmd.as_bytes());
                let mut resp = [0u8; 32];
                let _ = stream.read(&mut resp);
            }
        }

        // Restore each key with its original flags, TTL, and bytes
        for r in records {
            if r.len() < 4 {
                continue;
            }
            let key = String::from_utf8_lossy(&r[0]);
            let val = &r[1];
            let flags = if r[2].len() == 4 {
                u32::from_le_bytes(r[2].as_slice().try_into().unwrap())
            } else {
                0
            };
            let exp = if r[3].len() == 8 {
                i64::from_le_bytes(r[3].as_slice().try_into().unwrap())
            } else {
                0
            };

            // Calculate TTL in relative seconds if exp was unix timestamp
            let now = Utc::now().timestamp();
            let relative_ttl = if exp > now {
                exp - now
            } else if exp > 0 {
                exp
            } else {
                0
            };

            let cmd = format!("set {} {} {} {}\r\n", key, flags, relative_ttl, val.len());
            stream
                .write_all(cmd.as_bytes())
                .map_err(|e| StateError::provider(&self.spec.name, "restore_set_cmd", e))?;
            stream
                .write_all(val)
                .map_err(|e| StateError::provider(&self.spec.name, "restore_set_val", e))?;
            stream
                .write_all(b"\r\n")
                .map_err(|e| StateError::provider(&self.spec.name, "restore_set_eol", e))?;

            let mut resp = [0u8; 32];
            let _ = stream.read(&mut resp);
        }

        Ok(())
    }

    fn discard(&self, _snapshot_dir: &Path) -> Result<()> {
        Ok(())
    }
}
