# Leveling up ochna — lessons from the HPA dive

Conclusions from driving ochna through a real subsystem investigation
(see `docs/dives/kubernetes-hpa.md`). Ordered by leverage. Bias: simple, high-value,
schema-cheap changes first — no speculative machinery.

## What the walk-through taught me

1. **Top-down dives are the common case, but ochna only walks bottom-up.** I traced the
   reconcile tree by *guessing the next function's name* and running `search`, because the
   only graph command is `callers` (reverse edges). Every step "what does this function
   call?" was manual. This is the biggest ergonomic gap.
2. **Resolution by bare name is the biggest correctness gap.** `callers worker` returned a
   dozen unrelated controllers. ochna keys on symbol name, ignoring receiver type/package,
   so generic names (`worker`, `Run`, `Sync`, `reconcile*`) are unusable. Distinctive names
   are great — the tool is bimodal, and nothing tells the user which mode they're in.
3. **`search` output is flat, unranked, and unbounded.** A broad term dumps ~200 declaration
   rows (FTS → exact → LIKE, no ordering, no cap). The signal (the one symbol you want) is
   buried, which is what makes it feel no better than `rg`.
4. **Output omits the qualifier that would disambiguate.** Results show `name (kind) — file:line`
   but not the receiver/package (`podautoscaler.HorizontalController.worker`). The qualified
   name is exactly what resolves the name-collision confusion visually.

## Level-up plan (proposed epic: `ochna:dive-ergonomics`)

Smallest → biggest:

### task-1 — `callees`/forward call command  *(highest leverage, schema-cheap)*
Add `ochna callees <symbol>`: the symmetric query of `find_callers` (`src/db/nodes.rs:107`),
filtering edges by `source_nid` instead of `target_nid`. The edges PK is
`(source_nid, target_nid, kind)` (`src/db/schema.rs:33`), so the forward lookup is
index-served for free. Turns a top-down dive from "guess the next name" into one query.
Reuse the `callers` output/`--show-resolution`/`--no-tests` plumbing.

### task-2 — scope `callers`/`callees` to kill name collisions
Add `--in <path-prefix>` (and/or `--receiver <Type>`) so `callers worker --in pkg/controller/podautoscaler`
returns only the HPA edge. Filter target-node resolution by file-path prefix before edge
lookup. Low effort, directly fixes pitfall #1.

### task-3 — rank + cap `search`
Order results: exact name → prefix → FTS relevance → LIKE; add a default `--limit` (e.g. 30)
with a "+N more" note. Keeps the entry-point query as a *locator*, not a 200-line dump.
(Previously discussed as the standalone-search fix.)

### task-4 — show qualified names in output
Render `pkg/...:Type.method` (or package-qualified) in `search`/`callers`/`callees` lines so
collisions are visible at a glance. Pairs with task-2.

### Stretch — call-tree / path
`ochna tree <symbol> --depth N` (transitive `callees`) or `path <A> <B>`. Only after task-1
proves the forward-edge query; defer to avoid over-engineering.

## Non-goals
- Usage/occurrence search (where a field is read) — that stays `rg`'s job; ochna is the
  symbol/edge graph, not a text grep.
- Cross-name fuzzy resolution heuristics — prefer explicit scoping (`--in`/`--receiver`) over
  guessing, per the project's "simple over clever" bias.
