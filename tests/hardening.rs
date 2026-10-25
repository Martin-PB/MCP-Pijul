// Hardening tests (production readiness): pijul subprocess timeout,
// repo_path validation, and protocol version negotiation.
//
// These tests run WITHOUT the real pijul binary: the timeout test installs
// a fake pijul shell script that sleeps, and the validation tests are
// rejected before any subprocess is spawned. Unix-only (like the repo).

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

struct Server(Child);

impl Server {
    fn start(envs: &[(&str, Option<&str>)]) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcp-server-pijul"));
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
        for (k, v) in envs {
            match v {
                Some(v) => {
                    cmd.env(k, v);
                }
                None => {
                    cmd.env_remove(k);
                }
            }
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
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn result_text(resp: &Value) -> String {
    resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn nonexistent_repo_path_is_rejected_before_spawn() {
    let mut s = Server::start(&[("PIJUL_MCP_ALLOWED_ROOTS", None)]);
    let resp = s.call_tool(
        2,
        "pijul_status",
        json!({"repo_path": "/definitely/not/a/real/pijul/path"}),
    );
    let text = result_text(&resp);
    assert!(
        resp["result"]["isError"].as_bool().unwrap_or(false),
        "must be an error"
    );
    assert!(
        text.contains("does not exist"),
        "rejection must mention the missing path, got: {text}"
    );
}

#[test]
fn allowlist_rejects_paths_outside_its_roots() {
    let root_a = tempfile::tempdir().expect("tempdir A");
    let outside = tempfile::tempdir().expect("tempdir B");
    let mut s = Server::start(&[(
        "PIJUL_MCP_ALLOWED_ROOTS",
        Some(root_a.path().to_str().unwrap()),
    )]);
    let args = json!({"repo_path": outside.path().to_str().unwrap()});
    let resp = s.call_tool(2, "pijul_status", args);
    let text = result_text(&resp);
    assert!(
        resp["result"]["isError"].as_bool().unwrap_or(false),
        "outside path must be rejected"
    );
    assert!(
        text.contains("PIJUL_MCP_ALLOWED_ROOTS"),
        "rejection must name the allowlist, got: {text}"
    );
}

#[test]
fn allowlist_accepts_paths_inside_its_roots() {
    let root = tempfile::tempdir().expect("tempdir");
    let mut s = Server::start(&[(
        "PIJUL_MCP_ALLOWED_ROOTS",
        Some(root.path().to_str().unwrap()),
    )]);
    let args = json!({"repo_path": root.path().to_str().unwrap()});
    let resp = s.call_tool(2, "pijul_status", args);
    let text = result_text(&resp);
    // The path is allowlisted, so the call proceeds and may then fail for
    // its own reasons (pijul missing, or the dir not being a repository).
    // What must NOT happen is the allowlist rejection.
    assert!(
        !text.contains("PIJUL_MCP_ALLOWED_ROOTS"),
        "inside path must not be rejected by the allowlist, got: {text}"
    );
}

#[test]
fn initialize_confirms_the_supported_protocol_version() {
    let mut s = Server::start(&[]);
    let resp = s.request(1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    assert_eq!(resp["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn initialize_falls_back_to_its_version_when_client_differs() {
    let mut s = Server::start(&[]);
    let resp = s.request(1, "initialize", json!({"protocolVersion": "1999-01-01"}));
    assert_eq!(resp["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn hung_pijul_subprocess_is_killed_on_timeout() {
    // A fake pijul that hangs forever; the server must kill it after
    // PIJUL_MCP_TIMEOUT_SECS and answer isError instead of hanging.
    let dir = tempfile::tempdir().expect("tempdir");
    let fake = dir.path().join("pijul");
    std::fs::write(&fake, "#!/bin/sh\nexec /bin/sleep 60\n").expect("write fake pijul");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
        .expect("chmod +x fake pijul");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mcp-server-pijul"));
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .env("PATH", dir.path())
        .env("PIJUL_MCP_TIMEOUT_SECS", "1")
        .env_remove("PIJUL_MCP_ALLOWED_ROOTS");
    let mut child = cmd.spawn().expect("spawn server");
    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{}}}}"#
        )
        .unwrap();
        stdin.flush().unwrap();
    }
    let stdout = child.stdout.as_mut().unwrap();
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .expect("initialize response");

    let started = std::time::Instant::now();
    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"pijul_status","arguments":{{}}}}}}"#
        )
        .unwrap();
        stdin.flush().unwrap();
    }
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .expect("tool response must arrive (server must not hang)");
    let resp: Value = serde_json::from_str(line.trim()).expect("valid JSON-RPC");
    assert_eq!(resp["result"]["isError"], true, "timeout must surface as isError");
    assert!(
        result_text(&resp).contains("timed out"),
        "error must mention the timeout, got: {}",
        result_text(&resp)
    );
    assert!(started.elapsed().as_secs() < 30, "server must not hang");
    let _ = child.kill();
    let _ = child.wait();
}
