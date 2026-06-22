---
name: ochna
description: Use the ochna CLI to index, search, explore, and trace code call-graphs structurally without expensive recursive grep/read commands.
---

# ochna CLI Playbook

`ochna` is a local codebase intelligence CLI. It parses Rust, Go, Java, C, C++, and Zig files using Tree-sitter AST, indexes them in SQLite, and provides direct query commands. Call edges are resolved **across files**, so `callers`/`callees` trace the whole-project call graph; calls to symbols outside the index are recorded as unresolved references rather than dropped.

Use `ochna` BEFORE resorting to standard tools like `rg` or `view_file`.

**Machine-readable output**: every query command accepts a global `--json` flag that emits structured JSON on stdout (full node records with `id`, `qualified_name`, `signature`, line/column spans, plus callers/callees). Diagnostics and progress go to stderr, so `--json` stdout is always clean to parse. Prefer `--json` when consuming output programmatically. Verbosity is controlled by `RUST_LOG` (default `info`).

## Commands Reference

### 1. Indexing & Health

- **Initialize Index**:
  ```bash
  ochna init
  ```
  _Call this at the start of a project/session to create the SQLite index._
  _By default this skips library/generated directories (`target`, `node_modules`, `.venv`, `vendor`, `build`, `dist`); pass `--include-library` to index them._
- **Update/Sync Index**:
  ```bash
  ochna sync
  ```
  _Call this after editing files to incrementally sync code changes into the SQLite index._
  _Use `ochna sync --include-library` when generated/library directories should stay in the index._
- **Check Index Statistics**:
  ```bash
  ochna status
  ```
- **List Tracked Files**:
  ```bash
  ochna files
  ```

### 2. Search & Exploration

- **Concept/Symbol Search**:
  ```bash
  ochna search <query_keyword_or_name>
  ```
  _Performs FTS (Full-Text Search) and name matches. Returns matching symbols with their file paths and line numbers._
  _Add global `--no-tests` to query commands (`search`, `callers`, `node`, `explore`) to hide symbols classified from test paths._
- **Unified Exploration**:
  ```bash
  ochna explore <query>
  ```
  _Search for matching nodes, groups them by file path, prints their source code snippets, and displays caller/callee relationships in one command._

### 3. Navigation & Tracing

- **Find Callers (Incoming References)**:
  ```bash
  ochna callers <symbol_name_or_id>
  ```
  _Lists all call sites of a function, constructor, or method._
- **Inspect File (Structure or Content)**:
  - _Show symbols only_:
    ```bash
    ochna node --file <path> --symbols-only
    ```
  - _Slice source code_:
    ```bash
    ochna node --file <path> --offset <start_line> --limit <line_count>
    ```
- **Inspect Symbol (Definition & Context)**:
  - _Metadata only_:
    ```bash
    ochna node --symbol <name>
    ```
  - _Metadata & implementation source_:
    ```bash
    ochna node --symbol <name> --include-code
    ```
  - _Disambiguate by definition line_:
    ```bash
    ochna node --symbol <name> --include-code --line <line_number>
    ```

### 4. Python Database Analysis

For custom queries or advanced analytics directly from the SQLite database:

- **Generate Structured Report**:
  ```bash
  uv run python pyscripts/report.py
  ```
  _This runs under Python 3.14 and directly extracts file distributions, symbol counts, and hot call sites using `sqlite3` without invoking the binary._

## Workflow Integration Rules

1.  **Graph First**: For any task, run `ochna explore <keyword>` first to map out the relevant implementation files.
2.  **No Blind Grepping**: Do not run recursive greps (`rg`) for symbol lookups. Run `ochna search <name>` or `ochna callers <name>` instead.
3.  **Read Replacements**: Use `ochna node --file <path>` instead of `view_file` to read source files; it returns line numbers and attaches dependents.
