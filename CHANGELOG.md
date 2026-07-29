# Changelog

All notable changes to ochna are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[SemVer](https://semver.org/).

Each release carries a **Performance** note for indexing-pipeline changes.
`BENCHMARK.md` holds the live, reproducible counts and timings over the pinned
test giants (`make report`); the numbers quoted here are directional and
machine-dependent.

## [0.3.2] — 2026-07-30

Findings from a self-audit: running `ochna` on its own codebase and reviewing
`diff`/`db` for the same freshness-duplication and encoding-strictness
patterns already fixed once in 0.3.1.

### Fixed

- `init`/`sync`'s directory walker (`should_skip_dir`) never skipped
  `clones/`, unlike the separate `doctor` diagnostic scanner, which already
  did — the two skip-lists had drifted apart. Running `ochna init` at this
  repo's own root (the natural way to dogfood the tool on itself) silently
  indexed all 8 test-giant submodules instead of ochna's own source.
  `clones` added to the canonical skip-list; the diagnostic scanner now
  calls the same shared function instead of maintaining its own copy.
- `diff`'s current-index staleness gate (`current_index_is_fresh`) was a
  third, independent reimplementation of the freshness check fixed in
  0.3.1's `indexed_sources_are_fresh` — it had regressed in two ways:
  comparing file-set sizes instead of actual path sets, and propagating a
  raw UTF-8 decode error instead of gracefully excusing an unreadable file.
  Replaced with the shared `indexed_sources_are_fresh`.
- `ochna diff` crashed on any diff touching non-UTF-8 file content (binary
  files, or source in a non-UTF-8 encoding): its `git()` helper decoded
  stdout with strict `String::from_utf8` instead of the lossy decode used
  everywhere else in the codebase for git output.

### Known issue (not fixed this release)

- `search`'s FTS5 query passes raw user input into `MATCH` unescaped. A
  query containing FTS5 syntax characters (`-`, `"`, `*`, `:`, parens) can
  be silently reinterpreted as a boolean/phrase/prefix expression instead
  of literal text, and the exact-name/`LIKE` fallback only engages when FTS
  returns zero rows, not when it returns a wrongly-filtered nonempty set.

## [0.3.1] — 2026-07-29

### Fixed

- `status --json` freshness (`classify_freshness`) was git-porcelain-based:
  any git-dirty file anywhere in the workspace forced `freshness: "stale"`,
  even when the file was already reflected in the index and unrelated to
  ochna's own tracking. `doctor` already computed freshness correctly (per
  indexed-file content hash vs. disk); `status` now shares that same
  `indexed_sources_are_fresh` check instead of its own git-state comparison.
  A git-dirty workspace can be `status --json`-fresh immediately after
  `init`/`sync` as long as indexed content matches disk. Removed the now-dead
  `Freshness::Unknown` variant along with the git-comparison path it served.
- `indexed_sources_are_fresh` (shared by `status`/`doctor`) treated *any*
  discovered-but-unindexed file as staleness. A file that fails at any step
  of the indexing pipeline -- non-UTF-8 content, an extension `index.rs`
  can't map to a grammar, or a genuine parser error -- is silently skipped
  by `init`/`sync` and would fail identically on every future sync, so it
  could never actually resolve to `ok: true` no matter how many times it was
  synced. The freshness check now re-runs the same read -> resolve-language
  -> parse pipeline `run_init`/`run_sync` use, rather than special-casing one
  failure mode (e.g. just readability), so it stays correct for whichever
  step fails. Found via `clones/zig`, which has 2 legacy Latin-1
  (non-UTF-8) headers vendored from libc.

### Added

- `clones/ghostty` (Zig/C terminal application) and `clones/flink` (Java,
  large multi-module Maven build) as test-giant submodules, diversifying
  coverage beyond a Zig compiler and a single-library Java networking stack.
- `scripts/case_simulation.py` (`make case-sim`): formalizes the manual
  acceptance runs in `docs/experiments/v0.3-acceptance-{netty,kubernetes,
  linux}.md` into repeatable assertions of ochna's determined state against
  documented actual state from real historical PRs, plus a doctor/status
  smoke check for every other giant (including new ones with no known-answer
  case yet). This is what surfaced the `indexed_sources_are_fresh` bug above.

## [0.3.0] — 2026-07-29

Turns ochna from a fast symbol-lookup index into a diff-aware structural
review tool, frozen as a single contract in `docs/v0.3-contract.md`.

### Added

- `ochna doctor`, the review preflight: index/schema/freshness, a
  machine-readable `trust_verdict` (`trusted`/`degraded`/`unusable`),
  resolution-tier breakdown, collision-prone names, and low-quality
  locations/unsupported-extension diagnostics. Exits nonzero when the graph
  is degraded or unusable.
- `ochna unresolved [--in <path-prefix>] [--limit <n>]`, listing call sites
  that could not become a trusted edge, each with an explicit reason
  (`missing_target`, `ambiguous_target`, `macro_or_function`,
  `indirect_call`).
- `ochna tests-for <symbol> [--in <path-prefix>] [--limit <n>]`, returning
  test symbols with a structural path to a production symbol, labelled
  `direct_call`, `same_module_candidate`, or `name_heuristic` — never
  claimed as proof of runtime coverage.
- `ochna impact <symbol> [--depth <n>] [--direction callers|callees|both]
  [--min-confidence <n>] [--limit <n>]`, a bounded confidence-labelled
  traversal (default depth 2, min-confidence 30; max depth 5/200 nodes/400
  edges) that preserves every distinct root-to-node path, reports
  `affected_tests` and `unresolved_boundaries`, and refuses to guess across
  an ambiguous or common name rather than exploding into unbounded fanout.
- `ochna diff (--base <rev> [--head <rev>] | --files <path>...) [--limit
  <n>]`, mapping a Git range or explicit changed paths onto indexed
  symbols/edges. Historical ranges use disposable temporary indexes and
  report real added/removed/modified symbols, edges, and newly unresolved
  callers; a revision unavailable on a shallow clone returns a structured
  `base_revision_unavailable` failure rather than a fabricated diff.
- Explicit Java/Spring framework relationship edges: `route_handler`,
  `injected_into`, `configuration_binds`, `publishes_event`,
  `consumes_event`, `feign_calls`, `grpc_calls`, each carrying a confidence
  tier (`framework_annotation` 85, `framework_declaration` 80,
  `framework_convention` 70) distinct from compiler-exact edges.
- Schema v6: relationship-aware raw call edges backing the new framework
  and test relationships.
- `ochna howto` (both text and `--json`) now documents the review workflow
  (`doctor` → `diff` → `impact` → `tests-for` → `unresolved`) alongside the
  existing lookup flow.
- `fixtures/review-v0.3`, a deterministic before/after fixture backing the
  contract's acceptance user stories.

### Changed

- Bumped crate version to `0.3.0`.
- `docs/v0.3-contract.md` freezes the human/JSON contract, corpus
  benchmarks, and output budgets for every command above; the read-only MCP
  adapter designed alongside it is **deferred** (no concrete MCP-only
  consumer, discovery/allow-list requirement, or persistent-service need was
  demonstrated) — its contract remains frozen design material, not shipped
  in this release.
- `SKILL.md` documents the review workflow and its judgment notes (common
  Netty/Kubernetes-scale `doctor` verdicts, ambiguous-name `impact` refusal,
  the `tests-for` indirect-coverage limitation below).

### Known limitations

- `tests-for` returns an empty result for a production symbol whose only
  test coverage is *indirect* (e.g. a JUnit wrapper that triggers it via
  object lifecycle rather than a direct call) — confirmed unchanged against
  Netty PR 16959's `releaseAndFailQueuedWrite` in
  `docs/experiments/v0.3-acceptance-netty.md`. This is a deliberate
  tradeoff (never inflate coverage with an unproven heuristic), not a bug.
- `doctor --json`'s `collision_prone_names`/`low_quality_locations` arrays
  are uncapped; on a large monorepo (Kubernetes: 35,872 / 10,342 entries)
  this produced a 6.4MB JSON document. Contract-compliant today (no stated
  cap on those arrays) but worth a budget in a future contract revision.

### Verification

- Local gates: `make check`, `make test` (38), `make install`,
  `make verify-clis` all pass.
- Corpus acceptance (`docs/experiments/v0.3-acceptance-{netty,kubernetes,linux}.md`):
  Netty (`releaseAndFailQueuedWrite` → 3 `handlerRemoved` callers), Kubernetes
  (PR 139848 watch-cache test mapping plus a `GetList` common-name budget
  stress test), and Linux (`strncpy` removal absence plus a truthful
  shallow-clone `diff` failure) all pass their contract-required evidence.
  Every run indexed a disposable `/tmp` copy of the pinned commit; the
  `clones/*` submodules were never entered or mutated.

### Performance

- No indexing-pipeline changes to the existing call-resolution cascade;
  the new commands are additional read queries over the existing schema
  plus the v6 relationship-aware edges.

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
