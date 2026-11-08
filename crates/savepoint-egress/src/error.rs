use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum EgressError {
    #[error("I/O error at {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Cassette missing: {0}")]
    CassetteMissing(String),

    #[error("Cassette corrupt at {path:?}: {msg}")]
    CassetteCorrupt { path: PathBuf, msg: String },

    #[error("Mutation blocked by guard: {method} {url} (rule: {rule})")]
    MutationBlocked {
        method: String,
        url: String,
        rule: String,
    },

    #[error("Saga rollback failure ({failed_count} actions failed): {details}")]
    SagaRollbackFailed {
        failed_count: usize,
        details: String,
    },

    #[error("General egress error: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, EgressError>;

impl EgressError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
