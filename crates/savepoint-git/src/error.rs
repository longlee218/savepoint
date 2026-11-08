use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum GitError {
    #[error("I/O error at {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Git command failed: {cmd} (status {code:?}): {stderr}")]
    CommandFailed {
        cmd: String,
        code: Option<i32>,
        stderr: String,
    },

    #[error("Not a git repository: {0:?}")]
    NotAGitRepo(PathBuf),

    #[error("General git reconciler error: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, GitError>;

impl GitError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
