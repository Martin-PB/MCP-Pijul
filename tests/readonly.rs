// Read-only mode (backlog 1.4): PIJUL_MCP_READONLY must hard-block every
// mutating tool BEFORE any pijul subprocess is spawned. These tests therefore
// run without the pijul binary — the guard is protocol-level, not CLI-level.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

struct Server(Child);

impl Server {
    fn start(read_only: bool) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcp-server-pijul"));
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
        if read_only {
            cmd.env("PIJUL_MCP_READONLY", "1");
        } else {
            cmd.env_remove("PIJUL_MCP_READONLY");
        }
        Server(cmd.spawn().expect("spawn mcp-server-pijul"))
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        {
            let stdin = self.0.stdin.as_mut().expect("stdin open");
            let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
            writeln!(stdin, "{req}").expect("write request");
            stdin.flush().expect("flush request");
        }
        let line = {
            let stdout = self.0.stdout.as_mut().expect("stdout open");
            let mut line = String::new();
            BufReader::new(stdout)
                .read_line(&mut line)
                .expect("read response");
            line
        };
        serde_json::from_str(&line).expect("valid JSON-RPC response")
    }

    fn call_tool(&mut self, id: u64, name: &str, args: Value) -> Value {
        self.request(id, "tools/call", json!({"name": name, "arguments": args}))
    }

    fn result_text(resp: &Value) -> String {
        resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn readonly_rejects_write_tools_before_spawning_pijul() {
    let mut s = Server::start(true);
    // initialize still works in read-only mode.
    let init = s.request(1, "initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "mcp-server-pijul");

    for (i, tool) in [
        "pijul_record",
        "pijul_channel_create",
        "pijul_channel_switch",
        "pijul_apply",
        "pijul_unrecord",
        "pijul_pull",
        "pijul_push",
    ]
    .into_iter()
    .enumerate()
    {
        let resp = s.call_tool(10 + i as u64, tool, json!({}));
        let text = Server::result_text(&resp);
        assert!(
            resp["result"]["isError"].as_bool().unwrap_or(false),
            "{tool} should be an error in read-only mode"
        );
        assert!(
            text.to_lowercase().contains("read-only"),
            "{tool} rejection should mention read-only mode, got: {text}"
        );
        // The guard must fire before pijul is spawned: with pijul absent from
        // PATH in this test environment, a spawn attempt would produce a
        // different message. The read-only message proves no spawn happened.
        assert!(!text.contains("failed to spawn"));
    }
}

#[test]
fn readonly_still_lists_all_tools() {
    let mut s = Server::start(true);
    let resp = s.request(1, "tools/list", json!({}));
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 13, "read-only mode must not hide tools from tools/list");
    assert!(tools.iter().any(|t| t["name"] == "pijul_record"));
    assert!(tools.iter().any(|t| t["name"] == "pijul_status"));
}

#[test]
fn readonly_leaves_inspection_tools_untouched() {
    let mut s = Server::start(true);
    let resp = s.call_tool(1, "pijul_status", json!({}));
    let text = Server::result_text(&resp);
    // Without a pijul binary / repo the call fails for its own reasons — but
    // it must NOT be rejected by the read-only guard.
    assert!(
        !text.to_lowercase().contains("read-only"),
        "pijul_status must not be blocked in read-only mode, got: {text}"
    );
}

#[test]
fn write_tools_available_by_default() {
    let mut s = Server::start(false);
    let resp = s.call_tool(1, "pijul_record", json!({"message": "x"}));
    let text = Server::result_text(&resp);
    assert!(
        !text.to_lowercase().contains("read-only"),
        "without PIJUL_MCP_READONLY the guard must not fire, got: {text}"
    );
}
