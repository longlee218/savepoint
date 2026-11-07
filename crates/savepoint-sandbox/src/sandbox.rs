use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use savepoint_core::{ChangeKind, FileDelta, Result, SavepointError};
use savepoint_fs::{clone_tree, clone_workspace, compute_deltas, ShadowEngine};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SandboxConfig {
    pub id: Option<String>,
    pub port_offset: u16,
    pub base_port: u16,
    pub default_timeout: Option<Duration>,
    pub max_output_bytes: usize,
    pub share_build_cache: bool,
    pub block_network_egress: bool,
    pub build_jobs_limit: Option<usize>,
    pub extra_env: HashMap<String, String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            id: None,
            port_offset: 0,
            base_port: 3000,
            default_timeout: Some(Duration::from_secs(300)),
            max_output_bytes: 2 * 1024 * 1024, // 2 MiB per stream
            share_build_cache: true,
            block_network_egress: false,
            build_jobs_limit: None,
            extra_env: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration_ms: u128,
    pub timed_out: bool,
}

pub struct Sandbox {
    pub id: String,
    pub path: PathBuf,
    pub source: PathBuf,
    pub storage_root: PathBuf,
    pub config: SandboxConfig,
    pub auto_cleanup: bool,
    active_pgids: Arc<Mutex<HashSet<i32>>>,
}

impl Sandbox {
    /// Creates a new ephemeral CoW sandbox cloned from `source_workspace` with default configuration.
    pub fn create(source_workspace: &Path, id: Option<&str>) -> Result<Self> {
        let mut cfg = SandboxConfig::default();
        cfg.id = id.map(|s| s.to_string());
        Self::create_with_config(source_workspace, cfg)
    }

    /// Creates a new ephemeral CoW sandbox with custom `SandboxConfig`.
    /// Places the sandbox on the same filesystem device (`st_dev`) via `ShadowEngine::resolve_storage_dir`
    /// and clones top-level entries via `clone_workspace` (skipping `.git` and `.savepoint`).
    pub fn create_with_config(source_workspace: &Path, config: SandboxConfig) -> Result<Self> {
        let canonical_source = source_workspace.canonicalize().map_err(|e| SavepointError::Io {
            path: source_workspace.to_path_buf(),
            source: e,
        })?;

        let sandbox_id = match &config.id {
            Some(id) => {
                if id.is_empty()
                    || id.contains('/')
                    || id.contains('\\')
                    || id.contains("..")
                    || !id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    return Err(SavepointError::Other(format!(
                        "Invalid sandbox ID '{}': must be non-empty and contain only alphanumeric, '-', or '_' characters",
                        id
                    )));
                }
                id.clone()
            }
            None => format!("sb_{}", &Uuid::new_v4().simple().to_string()[..8]),
        };

        let storage_root = ShadowEngine::resolve_storage_dir(&canonical_source);
        let sandbox_dir = storage_root.join("sandboxes").join(&sandbox_id);

        if sandbox_dir.symlink_metadata().is_ok() {
            remove_any_path(&sandbox_dir)?;
        }

        clone_workspace(&canonical_source, &sandbox_dir)?;

        // Create isolated TMPDIR inside sandbox (.sp-tmp is ignored by savepoint-fs diff)
        let sp_tmp = sandbox_dir.join(".sp-tmp");
        fs::create_dir_all(&sp_tmp).map_err(|e| SavepointError::Io {
            path: sp_tmp,
            source: e,
        })?;

        Ok(Self {
            id: sandbox_id,
            path: sandbox_dir,
            source: canonical_source,
            storage_root,
            config,
            auto_cleanup: true,
            active_pgids: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    /// Runs a command inside the isolated sandbox directory using `config.default_timeout`.
    pub fn run_command(&self, cmd: &str, args: &[&str]) -> Result<CommandOutput> {
        self.run_command_with_timeout(cmd, args, self.config.default_timeout)
    }

    /// Runs a command inside a dedicated Unix Process Group (`PGID`), capturing bounded stdout/stderr
    /// and unconditionally reaping the entire process group (`kill(-pgid, SIGKILL)`) upon completion or timeout.
    pub fn run_command_with_timeout(
        &self,
        cmd: &str,
        args: &[&str],
        timeout: Option<Duration>,
    ) -> Result<CommandOutput> {
        let start = Instant::now();
        let mut command = Command::new(cmd);
        command.args(args);
        command.current_dir(&self.path);
        command.stdin(Stdio::null());
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }

        // Environment & Port Virtualization
        let sp_tmp = self.path.join(".sp-tmp");
        let _ = fs::create_dir_all(&sp_tmp);
        let assigned_port = self.config.base_port.saturating_add(self.config.port_offset);
        command.env("SAVEPOINT_SANDBOX_ID", &self.id);
        command.env("SAVEPOINT_PORT_OFFSET", self.config.port_offset.to_string());
        command.env("PORT", assigned_port.to_string());
        command.env("TMPDIR", &sp_tmp);

        if self.config.share_build_cache {
            let cache_base = self.storage_root.join("shared-cache");
            let cargo_target = cache_base.join("cargo-target");
            let npm_cache = cache_base.join("npm-cache");
            let pip_cache = cache_base.join("pip-cache");
            let _ = fs::create_dir_all(&cargo_target);
            let _ = fs::create_dir_all(&npm_cache);
            let _ = fs::create_dir_all(&pip_cache);
            command.env("CARGO_TARGET_DIR", &cargo_target);
            command.env("NPM_CONFIG_CACHE", &npm_cache);
            command.env("PIP_CACHE_DIR", &pip_cache);

            // Auto-wire sccache if installed on host and RUSTC_WRAPPER is not already set
            if std::env::var_os("RUSTC_WRAPPER").is_none() && has_executable_in_path("sccache") {
                command.env("RUSTC_WRAPPER", "sccache");
            }
        }

        // Network Egress Guard (Side-Effect Shield)
        if self.config.block_network_egress {
            command.env("SAVEPOINT_OFFLINE_MODE", "1");
            command.env("HTTP_PROXY", "http://127.0.0.1:1");
            command.env("HTTPS_PROXY", "http://127.0.0.1:1");
            command.env("ALL_PROXY", "http://127.0.0.1:1");
            command.env("http_proxy", "http://127.0.0.1:1");
            command.env("https_proxy", "http://127.0.0.1:1");
            command.env("NO_PROXY", "localhost,127.0.0.1,::1");
            command.env("no_proxy", "localhost,127.0.0.1,::1");
        }

        // Jobserver / Compiler Thread Governor per Sandbox
        if let Some(jobs) = self.config.build_jobs_limit {
            let clamped = jobs.max(1);
            command.env("CARGO_BUILD_JOBS", clamped.to_string());
            command.env("RAYON_NUM_THREADS", clamped.to_string());
            command.env("MAKEFLAGS", format!("-j{}", clamped));
        }

        for (k, v) in &self.config.extra_env {
            command.env(k, v);
        }

        let mut child = command.spawn().map_err(|e| {
            SavepointError::Other(format!(
                "Failed to spawn '{}' in sandbox {:?}: {}",
                cmd, self.path, e
            ))
        })?;

        let pgid = child.id() as i32;
        if let Ok(mut guard) = self.active_pgids.lock() {
            guard.insert(pgid);
        }

        let max_bytes = self.config.max_output_bytes;
        let stdout_pipe = child.stdout.take();
        let stderr_pipe = child.stderr.take();

        let stdout_handle = std::thread::spawn(move || drain_bounded(stdout_pipe, max_bytes));
        let stderr_handle = std::thread::spawn(move || drain_bounded(stderr_pipe, max_bytes));

        let mut timed_out = false;
        let exit_code = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.code().unwrap_or(-1),
                Ok(None) => {
                    if let Some(limit) = timeout {
                        if start.elapsed() >= limit {
                            timed_out = true;
                            let _ = child.kill();
                            kill_process_group(pgid);
                            let _ = child.wait();
                            break 124;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => {
                    let _ = child.kill();
                    kill_process_group(pgid);
                    let mut guard = match self.active_pgids.lock() {
                        Ok(g) => g,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    guard.remove(&pgid);
                    return Err(SavepointError::Other(format!(
                        "Error waiting for command '{}' in sandbox {:?}: {}",
                        cmd, self.path, e
                    )));
                }
            }
        };

        // Always reap the entire process group so background daemons spawned by the command cannot leak
        kill_process_group(pgid);
        {
            let mut guard = match self.active_pgids.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.remove(&pgid);
        }

        let stdout = stdout_handle.join().unwrap_or_default();
        let stderr = stderr_handle.join().unwrap_or_default();
        let duration_ms = start.elapsed().as_millis();

        Ok(CommandOutput {
            exit_code,
            stdout,
            stderr,
            duration_ms,
            timed_out,
        })
    }

    /// Computes the deltas produced inside the sandbox compared to the original source.
    pub fn compute_deltas(&self) -> Result<Vec<FileDelta>> {
        compute_deltas(&self.source, &self.path)
    }

    /// Atomically syncs/promotes changes from this sandbox back to the destination workspace via CoW `clone_tree`.
    pub fn promote_to_workspace(&self, dest_workspace: &Path) -> Result<Vec<FileDelta>> {
        let deltas = compute_deltas(dest_workspace, &self.path)?;

        for delta in &deltas {
            let target_file = dest_workspace.join(&delta.relative_path);
            let sandbox_file = self.path.join(&delta.relative_path);

            match delta.kind {
                ChangeKind::Created | ChangeKind::Modified => {
                    if sandbox_file.symlink_metadata().is_ok() {
                        clone_tree(&sandbox_file, &target_file)?;
                    }
                }
                ChangeKind::Deleted => {
                    if target_file.symlink_metadata().is_ok() {
                        remove_any_path(&target_file)?;
                    }
                }
            }
        }

        Ok(deltas)
    }

    /// Manually reaps any active process groups and cleans up the sandbox directory.
    pub fn destroy(mut self) -> Result<()> {
        self.auto_cleanup = false;
        self.reap_all_pgids();
        if self.path.symlink_metadata().is_ok() {
            remove_any_path(&self.path)?;
        }
        Ok(())
    }

    fn reap_all_pgids(&self) {
        let mut guard = match self.active_pgids.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        for &pgid in guard.iter() {
            kill_process_group(pgid);
        }
        guard.clear();
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.reap_all_pgids();
        if self.auto_cleanup && self.path.symlink_metadata().is_ok() {
            let _ = remove_any_path(&self.path);
        }
    }
}

fn kill_process_group(pgid: i32) {
    #[cfg(unix)]
    if pgid > 1 {
        unsafe {
            let _ = libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

fn has_executable_in_path(bin_name: &str) -> bool {
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path_var).any(|dir| dir.join(bin_name).is_file())
}

fn drain_bounded<R: Read>(pipe: Option<R>, max_bytes: usize) -> String {
    let Some(mut reader) = pipe else {
        return String::new();
    };
    let mut captured = Vec::new();
    let mut scratch = [0u8; 8192];
    loop {
        match reader.read(&mut scratch) {
            Ok(0) => break,
            Ok(n) => {
                if captured.len() < max_bytes {
                    let remaining = max_bytes - captured.len();
                    let take = n.min(remaining);
                    captured.extend_from_slice(&scratch[..take]);
                }
                // Continue reading and discarding excess bytes so the child never blocks on a full pipe buffer
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&captured).to_string()
}

fn remove_any_path(p: &Path) -> Result<()> {
    let meta = p.symlink_metadata().map_err(|e| SavepointError::Io {
        path: p.to_path_buf(),
        source: e,
    })?;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        fs::remove_dir_all(p).map_err(|e| SavepointError::Io {
            path: p.to_path_buf(),
            source: e,
        })
    } else {
        fs::remove_file(p).map_err(|e| SavepointError::Io {
            path: p.to_path_buf(),
            source: e,
        })
    }
}
