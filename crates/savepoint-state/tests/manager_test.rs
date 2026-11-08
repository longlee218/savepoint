use savepoint_state::config::ProviderKind;
use savepoint_state::error::{Result, StateError};
use savepoint_state::provider::{SnapshotManifest, StateProvider};
use savepoint_state::StateManager;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tempfile::tempdir;

struct MockProvider {
    name: String,
    fail_snapshot: bool,
    fail_verify: bool,
    discard_called: Arc<AtomicBool>,
}

impl StateProvider for MockProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Sqlite
    }

    fn preflight(&self) -> Result<()> {
        Ok(())
    }

    fn snapshot(&self, snapshot_dir: &Path) -> Result<SnapshotManifest> {
        if self.fail_snapshot {
            return Err(StateError::provider(&self.name, "snapshot", "intentional failure"));
        }
        fs::create_dir_all(snapshot_dir).unwrap();
        fs::write(snapshot_dir.join("marker.txt"), "ok").unwrap();
        Ok(SnapshotManifest {
            provider_name: self.name.clone(),
            kind: ProviderKind::Sqlite,
            created_at: chrono::Utc::now(),
            metadata: serde_json::json!({}),
        })
    }

    fn verify(&self, snapshot_dir: &Path) -> Result<()> {
        if self.fail_verify {
            return Err(StateError::Corrupt {
                path: snapshot_dir.to_path_buf(),
                msg: "verification failed".to_string(),
            });
        }
        Ok(())
    }

    fn restore(&self, _snapshot_dir: &Path) -> Result<()> {
        Ok(())
    }

    fn discard(&self, snapshot_dir: &Path) -> Result<()> {
        self.discard_called.store(true, Ordering::SeqCst);
        let _ = fs::remove_dir_all(snapshot_dir);
        Ok(())
    }
}

#[test]
fn test_manager_all_or_nothing_snapshot_on_failure() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join("state_snap");

    let p1_discarded = Arc::new(AtomicBool::new(false));
    let p1 = Box::new(MockProvider {
        name: "p1".to_string(),
        fail_snapshot: false,
        fail_verify: false,
        discard_called: p1_discarded.clone(),
    });

    let p2_discarded = Arc::new(AtomicBool::new(false));
    let p2 = Box::new(MockProvider {
        name: "p2".to_string(),
        fail_snapshot: true, // p2 fails!
        fail_verify: false,
        discard_called: p2_discarded.clone(),
    });

    let manager = StateManager::new(dir.path().to_path_buf(), vec![p1, p2]);

    let res = manager.snapshot_all(&state_dir);
    assert!(res.is_err(), "Snapshot must fail if any provider fails");

    // All-or-nothing check: p1's discard must have been invoked and state_dir removed!
    assert!(
        p1_discarded.load(Ordering::SeqCst),
        "P1 must have been discarded after P2 failed"
    );
    assert!(!state_dir.exists(), "State directory must be cleaned up on failure");
}

#[test]
fn test_manager_restore_aborts_before_touching_live_state_if_verification_fails() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join("state_snap_corrupt");
    fs::create_dir_all(&state_dir).unwrap();

    let p1 = Box::new(MockProvider {
        name: "p1".to_string(),
        fail_snapshot: false,
        fail_verify: true, // verification fails
        discard_called: Arc::new(AtomicBool::new(false)),
    });

    let manager = StateManager::new(dir.path().to_path_buf(), vec![p1]);
    let err = manager.restore_all(&state_dir).unwrap_err();
    assert!(err.to_string().contains("verification failed"));
}
