use savepoint_mcp::protocol::JsonRpcRequest;
use savepoint_mcp::McpHandler;
use serde_json::json;
use std::fs;

#[test]
fn test_mcp_tools_lifecycle_and_bundle_export_replay() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path().join("mcp_ws");
    let repro = temp.path().join("mcp_repro");
    let bundle_file = temp.path().join("mcp.spbundle");
    fs::create_dir_all(&ws).unwrap();
    fs::write(ws.join("code.txt"), "v1").unwrap();

    // 1. ping
    let ping_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(1)),
        method: "ping".to_string(),
        params: None,
    };
    let ping_resp = McpHandler::handle_request(ping_req).unwrap();
    assert!(ping_resp.error.is_none());

    // 2. savepoint_begin
    let begin_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(2)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "savepoint_begin",
            "arguments": { "path": ws.to_string_lossy() }
        })),
    };
    let begin_resp = McpHandler::handle_request(begin_req).unwrap();
    assert!(begin_resp.error.is_none());

    // 3. savepoint_speculate with policy = first_pass_wins
    let spec_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(3)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "savepoint_speculate",
            "arguments": {
                "path": ws.to_string_lossy(),
                "policy": "first_pass_wins",
                "max_concurrency": 2,
                "validation_cmd": "grep -q 'v2_winner' code.txt",
                "candidates": [
                    { "name": "cand_win", "setup_commands": ["echo 'v2_winner' > code.txt"] },
                    { "name": "cand_lose", "setup_commands": ["echo 'v2_loser' > code.txt"] }
                ]
            }
        })),
    };
    let spec_resp = McpHandler::handle_request(spec_req).unwrap();
    assert!(spec_resp.error.is_none());
    assert_eq!(
        fs::read_to_string(ws.join("code.txt")).unwrap().trim(),
        "v2_winner"
    );

    // 4. savepoint_export_bundle
    let export_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(4)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "savepoint_export_bundle",
            "arguments": {
                "path": ws.to_string_lossy(),
                "output": bundle_file.to_string_lossy()
            }
        })),
    };
    let export_resp = McpHandler::handle_request(export_req).unwrap();
    assert!(export_resp.error.is_none());
    assert!(bundle_file.exists());

    // 5. savepoint_replay_bundle (replay step 0 -> baseline "v1")
    let replay_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(5)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "savepoint_replay_bundle",
            "arguments": {
                "bundle": bundle_file.to_string_lossy(),
                "target_path": repro.to_string_lossy(),
                "step": 0
            }
        })),
    };
    let replay_resp = McpHandler::handle_request(replay_req).unwrap();
    assert!(replay_resp.error.is_none());
    assert_eq!(
        fs::read_to_string(repro.join("code.txt")).unwrap().trim(),
        "v1"
    );

    // 6. Invalid policy or max_concurrency = 0 must return an error response
    let bad_policy_req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(json!(6)),
        method: "tools/call".to_string(),
        params: Some(json!({
            "name": "savepoint_speculate",
            "arguments": {
                "path": ws.to_string_lossy(),
                "policy": "invalid_policy_name",
                "validation_cmd": "true",
                "candidates": [
                    { "name": "c1", "setup_commands": ["true"] }
                ]
            }
        })),
    };
    let bad_policy_resp = McpHandler::handle_request(bad_policy_req).unwrap();
    assert!(
        bad_policy_resp.error.is_some(),
        "Expected invalid policy to return JSON-RPC error"
    );
}
