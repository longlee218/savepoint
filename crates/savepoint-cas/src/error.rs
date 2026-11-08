use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CasError {
    #[error("I/O error at {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Chunk not found: {0}")]
    ChunkNotFound(String),

    #[error("Corrupt chunk {hash}: {msg}")]
    CorruptChunk { hash: String, msg: String },

    #[error("Compression error: {0}")]
    Compression(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("General CAS error: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, CasError>;

impl CasError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
