# ADR-0001: Confidence-tiered call resolution

Status: Implemented (v0.3.0)
Date: 2026-06-23 (original plan) — recorded 2026-07-29

## Context

The original resolver recorded call sites as simple names and matched them
against a project-wide symbol index: find candidates by name, prefer same
file, disambiguate by namespace, otherwise take the first candidate. This
was fast and worked for distinctive names, but on common method names in
large corpora (`GetList`/`Run`/`Add` in Kubernetes, overridden methods in
Netty) it produced a plausible-looking edge that was often wrong — worse
than an honest unresolved reference, because it looked authoritative.

Corpus evidence (`docs/experiments/kubernetes-pr-139848.md`,
`docs/experiments/netty-pr-16959.md`, `docs/experiments/
linux-strncpy-removal.md`) showed the shape of the problem: Netty's
`releaseAndFailQueuedWrite` resolved its 3 real callers exactly (distinctive
name, clean ownership) while Kubernetes's `GetList` fanned out across
unrelated types (common name, receiver context discarded at parse time).

Full type checking (becoming `gopls`/`clangd`/`rust-analyzer`) was
explicitly out of scope — that's a different product with a different cost
structure.

## Decision

Preserve more cheap, AST-derived call-site context (receiver expression,
receiver type when locally inferable, package/namespace, import hints) and
resolve in staged passes, each stage producing a labelled confidence tier
instead of a single silent guess:

```text
exact (100) > receiver_type (90) > package/namespace (80) > same_file (60) > name_only (30)
```

`resolution_kind` is stored as a small `INTEGER` enum directly on `edges`
(not a side `edge_metadata` table — every edge needs one, so a side table
would only add a join with no migration-risk benefit, since `.ochna/ochna.db`
is a derived cache that rebuilds on a schema-version bump). Confidence is a
pure function of `resolution_kind`, derived on read, never stored as a
second column.

Default query output never silently hides edges — it ranks high-confidence
first and annotates each with its resolution kind. `--min-confidence` is the
explicit opt-in filter; `--show-resolution` surfaces the kind in text output.
Ambiguous name-only matches with multiple candidates become an
`ambiguous_refs`/unresolved record rather than a guessed edge; a
name-only edge is only emitted when the global candidate is unique.

## Consequences

- Netty's `releaseAndFailQueuedWrite` and Kubernetes's disambiguated
  `CacheDelegator::GetList` both resolve correctly with confidence-labelled
  edges (verified in `scripts/case_simulation.py`); bare `GetList` correctly
  refuses to guess across its 9 candidates instead of picking one.
- JSON output always includes `resolution_kind` + derived `confidence`, so
  agents can decide when to trust an edge without anything being hidden.
- Selective incremental re-resolution had to be extended to consider the new
  raw-call context fields, not just `callee_simple`.
- C macro/function-pointer calls and deletion-heavy API removals (Linux
  `strncpy`) still need honesty over cleverness: mark lower confidence or
  unresolved rather than guess; explaining a removal from the current tree
  alone is out of scope for this resolver (see the `diff`/historical-index
  work that followed instead).

## Alternatives considered

- **Side `edge_metadata` table** — rejected. Its only advantage (lower
  migration risk) doesn't apply to a derived, rebuild-on-bump cache; every
  edge needs the field, so it would only add a join on the hot query path.
- **Full compiler-grade type resolution** — rejected as scope creep against
  ochna's differentiator (fast, local, structural, language-broad).
