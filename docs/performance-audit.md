# Performance Audit — indexing pipeline

Audit of `run_init` (the index/`sync` path) and the global call resolver.
Scope: indexing throughput, which is ochna's differentiator. Query-time
(`search`/`callers`/`node`) is already O(log N + result) via SQLite indexes
(`idx_nodes_name`, `idx_edges_source_id`, `idx_edges_target_id`) and is not
covered here.

Date: 2026-06-20. Baseline: tree-sitter 0.26, rusqlite 0.40 dep bump in flight.

## Symbols

- **F** = source files on disk, **D** = files already recorded in the DB
- **N** = total nodes (symbols), **C** = total call sites, **U** = distinct names
- **Kₛ** = nodes sharing simple name *s*; **K̄** = average candidate-set size
- **L** = average length of a node id string (`file::path::sym`)
- **B** = total source bytes, **p** = cores

## Per-stage complexity (current)

| Stage | Op | Time | Space | Verdict |
|---|---|---|---|---|
| Prune deleted (`commands.rs` prune block) | `for db_file { files.iter().any(\|f\| f.to_string_lossy()==db_file) }` | O(D·F) + O(D·F) allocs | O(1) | quadratic |
| Parallel parse | rayon `par_iter` read+hash+parse | O(B/p) | O(B/p) | optimal |
| DB write | upsert per node/call in one tx | O(N+C), SQLite-bound | — | fine (PRAGMA + prepare_cached tuned) |
| Build `name_to_ids` | `FxHashMap<String, Vec<String>>` | O(N) | O(N) cloned ids | ok |
| Build `id_to_file` | `FxHashMap<String, String>` | O(N) | O(N) path copies (path stored per node, not per file) | wasteful |
| Resolve calls (`parser::resolve_calls_global`) | per call: `split("::")`→Vec, same-file filter O(Kₛ), `disambiguate` substring `id.contains` O(Kₛ·L) | O(C·K̄·L) + O(C) transient allocs | O(K̄) temp | hot |

## Findings

### 1. Prune is O(D·F) — bites incremental `sync`, not fresh `init`

The deleted-file prune compares every DB path against the whole disk list with
`Vec::iter().any()`, and each comparison allocates via `to_string_lossy()`.
On a re-index of linux (F ≈ D ≈ 80k) that is ~6.4e9 allocating comparisons.
Free on a fresh `init` (D = 0); the cost lands squarely on the `sync` path.

**Fix (space→time):** hash disk paths once into an `FxHashSet`, membership O(1).
**O(D·F) → O(D+F)**, +O(F) space. Trivial, highest ROI.

### 2. Resolution is O(C·K̄·L) — runs in full on every init and sync

Edges are deleted and re-resolved globally from all `raw_calls` on every run
(intentional, for cross-file edge correctness — see the raw_calls design note in
AGENTS.md / project memory). Per call the resolver re-splits the callee name,
filters all Kₛ same-named candidates, and `disambiguate` does substring
`id.contains(&"::ns::")` over them. Cost concentrates on the worst names
(`new`, `get`, `build`) which have large Kₛ *and* are called most.

**Fix (space→time):** while already iterating the N nodes to build the maps,
precompute:
- `by_name_file: FxHashMap<(name, file), …>` → same-file candidate lookup in
  O(1) instead of an O(Kₛ) filter;
- carry each candidate's namespace as a stored field so disambiguation compares
  fields instead of substring-scanning ids.

**O(C·K̄·L) → ~O(C)**, +O(N) space.

### 3. String interning — the deep, compounding win (0.0.4 redesign)

Every node id (long `file::path::sym`) and file path is cloned into multiple
maps, `edges`, and `raw_calls`; memory is O(N·L) and every map op hashes/compares
long strings.

Intern ids and paths to `u32` handles once (`Vec<String>` + `FxHashMap<&str,u32>`):
maps become `FxHashMap<u32, …>`, edges become `(u32, u32)`.
- Memory: O(N·L) → one copy + 4-byte handles.
- Speed: FxHash of a `u32` ≈ one multiply; equality is an integer compare —
  cheaper lookups in every hot map, compounding across the pipeline.
- Cost: threads through the db interface, parser output, and resolver. A genuine
  structural change — stage it as the headline of 0.0.4, not a patch.

### 4. Minor / supporting

- `id_to_file` duplicates each file path O(N) times; if interning is deferred,
  at least store a `FileId`/index (subset of finding 3).
- Per-call transient allocations in the resolver (`name_parts` Vec, `same_file`
  / `working_set` Vecs, `format!` infix strings) are O(C) churn; finding 2's
  index removes most of them.

## Recommended sequencing

Ranked by ROI:

1. **A — prune-set** (finding 1): contained `perf:` change, O(D·F)→O(D+F).
2. **B — resolution index** (finding 2): contained `perf:` change, O(C·K̄·L)→~O(C).
3. **C — string interning** (finding 3): structural, owns 0.0.4.

A and B are independent, contained, and land as separate `perf:` commits after
the tree-sitter 0.26 / rusqlite 0.40 dep bump (+ cursor-based traversal) is in.
C is the 0.0.4 redesign.

## Possible further solutions

These are follow-on options beyond A/B/C. They should be benchmark-gated after
the contained fixes land, because the next bottleneck may move.

### Incremental edge re-resolution

The current pipeline deletes all resolved `edges` and `unresolved_refs`, then
re-resolves every `raw_call` globally on every run. That is simple and correct,
but it makes small `sync` runs pay whole-graph cost.

Persist normalized call fields at parse time:
- `callee_simple` — final path segment, e.g. `new` from `Point::new`;
- `callee_scope` — optional explicit receiver / namespace, e.g. `Point`;
- `caller_file` — copied from the caller node's file for fast same-file filters.

Then index for selective invalidation:
- `raw_calls(callee_simple)`;
- `raw_calls(caller_id)`;
- `nodes(name, file_path)`.

On `sync`, resolve only:
- calls from changed files;
- calls whose `callee_simple` appears in changed symbol names;
- unresolved refs whose specifier now matches a newly introduced symbol.

This is larger than the resolution index, but it can turn small incremental runs
from O(C) whole-graph work into work proportional to changed files plus affected
symbol names.

### Candidate vectors before full interning

If full string interning is too large for the next patch, introduce compact
candidate indices first:

```rust
struct SymbolCandidate {
    id: String,
    file_path: String,
    namespace: Option<String>,
}

type SymbolIx = u32;

by_name: FxHashMap<String, SmallVec<[SymbolIx; 4]>>
by_name_file: FxHashMap<(String, String), SmallVec<[SymbolIx; 2]>>
symbols: Vec<SymbolCandidate>
```

Most names have tiny candidate sets, so `SmallVec` keeps common cases inline and
avoids many heap allocations. The resolver then moves small integers through the
hot loop and touches full strings only for final edge construction.

### Library options

- `smallvec`: good fit for candidate lists, because most symbol-name buckets are
  small and only hot names spill to heap.
- `lasso`: good fit for the 0.0.4 interning redesign. It provides stable string
  keys, single-threaded and threaded interners, plus lower-memory reader/resolver
  modes once interning is complete.
- `nohash-hasher`: useful after ids become `u32` / `u64`; integer-keyed maps can
  skip hashing entirely. This is a later optimization after handles exist.
- `fst`: not a primary fit for call resolution today. It is strongest for huge
  ordered string sets/maps and automata queries, so it may fit future symbol
  autocomplete or disk-backed search indexes better than the indexing hot path.

### FTS maintenance

`nodes_fts` is trigger-maintained today. For fresh full builds, it may be faster
to bulk-load `nodes` first, then rebuild or optimize FTS once with SQLite FTS5
special commands. Keep trigger maintenance for incremental sync unless benchmarks
show trigger overhead dominates.
