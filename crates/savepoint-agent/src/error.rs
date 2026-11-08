use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AgentError {
    #[error("I/O error at {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Lease already exists for agent '{0}'")]
    LeaseAlreadyExists(String),

    #[error("Lease '{0}' not found or expired")]
    LeaseNotFound(String),

    #[error("Port offset exhaustion: max {0} agents")]
    PortExhaustion(usize),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("General agent error: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, AgentError>;

impl AgentError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
