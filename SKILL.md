---
name: ochna
description: Use the ochna CLI to index, search, explore, and trace code call-graphs structurally — a complement to text tools like rg and ast-grep for symbol and call-edge questions.
---

# ochna CLI Playbook

`ochna` is a local codebase intelligence CLI. It parses Rust, Go, Java, C, C++, and Zig files using Tree-sitter AST, indexes them in SQLite, and provides direct query commands. Call edges are resolved **across files**, so `callers`/`callees` trace the whole-project call graph; calls to symbols outside the index are recorded as unresolved references rather than dropped.

`ochna`, `rg`, and `ast-grep` are complementary — reach for the one that fits the
question: `ochna` for symbol and call-graph queries (definitions, callers/callees,
cross-file edges), `rg` for free-text/regex occurrences, and `ast-grep` for
structural AST patterns or rewrites. The win is using `ochna` for the graph
instead of reconstructing call edges by hand with `rg`, not avoiding `rg`.

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
`ANY /api/status` and have `route_handler` edges to their handler methods, so
`ochna explore "/api"` or `ochna callers <handler>` can reveal HTTP entry
points as graph nodes.

**Machine-readable output**: every query command accepts a global `--json` flag that emits structured JSON on stdout (full node records with `id`, `qualified_name`, `signature`, line/column spans, plus callers/callees). Diagnostics and progress go to stderr, so `--json` stdout is always clean to parse. Prefer `--json` when consuming output programmatically. Verbosity is controlled by `RUST_LOG` (default `info`).

**Self-describing workflow**: run `ochna howto` when you need the canonical query flow. Run `ochna howto --json` for a machine-readable capability descriptor. `ochna init`/`ochna sync` also write `.ochna/AGENT.md`, a generated pointer with index provenance and links back to `ochna howto` and `ochna status`.

**Structural review workflow (v0.3)**: `doctor` → `diff` → `impact` → `tests-for` → `unresolved` is the diff-aware review flow, frozen in `docs/v0.3-contract.md`. `doctor` is the review preflight (graph `trust_verdict`, resolution tiers, unresolved/collision/low-quality diagnostics; exits nonzero when degraded or unusable — a large monorepo like Netty or Kubernetes legitimately reports `degraded`, so treat it as a scale signal, not necessarily a defect). `diff (--base <rev> [--head <rev>] | --files <path>...)` maps a Git range or explicit paths to changed symbols/edges; historical ranges use disposable temporary indexes and honestly report `base_revision_unavailable` on a shallow clone rather than guessing. `impact <symbol> [--depth <n>] [--direction callers|callees|both] [--min-confidence <n>]` is a bounded, confidence-labelled blast radius (default depth 2, min-confidence 30, max depth 5/200 nodes/400 edges) that reports `affected_tests` and `unresolved_boundaries`, and refuses to guess across an ambiguous bare name (e.g. a 9-way `GetList` collision) rather than exploding into an unbounded common-name traversal. `tests-for <symbol>` labels each result `direct_call`, `same_module_candidate`, or `name_heuristic` and never claims proof of coverage — a production symbol whose only test coverage is _indirect_ (a JUnit wrapper that triggers it through lifecycle, not a direct call) correctly returns an empty `tests` array; this is a documented tradeoff, not a bug, see `docs/experiments/v0.3-acceptance-netty.md`. `unresolved [--in <path-prefix>] [--limit <n>]` lists call sites that couldn't become a trusted edge, each with an explicit reason. All five accept `--workspace`/`-C`, `--json`, `--no-tests`, and emit the same `contract_version: "0.3"` JSON envelope.

## Command Surface

Run `ochna howto` (or `ochna howto --json` for a capability descriptor) for the
full, always-current command and flag reference — it is the single source of
truth and stays in sync with the installed binary, so this playbook does not
re-list every command. The flow is `status` → `search` → `callers`/`callees` →
`node`, with `explore` for a combined view; the review flow above is
`doctor` → `diff` → `impact` → `tests-for` → `unresolved`. Judgment notes
specific to this playbook:

- `--show-resolution` / `--min-confidence <N>` on `callers` apply the confidence
  cascade above; use `--min-confidence 80` to cut noise on common Go/Java method
  names.
- `callees <symbol>` walks the call graph forward (what a symbol calls) — use it
  for top-down dives, the mirror of `callers`. `search` is ranked best-first and
  capped by `--limit` (default 30), and output shows the receiver-qualified name
  (`Type::method`).
- `--in <path-prefix>` on `callers`/`callees` scopes target resolution to symbols
  under a path, disambiguating bare names that collide across packages (e.g.
  `worker`, `Run`); pairs well with `--min-confidence` for noise reduction.
- Global `--workspace <PATH>` (`-C <PATH>`) targets a workspace's
  `.ochna/ochna.db` from any cwd — no need to `cd` into the project first.
- `ochna node --file <path> --symbols-only` and `--offset/--limit` replace
  `view_file` for structural reads; `--include-code [--line <n>]` returns a
  symbol's source and disambiguates overloads.
- `--include-library` on `init`/`sync` indexes vendored/generated dirs;
  `--no-tests` hides test-path symbols on any query.
- Source-checkout maintenance: `make verify-clis` drives the real binary and
  asserts behavior of every agent-facing surface.

## Python Database Analysis

For custom queries or advanced analytics directly from the SQLite database:

- **Generate Structured Report**:

  ```bash
  uv run python scripts/report.py
  ```

  _This runs under Python 3.14 and directly extracts file distributions, symbol counts, and hot call sites using `sqlite3` without invoking the binary._

- **Explain a GitHub PR against an indexed checkout**:

  ```bash
  uv run python scripts/pr_feature_report.py --workspace clones/kubernetes --repo kubernetes/kubernetes --pr 139848
  ```

  _Use this for large benchmark submodules where local history may be shallow. It reads PR metadata and changed files with `gh api`, then reads symbols from the local `.ochna/ochna.db`._

## Workflow Integration Rules

1. **Graph First**: For symbol and relationship questions, run `ochna explore <keyword>` / `ochna search` first to map the call graph — it answers "who calls / what does this call" that `rg` cannot cheaply.
2. **Right Tool per Question**: Use `ochna search`/`callers`/`callees` for symbol and call-graph lookups; use `rg` for free-text or regex occurrences (e.g. where a string or config key appears) and `ast-grep` for structural AST patterns or rewrites. They complement each other.
3. **Read Replacements**: For structural reads, `ochna node --file <path>` returns line numbers and attaches dependents — handy when you want symbols + graph context rather than raw text.
4. **Large PR Archaeology**: For Linux/Kubernetes-style corpora, do not assume local merge parents exist. First verify the index with `ochna status --json`, use `gh api` for PR metadata and changed files, then use `ochna node --file ... --symbols-only --json` and `ochna node --symbol ... --include-code --json` for the changed symbols. Treat common Go method callees such as `GetList`, `Run`, `Add`, and `Stop` as noisy unless they are anchored to the changed file or exact production symbol.
