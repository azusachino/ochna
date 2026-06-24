# Changelog

All notable changes to ochna are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[SemVer](https://semver.org/).

Each release carries a **Performance** note for indexing-pipeline changes.
`BENCHMARK.md` holds the live, reproducible counts and timings over the pinned
test giants (`make report`); the numbers quoted here are directional and
machine-dependent.

## [0.1.0] — 2026-06-24

### Added

- `ochna howto`, a self-describing workflow command for humans and agents. It
  prints the recommended `status` -> `search` -> `callers` -> `node` flow and
  emits a structured capability descriptor with global `--json`.
- `status --json` preflight verdicts with `ok`, `db_present`, schema match,
  nested counts, freshness, indexed/live Git state, and a single next `action`.
  Stale or unusable indexes exit non-zero so automation can gate on freshness.
- Generated `.ochna/AGENT.md` pointer on `init`/`sync`, containing only index
  provenance plus links to `ochna howto` and `ochna status`.
- `pyscripts/verify_clis.py` and `make verify-clis` / `make verify_clis` for a
  `uv`-run real-binary smoke test that asserts the *behavior* (not just exit
  codes) of every agent-facing CLI surface, including `report.py`.

### Changed

- `make validate` now runs static checks plus the Python CLI verifier.
- `scripts/report.sh` reads the new `status --json` nested `counts` shape.
- Consolidated docs around a single source of truth: `ochna howto` is the
  canonical command/flag reference; `README.md` and `SKILL.md` no longer
  duplicate the per-command walkthrough and point at `howto` instead.
- Bumped crate version to `0.1.0`.

### Fixed

- `pyscripts/report.py` hotspots query referenced the pre-interning
  `edges.target_id` column (removed in 0.0.4) and errored on the current schema;
  it now joins `edges.target_nid` to `nodes`. Covered by `verify_clis.py`.

### Performance

- No indexing-pipeline changes; benchmark counts and timings should be stable
  against `0.0.5`.

## [0.0.5] — 2026-06-23

### Added

- Confidence-aware staged call resolution. Each call site is resolved through a
  cascade of increasingly specific stages and tagged with a `resolution_kind`
  (stored as an integer enum on the `edges` table; confidence is derived on
  read): `exact` (100) → `receiver_type` (90) → `package` / `namespace` (80) →
  `same_file` (60) → `name_only` (30). Name-only matches with multiple equally
  plausible targets are recorded as ambiguous/unresolved references instead of
  emitting a low-confidence edge to every candidate.
- Query flags `--min-confidence <N>` (filter `callers` results below a
  confidence threshold) and `--show-resolution` (append
  `[resolution: <kind>, confidence: <N>]` to query output). Results rank and
  dedup by confidence so the strongest match surfaces first.
- `raw_calls` now persists cheap AST context captured at parse time
  (`call_kind`, `receiver`, `type`, `package`, `import_hint`), feeding the
  resolution cascade without re-parsing.
- `pyscripts/pr_feature_report.py` and `pyscripts/corpus_probes.py`, plus a
  documented large-corpus PR archaeology workflow (`gh api` for PR
  metadata/files + the local index for changed-file symbols); see
  `docs/experiments/kubernetes-pr-139848.md`.

### Changed

- Query output for `callers` / `explore` / `node` now respects resolution
  confidence by default (no edges are hidden unless `--min-confidence` is set).
- Bumped crate version to `0.0.5`.

### Performance

- Resolution hot loop no longer allocates per call site; edge-set parity
  (selective sync == full rebuild) re-verified after the resolution changes.

## [0.0.4] — 2026-06-22

### Added

- `CHANGELOG.md` (this file) and a `Re-sync (s)` column in `BENCHMARK.md` /
  `scripts/report.sh` to track per-version indexing performance.
- Spring MVC route indexing for Java `@Controller` / `@RestController`
  classes. `@RequestMapping` plus method-level mapping annotations now emit
  `route` nodes linked to handler methods, with handler-qualified route IDs so
  duplicate URL patterns across controllers do not collide.
- `spring-petclinic` benchmark submodule and report row as a real Spring MVC
  corpus for route-indexing coverage.
- Scope classification: index-time `is_test` metadata for files/nodes,
  default library/generated directory exclusion with `init/sync
  --include-library`, and global query filtering with `--no-tests`.

### Changed

- Bumped `rusqlite` 0.31 → 0.40 and `tree-sitter` 0.25 → 0.26; switched AST
  traversal to cursor-based child iteration (`node.children(&mut cursor)`).
- Bumped crate version to `0.0.4`.

### Performance

Indexing-pipeline efficiency pass (`docs/performance-audit.md`):

- **Prune-set** — deleted-file prune now uses an `FxHashSet` of disk paths,
  O(D·F) → O(D+F). Bites incremental `sync`, free on fresh `init`.
- **Resolution index** — per-call same-file candidate lookup and namespace
  disambiguation go through prebuilt `by_name` / `by_name_file` maps with
  `SmallVec` buckets instead of `O(Kₛ)` scans + `id.contains` substring matches,
  O(C·K̄·L) → ~O(C).
- **FTS bulk-rebuild** — fresh builds bulk-load nodes with the `nodes_fts`
  triggers dropped, then rebuild the index once; incremental sync keeps
  per-row trigger maintenance.
- **String interning** — resolver candidates store `u32` handles into a shared
  string pool (ids/paths/namespaces) instead of cloned `String`s.
- **Selective incremental re-resolution** — `sync` re-resolves only call sources
  in changed files, callers whose `callee_simple` matches a changed symbol name,
  and unresolved refs that now match a new symbol — instead of deleting all edges
  and re-resolving every `raw_call`. Fresh `init` still does the full global pass.
  Verified edge-set parity (selective sync == full rebuild) on the tokio corpus.

Measured on linux (1.4M nodes / 2.3M edges): fresh index **~275s (0.0.3) → ~65s**;
no-op re-sync **~3s**. See `BENCHMARK.md` for the full table.

## [0.0.3] — 2026-06-20

### Added

- C, C++, and Zig parsing/indexing.
- `linux` (C) and `zig` benchmark submodules.
- `make report` + `scripts/report.sh` emitting `BENCHMARK.md` over the pinned
  test giants as a reproducible quality gate.

### Changed

- Hot symbol-resolution maps use `FxHashMap` (rustc-hash).

### Fixed

- Incremental `sync` stays crash-safe on WAL: aggressive bulk-load PRAGMAs are
  gated to fresh builds; incremental runs use WAL + `synchronous=NORMAL`.

### Performance

- Parallelized parsing with rayon and tuned bulk-load inserts
  (`prepare_cached` + bulk PRAGMAs): linux fresh index ~275s → ~173s.

## [0.0.2] — 2026-06-20

### Fixed

- crates.io publication: idempotent publish guard and release-workflow fixes
  (no functional change to the CLI).

## [0.0.1] — 2026-06-20

### Added

- Initial release: Rust/Nix rewrite of the structural code-graph CLI.
- Tree-sitter indexing for Rust, Go, and Java into SQLite (symbols + call edges).
- CLI subcommands `search` / `callers` / `node` / `explore` / `sync`.
- Cross-file call-edge resolution with `unresolved_refs`; `--json` output and
  tracing diagnostics to stderr.

[0.1.0]: https://github.com/azusachino/ochna/compare/v0.0.5...v0.1.0
[0.0.5]: https://github.com/azusachino/ochna/compare/v0.0.4...v0.0.5
[0.0.4]: https://github.com/azusachino/ochna/compare/v0.0.3...v0.0.4
[0.0.3]: https://github.com/azusachino/ochna/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/azusachino/ochna/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/azusachino/ochna/releases/tag/v0.0.1
