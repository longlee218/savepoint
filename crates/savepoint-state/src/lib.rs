//! Out-of-tree state snapshots for Savepoint checkpoints.
//!
//! Each [`provider::StateProvider`] captures one external store (a SQLite file,
//! a PostgreSQL / MySQL database, a Redis logical DB, a Memcached instance)
//! into a checkpoint directory and can later make the live store equal to it.
pub mod config;
pub mod error;
pub mod glob;
pub mod manager;
pub mod memcached;
pub mod mysql;
pub mod postgres;
pub mod provider;
pub mod records;
pub mod redis;
pub mod sqlite;

pub use error::{Result, StateError};
pub use manager::StateManager;
pub use provider::{SnapshotManifest, StateProvider};
