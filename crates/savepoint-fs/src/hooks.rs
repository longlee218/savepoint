use savepoint_core::{Result, SavepointError};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LifecycleHooks {
    #[serde(default)]
    pub pre_checkpoint: Vec<String>,
    #[serde(default)]
    pub post_checkpoint: Vec<String>,
    #[serde(default)]
    pub pre_rollback: Vec<String>,
    #[serde(default)]
    pub post_rollback: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookExecutionReport {
    pub hook_name: String,
    pub command: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl LifecycleHooks {
    /// Loads hooks from workspace root looking for:
    /// 1. `.sp-hooks.yaml`
    /// 2. `.sp-hooks.yml`
    /// 3. `.sp-hooks.json`
    /// 4. `.savepoint/hooks.yaml`
    /// 5. `.savepoint/hooks.json`
    pub fn load(workspace_root: &Path) -> Option<Self> {
        let candidates = [
            workspace_root.join(".sp-hooks.yaml"),
            workspace_root.join(".sp-hooks.yml"),
            workspace_root.join(".sp-hooks.json"),
            workspace_root.join(".savepoint").join("hooks.yaml"),
            workspace_root.join(".savepoint").join("hooks.yml"),
            workspace_root.join(".savepoint").join("hooks.json"),
        ];

        for path in &candidates {
            if path.is_file() {
                if let Ok(content) = fs::read_to_string(path) {
                    if path.extension().and_then(|e| e.to_str()) == Some("json") {
                        if let Ok(hooks) = serde_json::from_str::<LifecycleHooks>(&content) {
                            return Some(hooks);
                        }
                    } else if let Ok(hooks) = serde_yaml::from_str::<LifecycleHooks>(&content) {
                        return Some(hooks);
                    }
                }
            }
        }

        None
    }

    /// Executes a list of hook commands within the workspace directory.
    pub fn run_hooks(
        workspace_root: &Path,
        hook_name: &str,
        commands: &[String],
        env_vars: &[(&str, &str)],
    ) -> Result<Vec<HookExecutionReport>> {
        let mut reports = Vec::new();

        for cmd_str in commands {
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(cmd_str).current_dir(workspace_root);

            for &(key, val) in env_vars {
                cmd.env(key, val);
            }
            cmd.env("SAVEPOINT_HOOK", hook_name);
            cmd.env("SAVEPOINT_WORKSPACE", workspace_root.to_string_lossy().to_string());

            let output = cmd.output().map_err(|e| SavepointError::Other(format!(
                "Failed to execute hook '{}' command '{}': {}",
                hook_name, cmd_str, e
            )))?;

            let success = output.status.success();
            let exit_code = output.status.code();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();

            reports.push(HookExecutionReport {
                hook_name: hook_name.to_string(),
                command: cmd_str.clone(),
                success,
                exit_code,
                stdout,
                stderr,
            });

            if !success {
                tracing::warn!(
                    hook = hook_name,
                    command = cmd_str,
                    exit_code = ?exit_code,
                    "Savepoint hook exited with failure"
                );
            }
        }

        Ok(reports)
    }
}
