use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StateError {
    #[error("state config {path:?}: {msg}")]
    Config { path: PathBuf, msg: String },

    #[error("[{provider}] {op} failed: {msg}")]
    Provider {
        provider: String,
        op: &'static str,
        msg: String,
    },

    #[error("I/O error at {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("state snapshot at {0:?} is missing or incomplete")]
    MissingSnapshot(PathBuf),

    #[error("state snapshot file {path:?} is corrupt: {msg}")]
    Corrupt { path: PathBuf, msg: String },

    #[error("state restore partially failed (restored: [{}]); failures: {}", restored.join(", "), failures.iter().map(|(n, e)| format!("{n}: {e}")).collect::<Vec<_>>().join("; "))]
    Partial {
        restored: Vec<String>,
        failures: Vec<(String, String)>,
    },
}

pub type Result<T> = std::result::Result<T, StateError>;

impl StateError {
    pub fn provider(provider: &str, op: &'static str, msg: impl std::fmt::Display) -> Self {
        StateError::Provider {
            provider: provider.to_string(),
            op,
            msg: msg.to_string(),
        }
    }

    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        StateError::Io {
            path: path.into(),
            source,
        }
    }
}
