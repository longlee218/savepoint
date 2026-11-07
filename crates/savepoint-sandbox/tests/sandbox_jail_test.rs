use std::fs;
use std::time::Duration;
use savepoint_sandbox::{Sandbox, SandboxConfig};

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

    let mut cfg = SandboxConfig::default();
    cfg.id = Some("timeout_test".to_string());
    cfg.default_timeout = Some(Duration::from_millis(150));

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

    let mut cfg = SandboxConfig::default();
    cfg.id = Some("env_test".to_string());
    cfg.port_offset = 7;
    cfg.base_port = 3000;
    cfg.share_build_cache = true;

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
