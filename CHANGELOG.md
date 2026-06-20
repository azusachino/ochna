# Changelog

All notable changes to ochna are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[SemVer](https://semver.org/).

Each release carries a **Performance** note for indexing-pipeline changes.
`BENCHMARK.md` holds the live, reproducible counts and timings over the pinned
test giants (`make report`); the numbers quoted here are directional and
machine-dependent.

## [Unreleased] — 0.0.4

### Added

- `CHANGELOG.md` (this file) and a `Re-sync (s)` column in `BENCHMARK.md` /
  `scripts/report.sh` to track per-version indexing performance.

### Changed

- Bumped `rusqlite` 0.31 → 0.40 and `tree-sitter` 0.25 → 0.26; switched AST
  traversal to cursor-based child iteration (`node.children(&mut cursor)`).

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

[Unreleased]: https://github.com/azusachino/ochna/compare/v0.0.3...HEAD
[0.0.3]: https://github.com/azusachino/ochna/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/azusachino/ochna/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/azusachino/ochna/releases/tag/v0.0.1
