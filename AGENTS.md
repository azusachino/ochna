# AGENTS.md — ochna

Project-specific conventions. Cross-project defaults (commits, indentation, tooling) live in the global config and asobi `ToolPreferences`/`CodingStyle`; don't duplicate them here.

## What ochna is

A structural code-graph CLI: parses Rust/Go/Java with Tree-sitter, indexes symbols and call edges into SQLite, and serves `search` / `callers` / `node` / `explore` queries. Layout: `src/main.rs` (clap CLI), `src/commands.rs` (subcommand impls), `src/db.rs` (schema + queries), `src/parser.rs` (per-language AST traversal).

## Commands

- `make build` / `make test` — cargo build (release) / cargo test.
- `make check` — `fmt` (check) + `clippy -D warnings`. Runs before commits.
- `make install` — install to `~/.cargo/bin`. Run this to exercise the CLI.
- `make setup` — init submodules, uv venv, build, index the `clones/`.

## Using ochna itself

- Exercise the CLI as an **installed global** (`make install`, then `ochna <cmd>`) — not via the target-dir binary path.
- **ochna resolves its `.ochna/ochna.db` from the current working directory.** To investigate a project (e.g. a `clones/` submodule), `cd` into it once, then run `ochna` bare. To target a workspace without `cd`-ing, pass the global `--workspace <PATH>` (`-C <PATH>`) flag — e.g. `ochna -C clones/tokio search Runtime`.
- Investigate flow: `ochna search <name>` → `ochna callers <name>` (who calls it) / `ochna callees <name>` (what it calls, for top-down walks) → `ochna node --symbol <name> --include-code` (or `ochna explore <query>` for the combined view). Reach for this for symbol and call-graph lookups; pair it with `rg` (free-text/regex) and `ast-grep` (structural patterns) — the tools complement each other rather than compete.
- For symbols whose name collides across packages (e.g. `worker`, `Run`), scope resolution with `--in <path-prefix>` — e.g. `ochna callees worker --in pkg/controller/podautoscaler`. `search` is ranked (exact → prefix → substring) and capped by `--limit` (default 30); output shows the receiver-qualified name (`Type::method`).
- Run command sequences plainly — no decorated echo banners around invocations.

## Test giants

`clones/tokio` (Rust), `clones/netty` (Java), `clones/kubernetes` (Go), `clones/linux` (C), and `clones/zig` (Zig/C/C++) are git submodules used as real-world index targets and benchmark baselines.
They intentionally stay as submodules; `.gitmodules` uses `ignore = dirty` so generated/untracked files inside the clones do not pollute parent `git status`, while recorded submodule commit changes still show up.
