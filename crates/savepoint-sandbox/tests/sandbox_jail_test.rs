use savepoint_sandbox::{Sandbox, SandboxConfig};
use std::fs;
use std::time::Duration;

#[test]
fn test_pgid_kills_background_grandchild_daemons() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();
    fs::write(ws.join("hello.txt"), "world").unwrap();

    let sandbox = Sandbox::create(ws, Some("pgid_test")).unwrap();
    let pid_file = sandbox.path.join("bg_pid.txt");

    // Spawn a background sleep process inside the command and print its PID
    let cmd = format!(
        "sleep 60 >/dev/null 2>&1 & echo $! > {}",
        pid_file.display()
    );
    let out = sandbox.run_command("sh", &["-c", &cmd]).unwrap();
    assert_eq!(out.exit_code, 0);
    assert!(!out.timed_out);

    let bg_pid: i32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .expect("valid pid");

    // Give the kernel a brief moment to process SIGKILL on the process group
    std::thread::sleep(Duration::from_millis(30));

    // Verify background grandchild process is dead (kill(pid, 0) returns -1 with ESRCH)
    let alive = unsafe { libc::kill(bg_pid, 0) == 0 };
    assert!(
        !alive,
        "Background grandchild PID {} survived after run_command finished!",
        bg_pid
    );
}

#[test]
fn test_command_timeout_kills_hung_process_group() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();
    fs::write(ws.join("main.rs"), "fn main() {}").unwrap();

    let cfg = SandboxConfig {
        id: Some("timeout_test".to_string()),
        default_timeout: Some(Duration::from_millis(150)),
        ..Default::default()
    };

    let sandbox = Sandbox::create_with_config(ws, cfg).unwrap();
    let out = sandbox.run_command("sh", &["-c", "sleep 30"]).unwrap();

    assert!(out.timed_out, "Expected command to time out");
    assert_ne!(out.exit_code, 0);
    assert!(
        out.duration_ms < 2000,
        "Timeout took too long: {}ms",
        out.duration_ms
    );
}

#[test]
fn test_env_port_offset_and_shared_cache_virtualization() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();
    fs::write(ws.join("app.txt"), "v1").unwrap();

    let cfg = SandboxConfig {
        id: Some("env_test".to_string()),
        port_offset: 7,
        base_port: 3000,
        share_build_cache: true,
        ..Default::default()
    };

    let sandbox = Sandbox::create_with_config(ws, cfg).unwrap();
    let out = sandbox
        .run_command(
            "sh",
            &[
                "-c",
                "echo \"hello\" > \"$TMPDIR/scratch.tmp\" && printf \"%s|%s|%s\" \"$PORT\" \"$SAVEPOINT_PORT_OFFSET\" \"$CARGO_TARGET_DIR\"",
            ],
        )
        .unwrap();

    assert_eq!(out.exit_code, 0);
    let parts: Vec<&str> = out.stdout.split('|').collect();
    assert_eq!(parts[0], "3007");
    assert_eq!(parts[1], "7");
    assert!(parts[2].contains("shared-cache/cargo-target"));

    // Files written inside $TMPDIR (.sp-tmp) must NOT pollute workspace deltas
    let deltas = sandbox.compute_deltas().unwrap();
    assert!(
        deltas.is_empty(),
        "Expected 0 deltas when writing only to TMPDIR, got: {:?}",
        deltas
    );
}

#[test]
fn test_promote_to_workspace_uses_cow_and_handles_collapsed_node_modules() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src/lib.rs"), "pub fn v1() {}").unwrap();

    let sandbox = Sandbox::create(ws, Some("promote_test")).unwrap();

    // Modify src/lib.rs and create a collapsed heavy dependency directory node_modules
    fs::write(sandbox.path.join("src/lib.rs"), "pub fn v2() {}").unwrap();
    fs::create_dir_all(sandbox.path.join("node_modules/lodash")).unwrap();
    fs::write(
        sandbox.path.join("node_modules/lodash/index.js"),
        "module.exports = {};",
    )
    .unwrap();

    let promoted_deltas = sandbox
        .promote_to_workspace(ws)
        .expect("promote_to_workspace must succeed on files and collapsed node_modules");

    assert_eq!(promoted_deltas.len(), 2);
    assert_eq!(
        fs::read_to_string(ws.join("src/lib.rs")).unwrap(),
        "pub fn v2() {}"
    );
    assert!(ws.join("node_modules/lodash/index.js").exists());
}

#[test]
fn test_rejects_malicious_sandbox_id_and_drains_large_output_without_deadlock() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();
    fs::write(ws.join("a.txt"), "ok").unwrap();

    // 1. Path traversal / invalid sandbox IDs must be rejected
    for bad_id in ["../evil", "/tmp/evil", "foo/bar", "", ".."] {
        let res = Sandbox::create(ws, Some(bad_id));
        assert!(
            res.is_err(),
            "Expected sandbox ID '{}' to be rejected",
            bad_id
        );
    }

    // 2. Output exceeding OS pipe buffer (128 KiB) with max_output_bytes = 1024 must not deadlock
    let cfg = SandboxConfig {
        id: Some("bounded_output_test".to_string()),
        max_output_bytes: 1024,
        ..Default::default()
    };
    let sandbox = Sandbox::create_with_config(ws, cfg).unwrap();
    // Generate ~100,000 bytes of stdout
    let out = sandbox
        .run_command("sh", &["-c", "yes '0123456789abcdef' | head -n 6000"])
        .unwrap();
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout.len(), 1024);
}

#[test]
fn test_network_egress_guard_and_build_jobs_virtualization() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();
    fs::write(ws.join("lib.rs"), "").unwrap();

    let cfg = SandboxConfig {
        id: Some("egress_jobs_test".to_string()),
        block_network_egress: true,
        build_jobs_limit: Some(2),
        ..Default::default()
    };
    let sandbox = Sandbox::create_with_config(ws, cfg).unwrap();
    let out = sandbox
        .run_command(
            "sh",
            &[
                "-c",
                "printf \"%s|%s|%s|%s\" \"$SAVEPOINT_OFFLINE_MODE\" \"$HTTPS_PROXY\" \"$CARGO_BUILD_JOBS\" \"$MAKEFLAGS\"",
            ],
        )
        .unwrap();
    assert_eq!(out.exit_code, 0);
    let parts: Vec<&str> = out.stdout.split('|').collect();
    assert_eq!(parts[0], "1");
    assert_eq!(parts[1], "http://127.0.0.1:1");
    assert_eq!(parts[2], "2");
    assert!(parts[3].contains("-j2"));
}
