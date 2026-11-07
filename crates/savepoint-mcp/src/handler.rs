use crate::protocol::{JsonRpcRequest, JsonRpcResponse};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use savepoint_fs::{FlightRecorder, ShadowEngine};
use savepoint_speculate::{
    ConvergencePolicy, SpeculationConfig, SpeculativeCandidate, SpeculativeEngine,
};

pub struct McpHandler;

impl McpHandler {
    pub fn handle_request(req: JsonRpcRequest) -> Option<JsonRpcResponse> {
        let id = match req.id {
            Some(id) => id,
            None => return None, // Notification, no response needed
        };

        match req.method.as_str() {
            "ping" => Some(JsonRpcResponse::success(id, json!({}))),
            "initialize" => {
                let res = json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "savepoint-mcp",
                        "version": "0.1.0"
                    }
                });
                Some(JsonRpcResponse::success(id, res))
            }
            "tools/list" => {
                let tools = json!({
                    "tools": [
                        {
                            "name": "savepoint_begin",
                            "description": "Start an ACID transaction runtime on workspace for zero-copy rollback safety",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": { "type": "string", "description": "Workspace root directory (defaults to current dir)" }
                                }
                            }
                        },
                        {
                            "name": "savepoint_checkpoint",
                            "description": "Create an instant sub-millisecond CoW snapshot checkpoint before or after making code changes",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "message": { "type": "string", "description": "Intent or description of the checkpoint" },
                                    "path": { "type": "string", "description": "Workspace root directory" }
                                }
                            }
                        },
                        {
                            "name": "savepoint_rollback",
                            "description": "Instantly revert the workspace to a previous checkpoint or step",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "step": { "type": "integer", "description": "Step index to rewind to (omit to undo 1 step)" },
                                    "path": { "type": "string", "description": "Workspace root directory" }
                                }
                            }
                        },
                        {
                            "name": "savepoint_fork",
                            "description": "Fork a speculative branch from the current checkpoint to experiment safely",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "branch": { "type": "string", "description": "New branch name" },
                                    "path": { "type": "string", "description": "Workspace root directory" }
                                },
                                "required": ["branch"]
                            }
                        },
                        {
                            "name": "savepoint_switch",
                            "description": "Switch workspace to a specific branch",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "branch": { "type": "string", "description": "Branch name to switch to" },
                                    "path": { "type": "string", "description": "Workspace root directory" }
                                },
                                "required": ["branch"]
                            }
                        },
                        {
                            "name": "savepoint_merge",
                            "description": "Merge changes from a source branch into a target branch",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "source_branch": { "type": "string", "description": "Source branch" },
                                    "target_branch": { "type": "string", "description": "Target branch (default: main)" },
                                    "path": { "type": "string", "description": "Workspace root directory" }
                                },
                                "required": ["source_branch"]
                            }
                        },
                        {
                            "name": "savepoint_speculate",
                            "description": "Autonomous Speculative Engine: Run multiple candidate solutions in parallel CoW sandboxes, test each, and auto-converge on the winner",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "candidates": {
                                        "type": "array",
                                        "items": {
                                            "type": "object",
                                            "properties": {
                                                "name": { "type": "string" },
                                                "setup_commands": { "type": "array", "items": { "type": "string" } }
                                            },
                                            "required": ["name", "setup_commands"]
                                        }
                                    },
                                    "validation_cmd": { "type": "string", "description": "Test command (e.g. 'cargo test', 'npm test')" },
                                    "policy": { "type": "string", "enum": ["best_pass_wins", "first_pass_wins"], "description": "Convergence policy (default: best_pass_wins)" },
                                    "max_concurrency": { "type": "integer", "description": "Optional max parallel sandboxes (auto-governed by CPU/RAM if omitted)" },
                                    "path": { "type": "string", "description": "Workspace root directory" }
                                },
                                "required": ["candidates", "validation_cmd"]
                            }
                        },
                        {
                            "name": "savepoint_status",
                            "description": "Inspect active transaction status, branches, checkpoints, and uncommitted changes",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "path": { "type": "string", "description": "Workspace root directory" }
                                }
                            }
                        },
                        {
                            "name": "savepoint_export_bundle",
                            "description": "Export a secret-scrubbed portable .spbundle Flight Recorder file for bug reproduction or sharing",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "output": { "type": "string", "description": "Output .spbundle file path (default: savepoint.spbundle)" },
                                    "path": { "type": "string", "description": "Workspace root directory" }
                                }
                            }
                        },
                        {
                            "name": "savepoint_replay_bundle",
                            "description": "Replay a .spbundle Flight Recorder file into a target directory at a specific checkpoint step",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "bundle": { "type": "string", "description": "Path to the .spbundle file" },
                                    "target_path": { "type": "string", "description": "Target directory to restore files into" },
                                    "step": { "type": "integer", "description": "Checkpoint step index to replay (omit for latest step)" }
                                },
                                "required": ["bundle"]
                            }
                        }
                    ]
                });
                Some(JsonRpcResponse::success(id, tools))
            }
            "tools/call" => {
                let params = req.params.unwrap_or(Value::Null);
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(Value::Null);

                let result = Self::execute_tool(tool_name, args);
                match result {
                    Ok(text) => {
                        let res = json!({
                            "content": [
                                {
                                    "type": "text",
                                    "text": text
                                }
                            ]
                        });
                        Some(JsonRpcResponse::success(id, res))
                    }
                    Err(err) => Some(JsonRpcResponse::error(id, -32000, err)),
                }
            }
            _ => Some(JsonRpcResponse::error(id, -32601, "Method not found")),
        }
    }

    fn execute_tool(name: &str, args: Value) -> Result<String, String> {
        let ws_path = args
            .get("path")
            .and_then(|p| p.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));

        match name {
            "savepoint_begin" => {
                let tx = ShadowEngine::begin_transaction(&ws_path)
                    .map_err(|e| format!("Failed to begin transaction: {}", e))?;
                Ok(format!(
                    "Savepoint transaction '{}' started successfully for {:?}. Pristine baseline checkpoint #0 established on branch 'main'.",
                    tx.id.as_str(),
                    tx.workspace_root
                ))
            }
            "savepoint_checkpoint" => {
                let mut tx = Self::get_active_tx(&ws_path)?;
                let msg = args.get("message").and_then(|m| m.as_str());
                let cp = ShadowEngine::checkpoint(&mut tx, msg, msg)
                    .map_err(|e| format!("Failed to checkpoint: {}", e))?;
                Ok(format!(
                    "Checkpoint #{} captured in branch '{}'. Modified files: {}. Snapshot: {:?}",
                    cp.step_index,
                    cp.branch,
                    cp.deltas.len(),
                    cp.snapshot_path
                ))
            }
            "savepoint_rollback" => {
                let mut tx = Self::get_active_tx(&ws_path)?;
                if let Some(step) = args.get("step").and_then(|s| s.as_u64()) {
                    ShadowEngine::rewind(&mut tx, step as usize)
                        .map_err(|e| format!("Failed to rewind: {}", e))?;
                    Ok(format!("Workspace rolled back to checkpoint #{}.", step))
                } else {
                    let rolled = ShadowEngine::rewind_previous(&mut tx)
                        .map_err(|e| format!("Failed to rewind previous: {}", e))?;
                    match rolled {
                        Some(target) => Ok(format!("Workspace rolled back 1 step to checkpoint #{}.", target)),
                        None => Ok("Already at baseline checkpoint #0. Nothing to rewind.".to_string()),
                    }
                }
            }
            "savepoint_fork" => {
                let mut tx = Self::get_active_tx(&ws_path)?;
                let branch = args
                    .get("branch")
                    .and_then(|b| b.as_str())
                    .ok_or_else(|| "Missing required 'branch' argument".to_string())?;
                ShadowEngine::fork_branch(&mut tx, branch)
                    .map_err(|e| format!("Failed to fork branch: {}", e))?;
                Ok(format!(
                    "Forked new speculative branch '{}' from step #{}.",
                    branch,
                    tx.active_step()
                ))
            }
            "savepoint_switch" => {
                let mut tx = Self::get_active_tx(&ws_path)?;
                let branch = args
                    .get("branch")
                    .and_then(|b| b.as_str())
                    .ok_or_else(|| "Missing required 'branch' argument".to_string())?;
                let step = ShadowEngine::switch_branch(&mut tx, branch)
                    .map_err(|e| format!("Failed to switch branch: {}", e))?;
                Ok(format!("Switched to branch '{}' at step #{}.", branch, step))
            }
            "savepoint_merge" => {
                let mut tx = Self::get_active_tx(&ws_path)?;
                let src = args
                    .get("source_branch")
                    .and_then(|b| b.as_str())
                    .ok_or_else(|| "Missing required 'source_branch' argument".to_string())?;
                let target = args
                    .get("target_branch")
                    .and_then(|b| b.as_str())
                    .unwrap_or("main");
                let cp = ShadowEngine::merge_branch(&mut tx, src, target)
                    .map_err(|e| format!("Failed to merge branch: {}", e))?;
                Ok(format!(
                    "Successfully merged branch '{}' into '{}' at checkpoint #{}.",
                    src, target, cp.step_index
                ))
            }
            "savepoint_speculate" => {
                let candidates_val = args.get("candidates").ok_or("Missing 'candidates' array")?;
                let candidates: Vec<SpeculativeCandidate> = serde_json::from_value(candidates_val.clone())
                    .map_err(|e| format!("Invalid candidates schema: {}", e))?;
                let validation_cmd = args
                    .get("validation_cmd")
                    .and_then(|v| v.as_str())
                    .ok_or("Missing 'validation_cmd'")?;

                let policy = match args.get("policy").and_then(|p| p.as_str()) {
                    Some("first_pass_wins") => ConvergencePolicy::FirstPassWins,
                    _ => ConvergencePolicy::BestPassWins,
                };
                let max_concurrency = args
                    .get("max_concurrency")
                    .and_then(|c| c.as_u64())
                    .map(|c| c as usize);

                let cfg = SpeculationConfig {
                    policy,
                    max_concurrency,
                    ..Default::default()
                };

                let summary = SpeculativeEngine::evaluate_with_config(&ws_path, candidates, validation_cmd, cfg)
                    .map_err(|e| format!("Speculative execution error: {}", e))?;

                Ok(serde_json::to_string_pretty(&summary).unwrap_or_default())
            }
            "savepoint_export_bundle" => {
                let output = args
                    .get("output")
                    .and_then(|o| o.as_str())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| ws_path.join("savepoint.spbundle"));
                let stats = FlightRecorder::export_bundle(&ws_path, &output)
                    .map_err(|e| format!("Failed to export .spbundle: {}", e))?;
                Ok(serde_json::to_string_pretty(&stats).unwrap_or_default())
            }
            "savepoint_replay_bundle" => {
                let bundle = args
                    .get("bundle")
                    .and_then(|b| b.as_str())
                    .map(PathBuf::from)
                    .ok_or_else(|| "Missing required 'bundle' argument".to_string())?;
                let target_path = args
                    .get("target_path")
                    .and_then(|t| t.as_str())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| ws_path.clone());
                let step = args.get("step").and_then(|s| s.as_u64()).map(|s| s as usize);
                let stats = FlightRecorder::replay_bundle(&bundle, &target_path, step)
                    .map_err(|e| format!("Failed to replay .spbundle: {}", e))?;
                Ok(serde_json::to_string_pretty(&stats).unwrap_or_default())
            }
            "savepoint_status" => {
                let tx = Self::get_active_tx(&ws_path)?;
                let deltas = ShadowEngine::compute_live_deltas(&tx).unwrap_or_default();
                let status_info = json!({
                    "transaction_id": tx.id.as_str(),
                    "active_branch": tx.active_branch,
                    "active_step": tx.active_step(),
                    "total_checkpoints": tx.checkpoints.len(),
                    "branches": tx.branch_names(),
                    "uncommitted_deltas": deltas.len()
                });
                Ok(serde_json::to_string_pretty(&status_info).unwrap_or_default())
            }
            _ => Err(format!("Unknown tool: {}", name)),
        }
    }

    fn get_active_tx(ws: &Path) -> Result<savepoint_core::Transaction, String> {
        ShadowEngine::load_active_transaction(ws)
            .map_err(|e| format!("Error checking active transaction: {}", e))?
            .ok_or_else(|| "No active Savepoint transaction found. Call 'savepoint_begin' first.".to_string())
    }
}
