use thiserror::Error;
use std::path::PathBuf;

#[derive(Error, Debug)]
pub enum SavepointError {
    #[error("I/O error at {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("APFS clonefile syscall failed for '{src}' -> '{dst}': errno {errno}")]
    CloneFileFailed {
        src: PathBuf,
        dst: PathBuf,
        errno: i32,
    },

    #[error("Transaction {0} not found")]
    TransactionNotFound(String),

    #[error("Transaction {0} is already finished (Status: {1:?})")]
    TransactionFinished(String, crate::types::TransactionStatus),

    #[error("Checkpoint {0} not found in transaction {1}")]
    CheckpointNotFound(usize, String),

    #[error("Branch '{0}' not found")]
    BranchNotFound(String),

    #[error("Branch '{0}' already exists")]
    BranchAlreadyExists(String),

    #[error("Speculative execution failed: {0}")]
    SpeculationFailed(String),

    #[error("Workspace path {0:?} is invalid or does not exist")]
    InvalidWorkspace(PathBuf),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("General error: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, SavepointError>;
