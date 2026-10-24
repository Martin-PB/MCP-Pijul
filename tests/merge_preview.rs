// pijul_merge_preview (backlog 1.3): protocol-level tests that do NOT
// require the pijul binary — argument validation and tool exposure.
// Repository-level behaviour (real channels, real patch sets) is covered by
// the integration suite once the CI harness (backlog 1.2) is merged.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

struct Server(Child);

impl Server {
    fn start() -> Self {
        let cmd = Command::new(env!("CARGO_BIN_EXE_mcp-server-pijul"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn mcp-server-pijul");
        Server(cmd)
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
fn merge_preview_exposed_in_tools_list() {
    let mut s = Server::start();
    let resp = s.request(1, "tools/list", json!({}));
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    let tool = tools
        .iter()
        .find(|t| t["name"] == "pijul_merge_preview")
        .expect("pijul_merge_preview listed");
    assert_eq!(tool["inputSchema"]["type"], "object");
    assert_eq!(
        tool["inputSchema"]["required"][0], "source",
        "source must be a required argument"
    );
}

#[test]
fn merge_preview_requires_source() {
    let mut s = Server::start();
    let resp = s.call_tool(1, "pijul_merge_preview", json!({}));
    let text = Server::result_text(&resp);
    assert!(
        resp["result"]["isError"].as_bool().unwrap_or(false),
        "missing source must be an error"
    );
    assert!(
        text.contains("source"),
        "error should explain the missing `source` argument, got: {text}"
    );
}

#[test]
fn merge_preview_same_channel_rejected_before_any_pijul_call() {
    let mut s = Server::start();
    let resp = s.call_tool(1, "pijul_merge_preview", json!({"source": "main", "target": "main"}));
    let text = Server::result_text(&resp);
    assert!(
        resp["result"]["isError"].as_bool().unwrap_or(false),
        "source == target must be an error"
    );
    assert!(
        text.contains("nothing to merge"),
        "same-channel call should be rejected up front, got: {text}"
    );
}

#[test]
fn merge_preview_pijul_failure_is_reported_not_crashed() {
    // With a channel that does not exist, the pijul call fails either at
    // spawn (no binary) or inside pijul (unknown channel). Either way the
    // server must answer with isError instead of crashing the connection.
    let mut s = Server::start();
    let resp = s.call_tool(1, "pijul_merge_preview", json!({"source": "no-such-channel"}));
    assert!(
        resp["result"]["isError"].as_bool().unwrap_or(false),
        "unknown channel must be reported as isError"
    );
    // The connection must still be alive and synchronous.
    let ping = s.request(2, "ping", json!({}));
    assert!(ping["result"].is_object(), "server must stay responsive");
}
