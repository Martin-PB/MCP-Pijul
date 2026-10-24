//! Protocol tests for the MCP stdio server.
//!
//! These tests drive the real binary over stdio (CARGO_BIN_EXE_* is set by
//! cargo for integration tests). They cover the JSON-RPC/MCP framing and
//! the graceful-degradation contract, and they run WITHOUT the `pijul`
//! binary in PATH — tools that need pijul must degrade to isError results
//! with a clear message, which is itself part of the contract.
//!
//! Tests that exercise real pijul repositories live in
//! `tests/repositories.rs` and skip themselves when `pijul` is not
//! available (CI installs it in the `integration` job).

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

struct Server {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Server {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-server-pijul"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to spawn server binary");
        let stdin = child.stdin.take().expect("no stdin");
        let stdout = BufReader::new(child.stdout.take().expect("no stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send(&mut self, request: &Value) {
        writeln!(self.stdin, "{request}").expect("write request");
        self.stdin.flush().expect("flush");
    }

    fn recv(&mut self) -> Value {
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read response");
        assert!(!line.trim().is_empty(), "server closed without responding");
        serde_json::from_str(line.trim()).expect("response is valid JSON")
    }

    fn rpc(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }));
        let resp = self.recv();
        assert_eq!(resp["id"], id, "response id matches request id");
        resp
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn initialized_server() -> Server {
    let mut s = Server::start();
    let resp = s.rpc(1, "initialize", Value::Null);
    assert!(resp["result"]["protocolVersion"].is_string());
    s.send(&serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    s
}

#[test]
fn initialize_advertises_tools_capability() {
    let mut s = Server::start();
    let resp = s.rpc(7, "initialize", Value::Null);
    let result = &resp["result"];
    assert_eq!(result["protocolVersion"], "2025-06-18");
    assert_eq!(result["serverInfo"]["name"], "mcp-server-pijul");
    assert!(result["capabilities"]["tools"].is_object());
}

#[test]
fn ping_answers_empty_object() {
    let mut s = initialized_server();
    let resp = s.rpc(8, "ping", Value::Null);
    assert!(resp["result"].is_object());
}

#[test]
fn tools_list_exposes_the_twelve_tools() {
    let mut s = initialized_server();
    let resp = s.rpc(9, "tools/list", Value::Null);
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 12, "exactly the 12 documented tools");
    assert_eq!(tools.len(), 13, "12 tools + pijul_merge_preview (backlog 1.3)");
    for t in tools {
        assert!(t["name"].as_str().unwrap().starts_with("pijul_"));
        assert!(t["description"].as_str().unwrap().len() > 20);
        assert_eq!(t["inputSchema"]["type"], "object");
    }
}

#[test]
fn tool_call_without_pijul_binary_degrades_to_iserror() {
    // Force a PATH where the pijul binary cannot be found for the child.
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-server-pijul"))
        .env("PATH", "/nonexistent-mcp-pijul-test-path")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#).unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).expect("initialize response");
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"pijul_status","arguments":{{}}}}}}"#
    )
    .unwrap();
    stdin.flush().unwrap();
    let mut line = String::new();
    stdout.read_line(&mut line).expect("response line");
    let resp: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(resp["result"]["isError"], true);
    assert!(
        resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("pijul"),
        "error message names the missing binary"
    );
    let _ = child.kill();
}

#[test]
fn unknown_tool_returns_iserror() {
    let mut s = initialized_server();
    let resp = s.rpc(10, "tools/call", serde_json::json!({
        "name": "pijul_inexistant",
        "arguments": {}
    }));
    assert_eq!(resp["result"]["isError"], true);
}

#[test]
fn unknown_method_returns_jsonrpc_error_32601() {
    let mut s = initialized_server();
    let resp = s.rpc(11, "resources/list", Value::Null);
    assert_eq!(resp["error"]["code"], -32601);
}

#[test]
fn invalid_json_lines_are_ignored_and_stream_stays_usable() {
    let mut s = Server::start();
    writeln!(s.stdin, "ceci n'est pas du json").unwrap();
    s.stdin.flush().unwrap();
    // A garbage line must produce NO response; the next valid request must
    // get the next response line, proving the stream stayed in sync.
    let resp = s.rpc(12, "ping", Value::Null);
    assert_eq!(resp["id"], 12);
}

#[test]
fn notifications_without_id_get_no_response() {
    let mut s = Server::start();
    s.send(&serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    // No response may be emitted; the next request/response pair proves it.
    let resp = s.rpc(13, "ping", Value::Null);
    assert_eq!(resp["id"], 13);
    assert!(resp["result"].is_object());
}
