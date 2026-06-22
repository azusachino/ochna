# ochna 🌳

`ochna` is a local codebase intelligence CLI. It recursively parses **Rust**, **Go**, **Java**, **C**, **C++**, and **Zig** source files using Tree-sitter ASTs, indexes symbols and call edges into a local SQLite database, and provides high-performance search and dependency-graph queries with minimal overhead.

---

## 🚀 Key Features

*   **Fast Indexing**: Scans and parses files recursively, using content hashes to only re-index modified files.
*   **Call Graph Resolution**: Traces callers and callees structurally across files to map codebase dependencies.
*   **FTS5 Full-Text Search**: Instantly searches signatures, symbols, and docstrings via SQLite's FTS5 engine.
*   **Git Baseline Mapping**: Links indexed database states with Git metadata (current commit SHA, branch, status), ensuring queries are matched against a known codebase version.
*   **Machine-Readable Output**: Accepts a global `--json` flag to emit structured JSON for programmatic consumption (diagnostics and progress go to `stderr`).

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

## 🎯 Quick Start & Command Guide

### 1. Initialize Index
Create a local database at `<workspace_root>/.ochna/ochna.db` and perform the initial scan:
```bash
ochna init
```
By default, generated/library directories such as `target`, `node_modules`, `.venv`, `vendor`, `build`, and `dist` are skipped. Use `ochna init --include-library` to index them.

### 2. Update/Sync Index
Incrementally update the index after code changes (only modified files are re-parsed):
```bash
ochna sync
```
Use `ochna sync --include-library` if the index should include generated/library directories.

### 3. Check Statistics
Display details about the indexed database and the Git commit baseline:
```bash
ochna status
```

### 4. Search Symbols
Search for symbols (names, comments, signatures) using full-text search:
```bash
ochna search <query_term>
```
Query commands accept global `--no-tests` to hide symbols classified from test paths.

### 5. Trace Callers
Trace who invokes/calls a specific function or constructor across the project:
```bash
ochna callers <symbol_name_or_id>
```

### 6. Inspect Symbol or File Node
Inspect definitions, code slices, and local scopes:
```bash
# View metadata and implementation source of a symbol
ochna node --symbol <name> --include-code

# View symbol list of a file
ochna node --file <path_to_file> --symbols-only
```

### 7. Explore Codebase
Unified view combining search, file scopes, code snippets, and call graph relationships:
```bash
ochna explore <query_term>
```

---

## 📜 License

This project is licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.
