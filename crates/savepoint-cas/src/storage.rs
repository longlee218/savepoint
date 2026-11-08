use crate::chunker::Chunker;
use crate::error::{CasError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobManifest {
    pub total_size: usize,
    pub chunks: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CasConfig {
    pub zstd_level: i32,
    pub min_chunk: usize,
    pub avg_chunk: usize,
    pub max_chunk: usize,
}

impl Default for CasConfig {
    fn default() -> Self {
        Self {
            zstd_level: 3,
            min_chunk: 16 * 1024,
            avg_chunk: 64 * 1024,
            max_chunk: 128 * 1024,
        }
    }
}

#[derive(Debug, Default)]
pub struct GcStats {
    pub pruned_chunks: usize,
    pub freed_bytes: u64,
}

pub struct CasEngine {
    root: PathBuf,
    config: CasConfig,
    chunker: Chunker,
}

impl CasEngine {
    pub fn open(root: impl Into<PathBuf>, config: CasConfig) -> Result<Self> {
        let root = root.into();
        let objects_dir = root.join("objects");
        fs::create_dir_all(&objects_dir).map_err(|e| CasError::io(&objects_dir, e))?;

        let chunker = Chunker::new(config.min_chunk, config.avg_chunk, config.max_chunk);

        Ok(Self {
            root,
            config,
            chunker,
        })
    }

    fn chunk_path(&self, hash: &str) -> PathBuf {
        let prefix = &hash[..2];
        let suffix = &hash[2..];
        self.root
            .join("objects")
            .join(prefix)
            .join(format!("{}.zst", suffix))
    }

    pub fn store_blob(&self, data: &[u8]) -> Result<BlobManifest> {
        let chunks = self.chunker.chunk(data);
        let mut manifest_hashes = Vec::with_capacity(chunks.len());

        for (slice, meta) in chunks {
            let path = self.chunk_path(&meta.hash);
            manifest_hashes.push(meta.hash.clone());

            // Deduplication: if chunk file already exists, reuse it without rewriting
            if path.exists() {
                continue;
            }

            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| CasError::io(parent, e))?;
            }

            let compressed = zstd::encode_all(slice, self.config.zstd_level)
                .map_err(|e| CasError::Compression(e.to_string()))?;

            let tmp_path = path.with_extension("tmp-chunk");
            fs::write(&tmp_path, &compressed).map_err(|e| CasError::io(&tmp_path, e))?;
            fs::rename(&tmp_path, &path).map_err(|e| CasError::io(&path, e))?;
        }

        Ok(BlobManifest {
            total_size: data.len(),
            chunks: manifest_hashes,
        })
    }

    pub fn read_blob(&self, manifest: &BlobManifest) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(manifest.total_size);

        for hash in &manifest.chunks {
            let path = self.chunk_path(hash);
            if !path.exists() {
                return Err(CasError::ChunkNotFound(hash.clone()));
            }

            let compressed = fs::read(&path).map_err(|e| CasError::io(&path, e))?;
            let decompressed = zstd::decode_all(compressed.as_slice())
                .map_err(|e| CasError::Compression(e.to_string()))?;

            // Integrity verification
            let computed_hash = blake3::hash(&decompressed).to_hex().to_string();
            if &computed_hash != hash {
                return Err(CasError::CorruptChunk {
                    hash: hash.clone(),
                    msg: format!("Computed {} does not match expected {}", computed_hash, hash),
                });
            }

            out.extend_from_slice(&decompressed);
        }

        Ok(out)
    }

    pub fn total_disk_usage(&self) -> Result<u64> {
        let mut total = 0;
        let objects_dir = self.root.join("objects");
        if !objects_dir.exists() {
            return Ok(0);
        }

        for prefix_entry in fs::read_dir(&objects_dir).map_err(|e| CasError::io(&objects_dir, e))? {
            let prefix_entry = prefix_entry.map_err(|e| CasError::io(&objects_dir, e))?;
            if prefix_entry.path().is_dir() {
                for file_entry in fs::read_dir(prefix_entry.path()).map_err(|e| CasError::io(prefix_entry.path(), e))? {
                    let file_entry = file_entry.map_err(|e| CasError::io(prefix_entry.path(), e))?;
                    if let Ok(meta) = file_entry.metadata() {
                        if meta.is_file() {
                            total += meta.len();
                        }
                    }
                }
            }
        }

        Ok(total)
    }

    pub fn run_gc(&self, active_manifests: &[&BlobManifest]) -> Result<GcStats> {
        let mut referenced_hashes = HashSet::new();
        for m in active_manifests {
            for h in &m.chunks {
                referenced_hashes.insert(h.clone());
            }
        }

        let mut stats = GcStats::default();
        let objects_dir = self.root.join("objects");
        if !objects_dir.exists() {
            return Ok(stats);
        }

        for prefix_entry in fs::read_dir(&objects_dir).map_err(|e| CasError::io(&objects_dir, e))? {
            let prefix_entry = prefix_entry.map_err(|e| CasError::io(&objects_dir, e))?;
            let prefix_path = prefix_entry.path();
            if prefix_path.is_dir() {
                let prefix_name = prefix_entry.file_name().to_string_lossy().to_string();
                for file_entry in fs::read_dir(&prefix_path).map_err(|e| CasError::io(&prefix_path, e))? {
                    let file_entry = file_entry.map_err(|e| CasError::io(&prefix_path, e))?;
                    let file_name = file_entry.file_name().to_string_lossy().to_string();
                    if let Some(suffix) = file_name.strip_suffix(".zst") {
                        let full_hash = format!("{}{}", prefix_name, suffix);
                        if !referenced_hashes.contains(&full_hash) {
                            if let Ok(m) = file_entry.metadata() {
                                stats.freed_bytes += m.len();
                            }
                            let _ = fs::remove_file(file_entry.path());
                            stats.pruned_chunks += 1;
                        }
                    }
                }
            }
        }

        Ok(stats)
    }
}
