// mcp-server-pijul: a minimal, dependency-light MCP server (stdio transport)
// exposing Pijul version control operations to AI agents.
//
// Design notes:
// - Zero async runtime: MCP over stdio is line-delimited JSON-RPC, a simple
//   read/dispatch/write loop is enough and keeps the binary tiny.
// - Every tool shells out to the `pijul` CLI. Pijul's CLI is its stable
//   interface; libpijul's API moves faster than the CLI.
// - Patch-native philosophy: tools expose changes/channels (Pijul's model),
//   not branches/rebases (Git's model). Agents get commutation and
//   first-class conflicts for free.

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::process::Command;

const PROTOCOL_VERSION: &str = "2025-06-18";
const SERVER_NAME: &str = "mcp-server-pijul";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

struct Tool {
    name: &'static str,
    description: &'static str,
    schema: Value,
}

fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "pijul_status",
            description: "Show the working copy status of a Pijul repository (changed files, pending changes). Read-only.",
            schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": {"type": "string", "description": "Path to the Pijul repository (default: cwd)"}
                }
            }),
        },
        Tool {
            name: "pijul_log",
            description: "List recorded changes (patches) in the current channel, newest first. Read-only.",
            schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": {"type": "string"},
                    "limit": {"type": "integer", "description": "Max number of changes to return (default 50)"}
                }
            }),
        },
        Tool {
            name: "pijul_diff",
            description: "Show the unrecorded changes between the working copy and the recorded state. Read-only.",
            schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": {"type": "string"}
                }
            }),
        },
        Tool {
            name: "pijul_record",
            description: "Stage and record all pending changes as a new Pijul change (patch) with a message. Equivalent of `pijul add` + `pijul record --all --message`. The patch hash is returned.",
            schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": {"type": "string"},
                    "message": {"type": "string", "description": "Change message (required)"}
                },
                "required": ["message"]
            }),
        },
        Tool {
            name: "pijul_channel_list",
            description: "List the channels of a repository (Pijul channels replace Git branches; switching channels does not duplicate the working copy state).",
            schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": {"type": "string"}
                }
            }),
        },
        Tool {
            name: "pijul_channel_create",
            description: "Create (fork) a new channel from the current one, without switching to it. Equivalent of `pijul channel new`.",
            schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": {"type": "string"},
                    "name": {"type": "string"}
                },
                "required": ["name"]
            }),
        },
        Tool {
            name: "pijul_channel_switch",
            description: "Switch the working copy to another channel. Patches recorded afterwards commute: switching is cheap and no rebase is ever needed.",
            schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": {"type": "string"},
                    "name": {"type": "string"}
                },
                "required": ["name"]
            }),
        },
        Tool {
            name: "pijul_apply",
            description: "Apply a recorded change (patch) identified by its hash prefix to the current channel. Patches are first-class: they can be applied wherever their dependencies exist.",
            schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": {"type": "string"},
                    "hash": {"type": "string", "description": "Change hash or unambiguous prefix (from pijul_log)"}
                },
                "required": ["hash"]
            }),
        },
        Tool {
            name: "pijul_unrecord",
            description: "Remove a recorded change from the channel without touching the working copy (the change stays on disk and can be re-applied). Use with care.",
            schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": {"type": "string"},
                    "hash": {"type": "string"}
                },
                "required": ["hash"]
            }),
        },
        Tool {
            name: "pijul_pull",
            description: "Pull changes from a remote (e.g. a Nest repository). Merges commute: independent patches apply in any order. Conflicts are recorded as a state, not a failure.",
            schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": {"type": "string"},
                    "remote": {"type": "string", "description": "Remote name or URL (default: the repo's default remote)"}
                }
            }),
        },
        Tool {
            name: "pijul_push",
            description: "Push recorded changes to a remote. An optional target channel maps to `pijul push --to-channel`.",
            schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": {"type": "string"},
                    "remote": {"type": "string"},
                    "channel": {"type": "string", "description": "Target channel on the remote (optional)"}
                }
            }),
        },
        Tool {
            name: "pijul_merge_preview",
            description: "Preview merging channel `source` into channel `target` (default: the current channel). Lists the changes present on source but missing on target, in ready-to-apply order, WITHOUT touching the repository. Set apply=true to perform the merge (pijul apply --channel: dependencies follow automatically; conflicts are first-class, inspect them with pijul_conflict_show).",
            schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": {"type": "string"},
                    "source": {"type": "string", "description": "Channel to merge FROM"},
                    "target": {"type": "string", "description": "Channel to merge INTO (default: current channel)"},
                    "apply": {"type": "boolean", "description": "Perform the merge instead of only previewing it (default: false)"}
                },
                "required": ["source"]
            }),
        },
        Tool {
            name: "pijul_conflict_show",
            description: "List files containing unresolved conflicts in the working copy. In Pijul conflicts are first-class: they are a state of the graph, resolvable by recording a new change.",
            schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": {"type": "string"}
                }
            }),
        },
    ]
}

fn run_pijul(repo_path: Option<&str>, args: &[&str]) -> (bool, String) {
    let mut cmd = Command::new("pijul");
    cmd.args(args);
    if let Some(p) = repo_path.filter(|s| !s.is_empty()) {
        cmd.current_dir(p);
    }
    match cmd.output() {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.trim().is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(err.trim_end());
            }
            (out.status.success(), text.trim_end().to_string())
        }
        Err(e) => (false, format!("failed to spawn pijul: {e}. Is Pijul installed and on PATH?")),
    }
}

/// Tools that mutate the repository or its remotes. In read-only mode
/// (PIJUL_MCP_READONLY set to a truthy value) these are rejected up front,
/// before any pijul subprocess is spawned, so the guarantee holds even on
/// machines where pijul is not installed.
const WRITE_TOOLS: [&str; 7] = [
    "pijul_record",
    "pijul_channel_create",
    "pijul_channel_switch",
    "pijul_apply",
    "pijul_unrecord",
    "pijul_pull",
    "pijul_push",
];

/// Tools that should carry destructiveHint: true in their MCP annotations
/// (they remove or invalidate already-recorded state).
const DESTRUCTIVE_TOOLS: [&str; 1] = ["pijul_unrecord"];

/// Tools that may interact with the outside world (remotes): openWorldHint.
const NETWORK_TOOLS: [&str; 2] = ["pijul_pull", "pijul_push"];

/// MCP tool annotations (spec 2025-06-18): hints only — the server's
/// own read-only guard (PIJUL_MCP_READONLY) remains the real enforcement.
/// - readOnlyHint: the 5 inspection tools; NOT pijul_merge_preview, whose
///   apply=true variant mutates the target channel.
/// - destructiveHint: pijul_unrecord only.
/// - openWorldHint: pijul_pull / pijul_push (network remotes).
fn annotations_for(name: &str) -> Value {
    json!({
        "readOnlyHint": !WRITE_TOOLS.contains(&name) && name != "pijul_merge_preview",
        "destructiveHint": DESTRUCTIVE_TOOLS.contains(&name),
        "openWorldHint": NETWORK_TOOLS.contains(&name),
    })
}

fn read_only() -> bool {
    std::env::var("PIJUL_MCP_READONLY")
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !v.is_empty() && v != "0" && v != "false" && v != "no"
        })
        .unwrap_or(false)
}

fn call_tool(name: &str, args: &Value) -> Value {
    if read_only() && WRITE_TOOLS.contains(&name) {
        return error_result(&format!(
            "read-only mode: `{name}` is disabled (PIJUL_MCP_READONLY is set). \
             Only inspection tools (pijul_status, pijul_log, pijul_diff, \
             pijul_channel_list, pijul_conflict_show, and pijul_merge_preview in preview-only mode) are available."
        ));
    }
    let repo = args.get("repo_path").and_then(Value::as_str);
    let (ok, out) = match name {
        "pijul_status" => run_pijul(repo, &["status"]),
        "pijul_log" => {
            let (ok, out) = run_pijul(repo, &["log"]);
            match args.get("limit").and_then(Value::as_i64) {
                Some(n) => {
                    let lines: Vec<&str> = out.lines().collect();
                    let n = n.clamp(1, 500) as usize;
                    let cap = lines.len().min(n);
                    (ok, lines[..cap].join("\n").to_string())
                }
                None => (ok, out),
            }
        }
        "pijul_diff" => run_pijul(repo, &["diff"]),
        "pijul_record" => {
            let msg = args
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if msg.trim().is_empty() {
                return error_result("`message` is required for pijul_record");
            }
            // `pijul add .` is idempotent for already-tracked files.
            let (add_ok, add_out) = run_pijul(repo, &["add", "."]);
            if !add_ok && !add_out.contains("already") && !add_out.is_empty() {
                return error_result(&format!("pijul add failed: {add_out}"));
            }
            run_pijul(repo, &["record", "--all", "--message", msg])
        }
        "pijul_channel_list" => run_pijul(repo, &["channel"]),
        "pijul_channel_create" => {
            let Some(n) = args.get("name").and_then(Value::as_str) else {
                return error_result("`name` is required");
            };
            run_pijul(repo, &["channel", "new", n])
        }
        "pijul_channel_switch" => {
            let Some(n) = args.get("name").and_then(Value::as_str) else {
                return error_result("`name` is required");
            };
            run_pijul(repo, &["channel", "switch", n])
        }
        "pijul_apply" => {
            let Some(h) = args.get("hash").and_then(Value::as_str) else {
                return error_result("`hash` is required");
            };
            run_pijul(repo, &["apply", h])
        }
        "pijul_unrecord" => {
            let Some(h) = args.get("hash").and_then(Value::as_str) else {
                return error_result("`hash` is required");
            };
            run_pijul(repo, &["unrecord", h])
        }
        "pijul_pull" => match args.get("remote").and_then(Value::as_str) {
            Some(r) if !r.is_empty() => run_pijul(repo, &["pull", r]),
            _ => run_pijul(repo, &["pull"]),
        },
        "pijul_push" => {
            let remote = args.get("remote").and_then(Value::as_str).unwrap_or("");
            let channel = args.get("channel").and_then(Value::as_str).unwrap_or("");
            match (remote.is_empty(), channel.is_empty()) {
                (true, true) => run_pijul(repo, &["push"]),
                (false, true) => run_pijul(repo, &["push", remote]),
                (true, false) => run_pijul(repo, &["push", "--to-channel", channel]),
                (false, false) => run_pijul(repo, &["push", remote, "--to-channel", channel]),
            }
        }
        "pijul_merge_preview" => return merge_preview(repo, args),
        "pijul_conflict_show" => run_pijul(repo, &["conflict", "show"]),
        other => return error_result(&format!("unknown tool: {other}")),
    };
    if ok {
        text_result(&out)
    } else {
        error_result(&out)
    }
}

use std::collections::HashSet;

/// Hashes recorded on a channel (`pijul log --hash-only --channel`), newest first.
fn channel_hashes(repo: Option<&str>, channel: &str) -> Result<Vec<String>, String> {
    let (ok, out) = run_pijul(repo, &["log", "--hash-only", "--channel", channel]);
    if !ok {
        return Err(format!("pijul log --channel {channel} failed: {out}"));
    }
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

/// Name of the channel currently in use (the one starred by `pijul channel`).
fn current_channel(repo: Option<&str>) -> Result<String, String> {
    let (ok, out) = run_pijul(repo, &["channel"]);
    if !ok {
        return Err(format!("pijul channel failed: {out}"));
    }
    out.lines()
        .map(str::trim)
        .find(|l| l.starts_with('*'))
        .map(|l| l.trim_start_matches('*').trim().to_string())
        .ok_or_else(|| "cannot determine the current channel".to_string())
}

/// pijul_merge_preview: analyse the patch-set difference between two channels
/// (source -> target) without touching anything; optionally apply the
/// missing changes with `pijul apply --channel`, which pulls dependencies
/// along. This mirrors the manual's channel-merge workflow: in Pijul, merge
/// and rebase are the same operation and conflicts are first-class.
fn merge_preview(repo: Option<&str>, args: &Value) -> Value {
    let Some(source) = args
        .get("source")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return error_result("`source` is required for pijul_merge_preview");
    };
    let target = args
        .get("target")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let do_apply = args.get("apply").and_then(Value::as_bool).unwrap_or(false);
    if do_apply && read_only() {
        return error_result(
            "read-only mode (PIJUL_MCP_READONLY): pijul_merge_preview is limited to \
             previewing; apply=true is disabled",
        );
    }

    let target = match target {
        Some(t) => t.to_string(),
        None => match current_channel(repo) {
            Ok(t) => t,
            Err(e) => error_result(&e),
        },
    };
    if source == target {
        return error_result(&format!(
            "source and target are the same channel (\"{source}\"): nothing to merge"
        ));
    }

    let src = match channel_hashes(repo, &source) {
        Ok(h) => h,
        Err(e) => return error_result(&e),
    };
    let dst = match channel_hashes(repo, &target) {
        Ok(h) => h,
        Err(e) => return error_result(&e),
    };
    let dst_set: HashSet<&String> = dst.iter().collect();
    let missing: Vec<&String> = src.iter().filter(|h| !dst_set.contains(*h)).collect();
    let common = src.len() - missing.len();

    // `pijul log` is newest-first; `pijul apply` wants oldest-first so that
    // dependencies are already in place (they would be pulled along anyway,
    // but a deterministic order keeps previews reproducible).
    let oldest_first: Vec<&String> = missing.iter().rev().cloned().collect();

    let mut report = format!(
        "merge preview: channel \"{source}\" -> channel \"{target}\"\n\
         changes on source: {} | already on target: {} | to merge: {}\n",
        src.len(),
        common,
        missing.len()
    );
    if missing.is_empty() {
        report.push_str("nothing to do: every change on source is already on target.\n");
        return text_result(&report);
    }
    report.push_str("changes to apply (oldest first):\n");
    for h in &oldest_first {
        report.push_str(&format!("  {h}\n"));
    }
    if !do_apply {
        report.push_str(
            "preview only (nothing was modified). set apply=true to merge, then use \
             pijul_conflict_show to inspect first-class conflicts.\n",
        );
        return text_result(&report);
    }

    report.push_str("applying...\n");
    let mut failures = 0;
    for h in &oldest_first {
        let (ok, out) = run_pijul(repo, &["apply", "--channel", &target, h]);
        if ok {
            report.push_str(&format!("  applied {h}\n"));
        } else {
            failures += 1;
            report.push_str(&format!("  FAILED {h}: {out}\n"));
        }
    }
    if failures == 0 {
        report.push_str(&format!(
            "merge complete: {}/{} changes applied to channel \"{target}\". \
             use pijul_conflict_show to inspect conflicts, then record the resolution.\n",
            oldest_first.len(),
            oldest_first.len()
        ));
        text_result(&report)
    } else {
        error_result(&format!(
            "{failures} change(s) failed to apply:\n\n{report}"
        ))
    }
}

fn text_result(text: &str) -> Value {
    json!({
        "content": [{"type": "text", "text": if text.is_empty() { "(no output)" } else { text }}]
    })
}

fn error_result(text: &str) -> Value {
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": true
    })
}

fn respond(id: Value, result: Value, out: &mut impl Write) {
    let _ = writeln!(out, "{}", json!({"jsonrpc": "2.0", "id": id, "result": result}));
    let _ = out.flush();
}

fn respond_error(id: Value, code: i64, message: &str, out: &mut impl Write) {
    let _ = writeln!(
        out,
        "{}",
        json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
    );
    let _ = out.flush();
}

fn main() {
    if read_only() {
        eprintln!("mcp-server-pijul: read-only mode (PIJUL_MCP_READONLY) — mutating tools are disabled");
    }
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let id = req.get("id").cloned();
        let params = req.get("params").cloned().unwrap_or(Value::Null);

        // Notifications (no id) are acknowledged by silence.
        let Some(id) = id else { continue };

        match method {
            "initialize" => respond(
                id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION}
                }),
                &mut out,
            ),
            "ping" => respond(id, json!({}), &mut out),
            "tools/list" => {
                let list: Vec<Value> = tools()
                    .into_iter()
                    .map(|t| json!({"name": t.name, "description": t.description, "inputSchema": t.schema, "annotations": annotations_for(t.name)}))
                    .collect();
                respond(id, json!({"tools": list}), &mut out);
            }
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(json!({}));
                let result = call_tool(name, &args);
                respond(id, result, &mut out);
            }
            _ => respond_error(id, -32601, &format!("method not found: {method}"), &mut out),
        }
    }
}
