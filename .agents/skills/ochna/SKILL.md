---
name: ochna
description: Use the ochna CLI to index, search, explore, and trace code call-graphs structurally without expensive recursive grep/read commands.
---

# ochna CLI Playbook

`ochna` is a local codebase intelligence CLI. It parses Java, Rust, and Go files using Tree-sitter AST, indexes them in SQLite, and provides direct query commands.

Use `ochna` BEFORE resorting to standard tools like `rg` or `view_file`.

## Commands Reference

### 1. Indexing & Health
*   **Initialize/Update Index**:
    ```bash
    ochna init
    ```
    *Call this at the start of a session or after editing files to sync the SQLite index.*
*   **Check Index Statistics**:
    ```bash
    ochna status
    ```
*   **List Tracked Files**:
    ```bash
    ochna files
    ```

### 2. Search & Exploration
*   **Concept/Symbol Search**:
    ```bash
    ochna search <query_keyword_or_name>
    ```
    *Performs FTS (Full-Text Search) and name matches. Returns matching symbols with their file paths and line numbers.*
*   **Unified Exploration**:
    ```bash
    ochna explore <query>
    ```
    *Search for matching nodes, groups them by file path, prints their source code snippets, and displays caller/callee relationships in one command.*

### 3. Navigation & Tracing
*   **Find Callers (Incoming References)**:
    ```bash
    ochna callers <symbol_name_or_id>
    ```
    *Lists all call sites of a function, constructor, or method.*
*   **Inspect File (Structure or Content)**:
    *   *Show symbols only*:
        ```bash
        ochna node --file <path> --symbols-only
        ```
    *   *Slice source code*:
        ```bash
        ochna node --file <path> --offset <start_line> --limit <line_count>
        ```
*   **Inspect Symbol (Definition & Context)**:
    *   *Metadata only*:
        ```bash
        ochna node --symbol <name>
        ```
    *   *Metadata & implementation source*:
        ```bash
        ochna node --symbol <name> --include-code
        ```
    *   *Disambiguate by definition line*:
        ```bash
        ochna node --symbol <name> --include-code --line <line_number>
        ```

### 4. Python Database Analysis
For custom queries or advanced analytics directly from the SQLite database:
*   **Generate Structured Report**:
    ```bash
    uv run python pyscripts/report.py
    ```
    *This runs under Python 3.14 and directly extracts file distributions, symbol counts, and hot call sites using `sqlite3` without invoking the binary.*

## Workflow Integration Rules

1.  **Graph First**: For any task, run `ochna explore <keyword>` first to map out the relevant implementation files.
2.  **No Blind Grepping**: Do not run recursive greps (`rg`) for symbol lookups. Run `ochna search <name>` or `ochna callers <name>` instead.
3.  **Read Replacements**: Use `ochna node --file <path>` instead of `view_file` to read source files; it returns line numbers and attaches dependents.
