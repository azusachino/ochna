# ochna 🌳

`ochna` is a local codebase intelligence CLI. It recursively parses **Java**, **Rust**, and **Go** source files using Tree-sitter ASTs, indexes symbols and relationships into a local SQLite database, and provides high-performance search and dependency-graph queries with minimal token overhead for AI coding agents and developers.

---

## 🚀 Key Features

*   **Language-Agnostic Symbol Separation**: Uses unified `::` separators in SQLite to track functions, methods, structures, interfaces, and constructors across Java, Go, and Rust.
*   **Fast Indexing**: Scans and parses files recursively, using SHA-256 content hashes to only re-index modified files.
*   **FTS5 Full-Text Search**: Instantly searches signatures, symbols, and docstrings via SQLite's virtual FTS5 engine.
*   **Call Graph Resolution**: Traces callers and callees structurally through source code to navigate codebase dependencies.
*   **Git Baseline Mapping**: Links indexed database states with Git metadata (current commit SHA, message, date, branch, and status), ensuring queries are matched against a known codebase version.

---

## 🛠 Installation & Setup

Ensure you have the Rust toolchain installed.

```bash
# Build and install the binary locally to ~/.cargo/bin/ochna
make install
```

To run inside a Nix-managed environment:
```bash
nix develop
# This loads a shell pre-configured with cargo, rustc, zig, and cargo-zigbuild.
```

---

## 🎯 Command Guide

### 1. Initialize / Re-Index
Creates a local database at `<workspace_root>/.ochna/ochna.db` and indexes the project.
```bash
ochna init
```

### 2. Check Database Status & Git Baseline
Displays stats about the indexed database and the Git commit baseline that the index represents.
```bash
ochna status
```

### 3. Structural FTS Search
Search for symbols (not just text matches) across names, comments, and signatures:
```bash
ochna search <query_term>
```

### 4. Trace Dependency Callers
Trace who invokes/calls a specific function or constructor:
```bash
ochna callers <symbol_name_or_id>
```

### 5. Inspect Symbol or File Node
Inspect definitions, code slices, and local scopes:
```bash
# View metadata and implementation source of a symbol
ochna node --symbol <name> --include-code

# View symbol list of a file
ochna node --file <path_to_file> --symbols-only
```

### 6. Semantic Exploration
Combines search, grouped file scopes, code snippets, and call graph relationships:
```bash
ochna explore <query_term>
```

---

## 🧪 Test-Driven Development (TDD)

`ochna` is built with a TDD-first architecture ensuring clean test fixtures and high coverage. The test suites are divided into:
*   **AST Parsers** (`src/parser.rs`): Direct unit tests parsing mock strings of Go, Rust, and Java code, verifying that nodes and caller/callee edges are generated properly.
*   **Database Layers** (`src/db.rs`): Runs migrations and FTS queries against thread-safe, in-memory SQLite instances (`Connection::open_in_memory()`), validating integrity without touching disk.
*   **CLI Integration** (`src/commands.rs`): Spawns temporary workspace directories and scans mock files to verify file hash caching and deleted file pruning.

To run the complete test suite:
```bash
cargo test
```

---

## 📊 Performance & Benchmarking

In large-scale codebases, `ochna` completes structural indexing and queries in milliseconds. Below is the baseline report evaluated on the `tokio` repository (784 files, 7,759 nodes, 5,121 edges):

### 1. Cold Start Indexing (Full parse & write)
```bash
# Deletes index database and parses/indexes everything from scratch
rm -rf .ochna && hyperfine --runs 3 "ochna init"
# Result: 5.291 s ±  0.142 s
```

### 2. Warm Start Indexing (Incremental / Hash check)
```bash
# Performs fast hash-based incremental checks (0 files changed)
hyperfine --runs 5 "ochna init"
# Result: 45.1 ms ±   2.9 ms  (117x speedup!)
```

### 3. Query Latency: Symbol Search
```bash
hyperfine --runs 10 "ochna search Builder"
# Result: 2.1 ms ±   0.7 ms
```

### 4. Query Latency: Callers Trace
```bash
hyperfine --runs 10 "ochna callers new_multi_thread"
# Result: 2.6 ms ±   1.6 ms
```

---

## 🐍 Python Integration (uv + Python 3.14)

`ochna` includes a companion Python utility configured via `uv` using Python 3.14. Since `ochna` stores its structural data in a standard, local SQLite database, you can write Python scripts to run custom analysis directly.

### Running the Python Companion Tool
Ensure you have `uv` installed, then run:
```bash
uv run python pyscripts/report.py
```
This script connects to the local `.ochna/ochna.db` index and generates a structured codebase report:
*   Project baseline details (git commit SHA, branch, status, indexing timestamp).
*   File statistics (counts, language breakdown, total sizes).
*   Top files by symbol density.
*   Total symbol breakdowns by kind.
*   Top 5 incoming call hotspots.

---

## 📜 License

This project is licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.
