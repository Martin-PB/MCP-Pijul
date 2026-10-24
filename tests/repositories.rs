//! Integration tests against real generated Pijul repositories.
//!
//! These tests require the `pijul` binary on PATH and skip themselves with
//! a clear message otherwise (the `unit` CI job runs without pijul; the
//! `integration` job installs it). No network: everything happens in
//! tempdirs.

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

fn pijul_available() -> bool {
    Command::new("pijul")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Run `pijul <args...>` in `dir`, asserting success and returning stdout.
fn pijul(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("pijul")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn pijul");
    assert!(
        out.status.success(),
        "pijul {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Minimal repository: init + one file + one recorded change.
fn scratch_repo() -> PathBuf {
    // into_path(): the path must outlive the setup phase; leaked on purpose.
    let dir = tempfile::tempdir().unwrap().into_path();
    pijul(&dir, &["init"]);
    std::fs::write(dir.join("hello.txt"), "A\nB\n").unwrap();
    pijul(&dir, &["add", "."]);
    pijul(&dir, &["record", "--all", "--message", "base"]);
    dir
}

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

    fn tool(&mut self, id: u64, name: &str, args: Value) -> Value {
        self.rpc(id, "tools/call", serde_json::json!({
            "name": name,
            "arguments": args
        }))
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
    let _ = s.rpc(1, "initialize", Value::Null);
    s.send(&serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    s
}

#[test]
fn status_of_a_fresh_repository_succeeds() {
    if !pijul_available() {
        eprintln!("SKIP: pijul not on PATH");
        return;
    }
    let dir = scratch_repo();
    let mut s = initialized_server();
    let resp = s.tool(2, "pijul_status", serde_json::json!({ "repo_path": dir.to_str().unwrap() }));
    assert_eq!(resp["result"]["isError"], Value::Null, "status succeeds");
}

#[test]
fn log_sees_the_recorded_base_change() {
    if !pijul_available() {
        eprintln!("SKIP: pijul not on PATH");
        return;
    }
    let dir = scratch_repo();
    let mut s = initialized_server();
    let resp = s.tool(3, "pijul_log", serde_json::json!({ "repo_path": dir.to_str().unwrap() }));
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("base"), "the recorded message appears in log");
}

#[test]
fn diff_then_record_roundtrip_produces_a_new_change() {
    if !pijul_available() {
        eprintln!("SKIP: pijul not on PATH");
        return;
    }
    let dir = scratch_repo();
    std::fs::write(dir.join("hello.txt"), "A\nX\nB\n").unwrap();
    let mut s = initialized_server();
    // diff shows the unrecorded change
    let diff = s.tool(4, "pijul_diff", serde_json::json!({ "repo_path": dir.to_str().unwrap() }));
    let text = diff["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("X"), "unrecorded line appears in diff");
    // record it
    let rec = s.tool(5, "pijul_record", serde_json::json!({
        "repo_path": dir.to_str().unwrap(),
        "message": "insert X"
    }));
    assert_eq!(rec["result"]["isError"], Value::Null, "record succeeds");
    // log now contains the new message
    let log = s.tool(6, "pijul_log", serde_json::json!({ "repo_path": dir.to_str().unwrap() }));
    let text = log["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("insert X"));
}

#[test]
fn channels_create_switch_roundtrip() {
    if !pijul_available() {
        eprintln!("SKIP: pijul not on PATH");
        return;
    }
    let dir = scratch_repo();
    let mut s = initialized_server();
    let mk = s.tool(7, "pijul_channel_create", serde_json::json!({
        "repo_path": dir.to_str().unwrap(),
        "name": "feature-x"
    }));
    assert_eq!(mk["result"]["isError"], Value::Null, "channel create succeeds");
    let list = s.tool(8, "pijul_channel_list", serde_json::json!({ "repo_path": dir.to_str().unwrap() }));
    let text = list["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("feature-x"), "new channel is listed");
    let sw = s.tool(9, "pijul_channel_switch", serde_json::json!({
        "repo_path": dir.to_str().unwrap(),
        "name": "feature-x"
    }));
    assert_eq!(sw["result"]["isError"], Value::Null, "channel switch succeeds");
}
