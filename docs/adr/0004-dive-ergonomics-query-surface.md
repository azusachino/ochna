# ADR-0004: Dive-ergonomics query surface

Status: Implemented (v0.2.0); stretch item tracked as issue #11
Date: 2026-06-25 (original plan, from `docs/dives/kubernetes-hpa.md`) — recorded 2026-07-29

## Context

Driving ochna through a real subsystem investigation (a Kubernetes HPA
reconcile-loop dive) surfaced four ergonomic gaps, ordered by how much they
hurt:

1. **Only bottom-up navigation existed.** `callers` (reverse edges) was the
   only graph command; tracing a top-down call tree meant guessing the next
   function's name and re-running `search` at every step.
2. **Bare-name resolution was the biggest correctness gap.** `callers
   worker` returned a dozen unrelated controllers — name matching ignored
   receiver type/package, so generic names were unusable while distinctive
   names worked fine, with no signal telling the user which mode they were
   in.
3. **`search` was flat, unranked, and unbounded.** A broad term dumped
   ~200 rows with no ordering or cap, burying the one relevant result.
4. **Output omitted the qualifier that would disambiguate** — `name (kind)
   — file:line` without the receiver/package, when the qualified name is
   exactly what makes a collision visible.

## Decision

Ship smallest-to-biggest, schema-cheap first:

- **`ochna callees <symbol>`** — the symmetric forward-edge query to
  `callers`, filtering by `source_nid` instead of `target_nid`; the edges PK
  `(source_nid, target_nid, kind)` makes this index-served for free. Reuses
  `callers`' output/`--show-resolution`/`--no-tests` plumbing rather than
  duplicating it.
- **`--in <path-prefix>` scoping** on `callers`/`callees`, filtering
  target-node resolution by file-path prefix before edge lookup — directly
  fixes the name-collision gap.
- **Ranked, capped `search`**: exact name → prefix → FTS relevance → LIKE,
  with a default `--limit` and a "+N more" note, keeping it a *locator*
  rather than a dump.
- **Qualified names in output** (`pkg/...:Type.method`) across
  `search`/`callers`/`callees`, pairing with `--in` to make collisions
  visible at a glance.
- **Stretch, deferred at the time**: `ochna tree <symbol> --depth N`
  (transitive `callees`) or `path <A> <B>` — explicitly held back "to avoid
  over-engineering" until the forward-edge query proved out. It has;
  tracked as [issue #11](https://github.com/azusachino/ochna/issues/11) in
  the v0.4 milestone, to reuse `impact`'s bounded-traversal machinery
  (depth/node/edge caps, confidence labelling) rather than a second
  unbounded-traversal path.

Explicit non-goals: usage/occurrence search (where a field is *read*) stays
`rg`'s job — ochna is the symbol/edge graph, not a text grep. Cross-name
fuzzy resolution heuristics were rejected in favor of explicit scoping
(`--in`/`--receiver`) over guessing, matching the project's "simple over
clever" bias.

## Consequences

- A top-down dive is now one query per level (`callees`) instead of a
  name-guessing loop.
- `--in` and qualified-name output together resolve the collision problem
  that made `search`/`callers` feel no better than `rg` on common names.
- The deferred stretch item (call-tree/path) is explicitly scoped to reuse
  `impact`'s existing bounded traversal rather than introduce a second
  unbounded one, avoiding a maintenance fork.

## Alternatives considered

- Cross-name fuzzy resolution (guessing across similarly-named symbols) —
  rejected in favor of explicit `--in`/`--receiver` scoping.
- Shipping call-tree/path immediately alongside `callees` — deferred until
  the forward-edge query itself proved valuable, to avoid speculative
  machinery.
