# ochna 🌳

`ochna` is a local codebase intelligence CLI. It recursively parses **Rust**, **Go**, **Java**, **C**, **C++**, and **Zig** source files using Tree-sitter ASTs, indexes symbols and call edges into a local SQLite database, and provides high-performance search and dependency-graph queries with minimal overhead.

---

## 🚀 Key Features

*   **Fast Indexing**: Scans and parses files recursively, using content hashes to only re-index modified files.
*   **Confidence-Aware Call Graph**: Traces callers and callees structurally across files. Each edge is resolved through a staged cascade (exact qualified hint → receiver type → package/namespace → same file → unique name) and tagged with a confidence score; ambiguous name-only matches are kept as unresolved references instead of polluting the graph with low-confidence edges.
*   **FTS5 Full-Text Search**: Instantly searches signatures, symbols, and docstrings via SQLite's FTS5 engine.
*   **Git Baseline Mapping**: Links indexed database states with Git metadata (current commit SHA, branch, status), ensuring queries are matched against a known codebase version.
*   **Machine-Readable Output**: Accepts a global `--json` flag to emit structured JSON for programmatic consumption (diagnostics and progress go to `stderr`).
*   **Agent-Friendly Workflow Guide**: `ochna howto` teaches the recommended search -> callers -> node investigation flow and emits a JSON capability descriptor for automation.

---

## 🛠 Installation

Install `ochna` directly from [crates.io](https://crates.io/crates/ochna):

```bash
cargo install ochna
```

To build and install from source:

```bash
git clone https://github.com/azusachino/ochna.git
cd ochna
cargo install --path .
```

---

## 🎯 Quick Start

### 1. Initialize the index
Create a local database at `<workspace_root>/.ochna/ochna.db` and perform the initial scan:
```bash
ochna init
```
By default, generated/library directories such as `target`, `node_modules`, `.venv`, `vendor`, `build`, and `dist` are skipped. Use `ochna init --include-library` to index them.

### 2. Keep it in sync
Incrementally update the index after code changes (only modified files are re-parsed):
```bash
ochna sync
```

### 3. Check freshness
Display the index statistics and Git baseline, or gate automation on `status --json`:
```bash
ochna status
ochna status --json   # exits non-zero and reports an `action` when stale/unusable
```

### 4. Learn the query flow
`ochna howto` is the canonical, always-current reference for the query commands
(`search`, `callers`, `node`, `explore`) and their flags. It stays in sync with the
installed binary, so this README intentionally does not duplicate it:
```bash
ochna howto          # human-readable workflow
ochna howto --json   # machine-readable capability descriptor
```

---

## 🧰 Development

Run the agent-facing CLI smoke tests against a release build (drives the real
binary and asserts behavior, not just exit codes):
```bash
make verify-clis
```

---

## 📜 License

This project is licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.
