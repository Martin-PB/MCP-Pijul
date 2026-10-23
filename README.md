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
| `pijul_unrecord` | write | Remove a patch from the channel (working copy untouched) |
| `pijul_pull` | write | Pull from a remote (Nest or other) |
| `pijul_push` | write | Push to a remote, optionally to a target channel |
| `pijul_conflict_show` | read | List unresolved conflicts (a state, not a failure) |

All tools accept an optional `repo_path` argument (default: the server's working directory).

## Installation

Requires [Pijul](https://pijul.org) ≥ 1.0 on `PATH`.

```sh
cargo install --git https://github.com/Martin-PB/MCP-Pijul
```

### Claude Desktop / Claude Code

```json
{
  "mcpServers": {
    "pijul": {
      "command": "mcp-server-pijul",
      "args": ["--repository", "/path/to/repo"]
    }
  }
}
```

### Cursor / Windsurf

Same `command`/`args` shape; point it at the repository you want the agent to work on.

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

## Roadmap

- [ ] `pijul_merge_preview`: show whether a set of patches commutes and which conflicts would remain, *before* touching the working copy
- [ ] Nest integration: list changes, discussions, and apply patches on https://nest.pijul.org
- [ ] Configuration for read-only mode (hooks / approval gates for agentic CI)
- [ ] Integration tests against generated repositories
- [ ] Publish to crates.io

## Contributing

Issues and pull requests welcome. Patches created automatically by linters are
subject to maintainer review.

## License

MIT — see [LICENSE](LICENSE).
