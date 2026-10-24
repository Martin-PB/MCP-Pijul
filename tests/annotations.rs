// Annotations MCP (reliquat backlog 1.4) : chaque tool doit exposer des
// hints coherents avec son role reel :
//   - readOnlyHint: true uniquement pour les 5 outils d'inspection ;
//   - destructiveHint: true uniquement pour pijul_unrecord ;
//   - openWorldHint: true uniquement pour pijul_pull / pijul_push.
// Les hints ne remplacent PAS la garde PIJUL_MCP_READONLY (le test de la
// garde vit dans tests/readonly.rs) : ils informent le client.

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
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

const INSPECTION_TOOLS: [&str; 5] = [
    "pijul_status",
    "pijul_log",
    "pijul_diff",
    "pijul_channel_list",
    "pijul_conflict_show",
];
const WRITE_TOOLS: [&str; 7] = [
    "pijul_record",
    "pijul_channel_create",
    "pijul_channel_switch",
    "pijul_apply",
    "pijul_unrecord",
    "pijul_pull",
    "pijul_push",
];
const NETWORK_TOOLS: [&str; 2] = ["pijul_pull", "pijul_push"];

#[test]
fn annotations_match_tool_semantics() {
    let mut s = Server::start();
    let resp = s.request(1, "tools/list", json!({}));
    let tools = resp["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 13, "12 tools + pijul_merge_preview");

    for t in tools {
        let name = t["name"].as_str().expect("tool name");
        let a = &t["annotations"];
        assert!(a.is_object(), "{name} doit exposer des annotations, obtenu : {a}");

        let read_only = INSPECTION_TOOLS.contains(&name);
        // merge_preview : hint faux honnête, car apply=true mute la cible.
        let expect_ro = read_only && name != "pijul_merge_preview";
        assert_eq!(
            a["readOnlyHint"].as_bool(),
            Some(expect_ro),
            "readOnlyHint incorrect pour {name} (attendu {expect_ro})"
        );
        assert_eq!(
            a["readOnlyHint"].as_bool().unwrap_or(false),
            !WRITE_TOOLS.contains(&name) && name != "pijul_merge_preview",
            "{name} : readOnlyHint doit être cohérent avec la garde WRITE_TOOLS du serveur"
        );

        let expect_destructive = name == "pijul_unrecord";
        assert_eq!(
            a["destructiveHint"].as_bool(),
            Some(expect_destructive),
            "destructiveHint incorrect pour {name}"
        );

        let expect_open = NETWORK_TOOLS.contains(&name);
        assert_eq!(
            a["openWorldHint"].as_bool(),
            Some(expect_open),
            "openWorldHint incorrect pour {name}"
        );
    }

    // Le 13e outil (merge_preview) est explicitement couvert par la boucle :
    // il ne doit PAS se déclarer read-only malgré son comportement par défaut.
    let mp = tools.iter().find(|t| t["name"] == "pijul_merge_preview").unwrap();
    assert_eq!(mp["annotations"]["readOnlyHint"].as_bool(), Some(false));
}
