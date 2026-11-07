use std::fs;
use savepoint_fs::{FlightRecorder, ShadowEngine};

#[test]
fn test_flight_recorder_export_scrubs_secrets_and_replays_step_accurately() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path().join("ws");
    let repro = temp.path().join("repro");
    let bundle_path = temp.path().join("bug_report.spbundle");

    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(
        ws.join("src/app.rs"),
        "pub fn run() -> &'static str { \"v1\" }",
    )
    .unwrap();
    fs::write(
        ws.join(".env"),
        "OPENAI_API_KEY=sk-1234567890abcdefghijklmnop\n",
    )
    .unwrap();

    let mut tx = ShadowEngine::begin_transaction(&ws).unwrap();

    // Step 1: valid feature + config file containing an accidental inline API token
    // and a normal kebab-case identifier "flask-restful-architecture-blueprint" that ends in "sk-"
    fs::write(
        ws.join("src/app.rs"),
        "pub fn run() -> &'static str { \"v2_good_flask-restful-architecture-blueprint\" }",
    )
    .unwrap();
    fs::write(
        ws.join("src/config.toml"),
        "endpoint = \"https://api.example.com\"\ntoken = \"sk-proj9876543210abcdefghijklmnop\"\n",
    )
    .unwrap();
    ShadowEngine::checkpoint(
        &mut tx,
        Some("Turn 1 with secret sk-inline1234567890abcdefgh"),
        None,
    )
    .unwrap();

    // Step 2: buggy code introduced by agent
    fs::write(
        ws.join("src/app.rs"),
        "pub fn run() -> &'static str { \"v3_buggy\" }",
    )
    .unwrap();
    ShadowEngine::checkpoint(&mut tx, Some("Turn 2 broke app.rs"), None).unwrap();

    // Commit transaction to also prove load_latest_transaction works on committed transactions
    ShadowEngine::commit(&mut tx).unwrap();

    // Export bundle with pinned_step = Some(1)
    let export_stats =
        FlightRecorder::export_bundle_with_step(&ws, &bundle_path, Some(1)).unwrap();
    assert!(bundle_path.exists());
    assert_eq!(export_stats.pinned_step, Some(1));
    assert!(
        export_stats.replay_uri.starts_with("sp://replay/"),
        "Expected sp://replay/<id> URI, got {}",
        export_stats.replay_uri
    );
    assert!(
        export_stats.scrubbed_files_count >= 1,
        "Expected .env to be scrubbed, got {}",
        export_stats.scrubbed_files_count
    );
    assert!(
        export_stats.scrubbed_secrets_count >= 2,
        "Expected inline sk- tokens in config.toml and prompt to be scrubbed, got {}",
        export_stats.scrubbed_secrets_count
    );

    let raw_bundle_json = fs::read_to_string(&bundle_path).unwrap();
    assert!(!raw_bundle_json.contains("sk-1234567890abcdefghijklmnop"));
    assert!(!raw_bundle_json.contains("sk-proj9876543210abcdefghijklmnop"));
    assert!(!raw_bundle_json.contains("sk-inline1234567890abcdefgh"));
    assert!(raw_bundle_json.contains("[REDACTED_SECRET]"));
    // False-positive check: "flask-restful-architecture-blueprint" must NOT be scrubbed!
    assert!(
        raw_bundle_json.contains("flask-restful-architecture-blueprint"),
        "False positive secret scrub on 'flask-restful-architecture-blueprint'!"
    );

    // Replay Step 1 into repro directory
    let replay1 = FlightRecorder::replay_bundle(&bundle_path, &repro, Some(1)).unwrap();
    assert_eq!(replay1.replayed_step, 1);
    assert_eq!(
        fs::read_to_string(repro.join("src/app.rs")).unwrap(),
        "pub fn run() -> &'static str { \"v2_good_flask-restful-architecture-blueprint\" }"
    );
    let restored_cfg = fs::read_to_string(repro.join("src/config.toml")).unwrap();
    assert!(restored_cfg.contains("[REDACTED_SECRET]"));
    assert!(
        !repro.join(".env").exists(),
        ".env must never be exported or replayed"
    );

    // Replay Step 2 into repro directory
    let replay2 = FlightRecorder::replay_bundle(&bundle_path, &repro, Some(2)).unwrap();
    assert_eq!(replay2.replayed_step, 2);
    assert_eq!(
        fs::read_to_string(repro.join("src/app.rs")).unwrap(),
        "pub fn run() -> &'static str { \"v3_buggy\" }"
    );

    // Symlink escape guard: if repro/src/app.rs is replaced with a symlink pointing outside repro,
    // replay_bundle must NOT follow the symlink to overwrite the external target!
    let external_victim = temp.path().join("external_victim.txt");
    fs::write(&external_victim, "DO_NOT_TOUCH").unwrap();
    fs::remove_file(repro.join("src/app.rs")).unwrap();
    std::os::unix::fs::symlink(&external_victim, repro.join("src/app.rs")).unwrap();

    FlightRecorder::replay_bundle(&bundle_path, &repro, Some(1)).unwrap();
    assert_eq!(
        fs::read_to_string(&external_victim).unwrap(),
        "DO_NOT_TOUCH",
        "replay_bundle followed a symlink and overwrote an external file!"
    );
    assert!(!repro
        .join("src/app.rs")
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());

    // Replay using the sp://replay/<id> URI with step = None (should use pinned_step = 1!)
    let uri_repro = temp.path().join("uri_repro");
    let replay_via_uri = FlightRecorder::replay_bundle(
        std::path::Path::new(&export_stats.replay_uri),
        &uri_repro,
        None,
    )
    .unwrap();
    assert_eq!(replay_via_uri.replayed_step, 1);
    assert_eq!(
        fs::read_to_string(uri_repro.join("src/app.rs")).unwrap(),
        "pub fn run() -> &'static str { \"v2_good_flask-restful-architecture-blueprint\" }"
    );
}
