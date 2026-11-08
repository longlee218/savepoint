pub mod bundle;
pub mod clone;
pub mod darwin;
pub mod diff;
pub mod engine;
pub mod git_bridge;
pub mod hooks;
pub mod ignore;
pub mod linux;

pub use bundle::{
    BundleExportStats, BundleFileState, BundleReplayStats, FlightBundle, FlightRecorder,
};
pub use clone::{clone_tree, clone_workspace};
pub use diff::compute_deltas;
pub use engine::ShadowEngine;
pub use git_bridge::GitCommitResult;
pub use hooks::{HookExecutionReport, LifecycleHooks};
pub use ignore::{SavepointIgnore, ALWAYS_IGNORED_NAMES, HEAVY_CACHE_DIRS, HEAVY_DEP_DIRS};
