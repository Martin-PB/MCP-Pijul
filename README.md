# mcp-server-pijul

A [Model Context Protocol](https://modelcontextprotocol.io) server that lets AI agents operate
[Pijul](https://pijul.org) repositories — natively.

Pijul is a distributed version control system built on a mathematically sound theory of patches:
independent changes commute, merges are correct by construction, and conflicts are a
first-class state rather than a merge failure. This makes Pijul a particularly good fit for
agentic SDLC workflows where multiple agents work in parallel on the same repository —
no rebase ceremony, no history rewriting, an immutable audit trail.

This server exposes the Pijul **CLI** as MCP tools, so agents can read, record, and
synchronize Pijul repositories from any MCP-compatible client (Claude Code / Claude Desktop,
Cursor, Windsurf, etc.).

## Tools

| Tool | Kind | Description |
|---|---|---|
| `pijul_status` | read | Working copy status |
| `pijul_log` | read | Recorded changes (patches) in the current channel |
| `pijul_diff` | read | Unrecorded working-copy changes |
| `pijul_record` | write | Record all pending changes as a new patch (returns the hash) |
| `pijul_channel_list` | read | List channels (Pijul's branches) |
| `pijul_channel_create` | write | Fork a new channel |
| `pijul_channel_switch` | write | Switch the working copy to a channel |
| `pijul_apply` | write | Apply a patch by hash prefix, wherever its dependencies exist |
| `pijul_unrecord` | write (destructive) | Remove a patch from the channel (working copy untouched) |
| `pijul_pull` | write (network) | Pull from a remote (Nest or other) |
| `pijul_push` | write (network) | Push to a remote, optionally to a target channel |
| `pijul_merge_preview` | read (preview) / write (apply=true) | Preview (or perform) a channel merge, with ready-to-apply order and conflict hints |
| `pijul_conflict_show` | read | List unresolved conflicts (a state, not a failure) |

All tools accept an optional `repo_path` argument (default: the server's working directory).

## Configuration

All configuration is via environment variables:

| Variable | Default | Effect |
|---|---|---|
| `PIJUL_MCP_READONLY` | unset | When set to a truthy value, mutating tools (`pijul_record`, `pijul_apply`, `pijul_pull`, `pijul_push`, …, and `pijul_merge_preview apply=true`) are rejected **before** any pijul subprocess is spawned |
| `PIJUL_MCP_TIMEOUT_SECS` | 60 | Wall-clock budget per pijul subprocess; on expiry the subprocess is killed and the tool call returns an `isError` result instead of hanging the server |
| `PIJUL_MCP_ALLOWED_ROOTS` | unset | Colon-separated allowlist of repository roots; when set, every `repo_path` argument must resolve under one of the roots (after symlink canonicalization) |

## Operational guarantees

- **No hung server.** Every pijul subprocess runs under a wall-clock timeout with draining
  reader threads (a chatty pijul can never fill the pipe buffer); on expiry the subprocess
  is killed and the caller gets a clear `isError`.
- **Validated paths.** `repo_path` arguments must exist and be directories; with
  `PIJUL_MCP_ALLOWED_ROOTS` set, they must live inside the allowlist. Validation happens
  before any subprocess is spawned.
- **Read-only mode is protocol-level.** The guard rejects mutating tools up front, so the
  guarantee holds even on machines where pijul is not installed.
- **Protocol negotiation.** `initialize` confirms the client's protocol version when
  supported, and otherwise responds with the version the server implements
  (`2025-06-18`).
- **Security model.** stdio transport, single local client: anyone who can talk to the
  server can run pijul on the allowed repositories with the server's permissions. For
  untrusted or multi-tenant hosting, run the server per-client, read-only, with a tight
  `PIJUL_MCP_ALLOWED_ROOTS`.

## Installation

Requires [Pijul](https://pijul.org) ≥ 1.0 on `PATH`.

```sh
cargo install --git https://github.com/Martin-PB/MCP-Pijul
```

### Claude Desktop / Claude Code

The server takes no command-line arguments; configuration is via environment variables,
and the repository is chosen per tool call (`repo_path`) or by launching the server from
the repository directory:

```json
{
  "mcpServers": {
    "pijul": {
      "command": "mcp-server-pijul",
      "env": {
        "PIJUL_MCP_ALLOWED_ROOTS": "/home/me/work:/home/me/repos",
        "PIJUL_MCP_TIMEOUT_SECS": "120"
      }
    }
  }
}
```

For a strictly observational agent, set `"PIJUL_MCP_READONLY": "1"` in the same `env` block.

### Cursor / Windsurf

Same `command`/`env` shape; pass `repo_path` per call or start the server in the
repository you want the agent to work on.

## Local testing (Linux)

`scripts/local-check.sh` runs the full local verification: build, protocol and hardening
tests, and — when `pijul` (≥ 1.0.0-beta) is on `PATH` — the annotations, readonly,
merge_preview and repository integration tests, plus a live stdio smoke test: it spawns
the freshly built server against a throwaway Pijul repository, drives a real MCP session
(initialize, `tools/list`, `pijul_status`, `pijul_merge_preview`), and checks that the
`PIJUL_MCP_READONLY` guard rejects mutating tools. No network access is needed beyond
crates.io for the first build.

```sh
bash scripts/local-check.sh
```

(Invoke it via `bash`: some commit routes do not preserve the executable bit; `chmod +x`
works too.)

Without `pijul` on `PATH`, the script degrades to the no-pijul subset, exactly like the
CI unit job.

## Design principles

1. **The CLI is the contract.** Pijul's command-line interface is its most stable
   surface; this server shells out to `pijul` rather than linking `libpijul`.
2. **Patch-native vocabulary.** Tools expose *changes* and *channels*, not
   branches and rebases. Agents working through this server learn Pijul's model,
   not a Git emulation of it.
3. **Conflicts are states.** `pijul_conflict_show` returns a resolvable state of
   the dependency graph; the agent can continue working and resolve it by
   recording a new change.
4. **Minimal dependencies.** Only `serde`/`serde_json`; the JSON-RPC stdio loop
   is hand-rolled, so the binary stays small and auditable.
5. **Fail closed, fail loud.** Timeouts kill the subprocess and surface as
   `isError`; path validation, the read-only guard, and version negotiation all
   reject before any side effect.

## Roadmap

- [ ] Warn at startup when the installed pijul CLI is older than the version the CI targets
- [ ] Nest integration: list changes, discussions, and apply patches on https://nest.pijul.org
- [ ] Publish to crates.io

## Contributing

Issues and pull requests welcome. Patches created automatically by linters are
subject to maintainer review.

## License

MIT — see [LICENSE](LICENSE).
