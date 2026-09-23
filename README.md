# Receipts

Review and explore your agentic chats across Claude Code, Codex and other agents, all in one local, searchable place.

- **One index for every agent.** Reads `~/.claude/projects` and `~/.codex/sessions` directly. No accounts, no uploads.
- **Full-text search** across prompts, replies, thinking, tool calls and tool output (SQLite FTS5).
- **Keeps your receipts.** Transcripts are copied into Receipts' own database, so they survive when an agent prunes old logs. Claude Code deletes them after 30 days by default.
- **Single binary.** The Rust server embeds the React UI. It runs on macOS, Linux and Windows.

## Install

```sh
curl -fsSL https://github.com/joel-huang/receipts/releases/latest/download/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://github.com/joel-huang/receipts/releases/latest/download/install.ps1 | iex
```

Options: `RECEIPTS_VERSION=v0.1.0`, `RECEIPTS_INSTALL_DIR=/usr/local/bin`.

## Use

```sh
receipts                 # index, start the UI at http://127.0.0.1:7878 and open it
receipts serve --no-open --port 9000
receipts search "migration rollback"
receipts index           # index only
receipts remote devbox   # browse the chats on another machine over SSH, in this browser
receipts where           # print the database path
receipts update          # update to the newest release
```

Each time it runs, `receipts` checks GitHub for a newer release. If it finds one, it asks before it
updates.
Set `RECEIPTS_NO_UPDATE=1` to turn the check off.

| Variable | Default |
| --- | --- |
| `CLAUDE_CONFIG_DIR` | `~/.claude` |
| `CODEX_HOME` | `~/.codex` |
| `RECEIPTS_NO_UPDATE` | unset (set it to turn off the update check) |
| `RECEIPTS_HOME` | platform data dir (`~/Library/Application Support/receipts`, `~/.local/share/receipts`, `%APPDATA%\receipts`) |

`receipts remote <target>` takes any SSH target, such as `user@host` or a `Host` from `~/.ssh/config`.
It installs Receipts on that machine if needed, starts it there on `127.0.0.1`, and opens it here
through an SSH tunnel. The remote chats stay on the remote machine. Extra SSH options go after
`--`, such as `receipts remote devbox -- -p 2222`.

The server binds to `127.0.0.1` and rejects any non-localhost `Host` header, which blocks DNS-rebinding attacks.

## Develop

Requires Rust (stable) and Node 20+.

```sh
cd web && npm install && npm run build && cd ..   # build the UI once so it can be embedded
cargo run -- serve --no-open                       # API + embedded UI on :7878

# UI with hot reload (proxies /api to :7878):
cd web && npm run dev

# Share the dev server on your tailnet at https://<machine>.<tailnet>.ts.net:
tailscale serve --bg 5173
```

### Layout

```
src/
  main.rs            CLI (clap)
  server.rs          axum HTTP API + embedded static assets
  index.rs           SQLite schema, incremental indexing, FTS search
  model.rs           Session/Message types shared by all sources
  sources.rs         `Source` trait: discover() + parse()
  sources/
    claude.rs        Claude Code JSONL
    codex.rs         Codex CLI rollout JSONL
web/                 Vite + React + TypeScript UI
install.sh, install.ps1
.github/workflows/   CI + tag-triggered cross-platform release
```

### Adding an agent

Implement `sources::Source` (discover the log files and parse one into a `SessionMeta` plus `Vec<Message>`), then register it in `sources::all()`. Indexing, search and the UI pick it up automatically.

## Release

Push a tag like `v0.1.0`. The release workflow builds the UI, compiles for macOS (arm64/x86_64), Linux musl (x86_64/arm64) and Windows, and attaches tarballs, `.sha256` checksums, `install.sh` and `install.ps1` to the GitHub release. The installer URL always points at the latest release.
