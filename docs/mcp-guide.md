# Savepoint Model Context Protocol (MCP) Guide

Savepoint includes a native JSON-RPC 2.0 stdio MCP server (`savepoint-mcp`), allowing autonomous agents (Claude Code, Cursor, Windsurf, OpenCode) to control ACID transactions directly.

---

## Starting the MCP Server

```bash
sp mcp
```

---

## Configuration

### Claude Desktop / Claude Code (`~/.claude/claude_desktop_config.json`)
```json
{
  "mcpServers": {
    "savepoint": {
      "command": "sp",
      "args": ["mcp"]
    }
  }
}
```

### Cursor (`.cursor/mcp.json`)
```json
{
  "mcpServers": {
    "savepoint": {
      "command": "sp",
      "args": ["mcp"]
    }
  }
}
```

---

## Available MCP Tools

| Tool | Parameters | Description |
|:---|:---|:---|
| `savepoint_begin` | `path` (string) | Starts an ACID transaction in the workspace and creates baseline step #0. |
| `savepoint_checkpoint` | `message` (string), `path` (string) | Takes a sub-millisecond CoW snapshot of current turn changes. |
| `savepoint_rewind` | `to` (integer), `path` (string) | Rewinds the workspace to a specific checkpoint step. |
| `savepoint_status` | `path` (string) | Returns transaction state, active branch, and checkpoint timeline. |
| `savepoint_commit` | `path` (string) | Commits the transaction and finalizes changes. |
| `savepoint_speculate` | `candidates` (array), `validation` (string), `policy` (string), `path` (string) | Runs parallel branch candidates across isolated CoW sandboxes with automatic convergence. |
| `savepoint_export_flight_bundle` | `output_path` (string), `step` (optional int), `path` (string) | Exports a secret-scrubbed `.spbundle` flight recorder package. |
| `savepoint_replay_bundle` | `bundle_path` (string), `step` (optional int), `target_path` (string) | Replays a `.spbundle` or `sp://replay/<id>` URI into the target directory. |
