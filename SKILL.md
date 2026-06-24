---
name: ochna
description: Use the ochna CLI to index, search, explore, and trace code call-graphs structurally without expensive recursive grep/read commands.
---

# ochna CLI Playbook

`ochna` is a local codebase intelligence CLI. It parses Rust, Go, Java, C, C++, and Zig files using Tree-sitter AST, indexes them in SQLite, and provides direct query commands. Call edges are resolved **across files**, so `callers`/`callees` trace the whole-project call graph; calls to symbols outside the index are recorded as unresolved references rather than dropped.

Use `ochna` BEFORE resorting to standard tools like `rg` or `view_file`.

**Confidence-aware edges**: call edges carry a resolution kind and a derived
confidence score from a staged cascade — `exact` (100), `receiver_type` (90),
`package` / `namespace` (80), `same_file` (60), `name_only` (30). Ambiguous
name-only matches become unresolved references rather than low-confidence edges.
Add `--show-resolution` to any query to see `[resolution: <kind>, confidence:
<N>]`, and `--min-confidence <N>` to `callers` to drop weak edges (e.g.
`--min-confidence 80` to keep only typed/qualified/namespace-anchored callers).
Use this to cut noise on common method names in large Go/Java corpora.

**Framework routes**: Java Spring MVC controllers are indexed as `route` nodes.
`@Controller` / `@RestController` classes combine class-level
`@RequestMapping` paths with method-level `@GetMapping`, `@PostMapping`,
`@PutMapping`, `@DeleteMapping`, `@PatchMapping`, and `@RequestMapping`
annotations. Route nodes are named like `GET /api/users/{id}` or
`ANY /api/status` and have call edges to their handler methods, so
`ochna explore "/api"` or `ochna callers <handler>` can reveal HTTP entry
points as graph nodes.

**Machine-readable output**: every query command accepts a global `--json` flag that emits structured JSON on stdout (full node records with `id`, `qualified_name`, `signature`, line/column spans, plus callers/callees). Diagnostics and progress go to stderr, so `--json` stdout is always clean to parse. Prefer `--json` when consuming output programmatically. Verbosity is controlled by `RUST_LOG` (default `info`).

**Self-describing workflow**: run `ochna howto` when you need the canonical query flow. Run `ochna howto --json` for a machine-readable capability descriptor. `ochna init`/`ochna sync` also write `.ochna/AGENT.md`, a generated pointer with index provenance and links back to `ochna howto` and `ochna status`.

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
  _Use `ochna status --json` as a preflight gate before automated queries. It reports `ok`, schema match, counts, freshness, Git baseline, and the next `action`; stale or unusable indexes exit non-zero._
- **Learn Query Workflow**:
  ```bash
  ochna howto
  ochna howto --json
  ```
  _Prints the recommended search -> callers -> node flow and operational facts such as cwd-based database resolution._
- **Verify CLI Surfaces (source checkout)**:
  ```bash
  make verify-clis
  ```
  _Runs the `uv`-backed Python real-binary smoke tests for `howto`, `status --json`, stale preflight exit behavior, and generated `.ochna/AGENT.md`._
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
  _Add `--min-confidence <N>` to drop weak edges and `--show-resolution` to print each caller's resolution kind and confidence._
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

- **Explain a GitHub PR against an indexed checkout**:
  ```bash
  uv run python pyscripts/pr_feature_report.py --workspace clones/kubernetes --repo kubernetes/kubernetes --pr 139848
  ```
  _Use this for large benchmark submodules where local history may be shallow. It reads PR metadata and changed files with `gh api`, then reads symbols from the local `.ochna/ochna.db`._

## Workflow Integration Rules

1.  **Graph First**: For any task, run `ochna explore <keyword>` first to map out the relevant implementation files.
2.  **No Blind Grepping**: Do not run recursive greps (`rg`) for symbol lookups. Run `ochna search <name>` or `ochna callers <name>` instead.
3.  **Read Replacements**: Use `ochna node --file <path>` instead of `view_file` to read source files; it returns line numbers and attaches dependents.
4.  **Large PR Archaeology**: For Linux/Kubernetes-style corpora, do not assume local merge parents exist. First verify the index with `ochna status --json`, use `gh api` for PR metadata and changed files, then use `ochna node --file ... --symbols-only --json` and `ochna node --symbol ... --include-code --json` for the changed symbols. Treat common Go method callees such as `GetList`, `Run`, `Add`, and `Stop` as noisy unless they are anchored to the changed file or exact production symbol.
