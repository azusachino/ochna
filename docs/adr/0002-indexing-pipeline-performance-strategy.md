# ADR-0002: Indexing pipeline performance strategy

Status: Implemented (v0.0.4)
Date: 2026-06-20 (original audit) — recorded 2026-07-29

## Context

Indexing throughput is ochna's differentiator (query time was already
O(log N + result) via SQLite indexes and out of scope for this audit). A
complexity audit of `run_init`/`sync` and the global call resolver found
several stages with avoidable superlinear cost as corpora grow toward the
Linux scale (~80k files, ~1.4M nodes):

- deleted-file pruning compared every DB path against the whole disk list
  with `Vec::iter().any()` — O(D·F), free on fresh `init` but expensive on
  incremental `sync`;
- call resolution re-split and substring-scanned same-named candidate sets
  per call — O(C·K̄·L), concentrated on the worst common names;
- every node id and file path (long `file::path::sym` strings) was cloned
  into multiple maps, edges, and `raw_calls` — O(N·L) memory, with every map
  operation hashing/comparing long strings.

## Decision

Sequence the fixes by ROI, smallest/contained first:

1. **Prune-set**: hash disk paths once into an `FxHashSet` for O(1)
   membership. O(D·F) → O(D+F).
2. **Resolution index**: precompute `by_name_file` and a stored namespace
   field while already iterating nodes, replacing the O(Kₛ) filter +
   substring scan with O(1) lookups and field comparisons. O(C·K̄·L) → ~O(C).
3. **String interning** (the structural 0.0.4 redesign): intern node ids and
   file paths to `u32` handles once; maps and edges key on integers instead
   of strings. O(N·L) → one string copy + 4-byte handles; FxHash of a `u32`
   is ~one multiply instead of a long-string hash.
4. **FTS bulk-rebuild** on fresh builds instead of trigger-per-row
   maintenance (keep triggers for incremental `sync`).
5. **Selective incremental re-resolution**: persist normalized call fields
   (`callee_simple`, `callee_scope`, `caller_file`) and index them, so `sync`
   resolves only calls from changed files / calls whose callee name changed
   / unresolved refs that now have a matching symbol — instead of deleting
   and re-resolving every edge from every `raw_call` on every run.

`smallvec` fit candidate lists (most name buckets are small, only hot names
spill to heap); `lasso` fit the interning redesign (stable string keys,
threaded interner). `nohash-hasher` and `fst` were evaluated and parked —
useful only after handles exist, or not a fit for this hot path.

## Consequences

- Linux fresh index: ~275s → ~65s. No-op re-sync: ~3s (see `BENCHMARK.md`
  for current, machine-dependent numbers via `make report`).
- Selective re-resolution requires edge-set parity testing against a full
  rebuild to prove correctness on every change to the invalidation logic —
  this is correctness-critical, not just a performance nicety.
- The interning redesign threaded through the db interface, parser output,
  and resolver — a genuine structural change, not a contained patch, and
  was staged as its own 0.0.4 release rather than bundled with 1–2.

## Alternatives considered

- Leaving prune/resolution at their original complexity until they proved
  to be an actual bottleneck at benchmark scale — rejected once the Linux
  corpus made the cost measurable and reproducible per pinned commit.
