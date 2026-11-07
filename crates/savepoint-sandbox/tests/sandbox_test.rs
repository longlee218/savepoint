use savepoint_sandbox::Sandbox;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_ephemeral_cow_sandbox_isolation_and_promotion() {
    let ws_dir = tempdir().expect("tempdir");
    let ws_path = ws_dir.path();
    let original_file = ws_path.join("app.rs");
    fs::write(&original_file, "const VERSION: &str = \"1.0.0\";").expect("write original");

    // 1. Create instant CoW sandbox
    let sandbox = Sandbox::create(ws_path, Some("test_sb_01")).expect("create sandbox");
    assert!(sandbox.path.exists());
    assert!(sandbox.path.join("app.rs").exists());

    // 2. Modify files inside sandbox
    let sandbox_file = sandbox.path.join("app.rs");
    fs::write(&sandbox_file, "const VERSION: &str = \"2.0.0\";").expect("write sandbox file");

    let new_feature = sandbox.path.join("feature.rs");
    fs::write(&new_feature, "pub fn new_feature() {}").expect("write new feature");

    // Verify ISOLATION: original workspace is completely untouched!
    assert_eq!(
        fs::read_to_string(&original_file).unwrap(),
        "const VERSION: &str = \"1.0.0\";"
    );
    assert!(!ws_path.join("feature.rs").exists());

    // 3. Run command inside sandbox
    let output = sandbox
        .run_command("sh", &["-c", "cat app.rs"])
        .expect("run command");
    assert_eq!(output.exit_code, 0);
    assert!(output.stdout.contains("2.0.0"));

    // 4. Compute deltas
    let deltas = sandbox.compute_deltas().expect("compute deltas");
    assert_eq!(deltas.len(), 2);

    // 5. Promote sandbox to original workspace
    let promoted_deltas = sandbox.promote_to_workspace(ws_path).expect("promote");
    assert_eq!(promoted_deltas.len(), 2);

    // Verify workspace is now updated with sandbox changes!
    assert_eq!(
        fs::read_to_string(&original_file).unwrap(),
        "const VERSION: &str = \"2.0.0\";"
    );
    assert!(ws_path.join("feature.rs").exists());

    // 6. Cleanup
    let sandbox_path = sandbox.path.clone();
    sandbox.destroy().expect("destroy");
    assert!(!sandbox_path.exists());
}
