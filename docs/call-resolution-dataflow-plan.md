# Call Resolution Dataflow Plan

## Executive Summary

Ochna is already useful for large-codebase archaeology: it can index Linux, Kubernetes, and Netty at useful scale and answer symbol/file questions quickly. The recent corpus experiments exposed the next ceiling: call edges are sometimes too broad because the current resolver receives call sites with too little semantic context.

The current dataflow records raw calls mostly as simple names, then resolves those names against a project-wide symbol index. This works for many direct calls and for small/medium codebases. It breaks down on common method names in large corpora:

- Go/Kubernetes: `GetList`, `Run`, `Add`, `Stop`, `Update`, `Watch`.
- Java/Netty: Java performs better because class-qualified methods are already indexed, but overridden/common names can still fan out.
- C/Linux: direct C function symbols work well; deletion-heavy API removals still need patch metadata because the important fact may be that a symbol no longer exists.

The proposed redesign is not "add full type checking." That would be a different product. The proposal is to preserve more cheap, AST-derived call-site context and resolve in staged passes with confidence levels. This keeps ochna lightweight while making graph answers more trustworthy.

## Why This Matters

Ochna is intended to be an agent-grade navigation tool. An agent does not need a perfect compiler graph for every language. It needs a graph that is honest about certainty and good enough to narrow investigation from "millions of lines" to "the 3-10 relevant definitions and callers."

The current resolver can produce a plausible but noisy edge for a common method name. That is worse than a clean unresolved reference in some workflows because it looks authoritative. A confidence-aware resolver gives the user and agent a better contract:

- exact edges are high signal;
- receiver/package edges are useful but inspectable;
- name-only edges are hints, not proof;
- unresolved references are preserved instead of silently dropped.

This plan keeps the current successful properties:

- fast fresh indexing;
- selective incremental re-resolution;
- SQLite portability;
- JSON query output;
- language support that can grow one parser at a time.


## Evidence From Recent Corpus Runs

Full write-ups live in `docs/experiments/` and the asobi project log; this is the
distilled signal. All runs used the installed `ochna` binary against the
benchmark submodules. The submodules are shallow to a single commit, so local
parent diffs are unavailable. The reliable workflow was:

1. verify the local index with `ochna status --json`;
2. use `git log --oneline -1` only to identify the current commit/PR;
3. use `gh api` for PR/commit metadata and patch files;
4. use ochna for current-tree symbols, line spans, callers, and callees.

| Corpus | Change | What ochna did well | Where it fell short | Root cause |
| --- | --- | --- | --- | --- |
| Kubernetes (PR 139848) | watch-cache test rewrite | located exact symbols (`ShouldDelegateList`, `GetExactSnapshotLocked`, the new tests) | broad callees of a big Go test mixed in unrelated `Run`/`Add`/`Stop`/`GetList` | Go selector calls store only the field name, not the receiver expr/type |
| Netty (PR 16959) | traffic-shaping write leak fix | `callers releaseAndFailQueuedWrite` returned exactly the 3 `handlerRemoved` cleanups | overloaded/common Java method names still fan out at scale | name-only fallback is accurate only when the name is distinctive |
| Linux (`strncpy` removal) | core API + arch impls deleted | confirmed `strncpy` gone from `lib/string.c`/`string.h`/FORTIFY while `strncpy_from_user` etc. remain | could not *explain* the removal from a symbol graph alone | current index models only the current tree; no prior tree / removed symbols |

See the standalone experiment reports for the full traces:

- `docs/experiments/kubernetes-pr-139848.md`
- `docs/experiments/linux-strncpy-removal.md`
- `docs/experiments/netty-pr-16959.md`

The Netty `callers` result is the positive model for this redesign: when names
are distinctive and ownership is clear, ochna already behaves like a strong
structural graph. The goal is to make common-named calls behave like that case,
and to be honest when they can't.


## Current Dataflow

Today, parser modules emit:

```text
source file
  -> AST traversal
  -> nodes: id, name, kind, qualified_name, file_path, span
  -> raw_calls: caller_id, callee_name, callee_simple, callee_scope, line, column
```

`run_init` then builds a `SymbolIndex` from nodes:

```text
by_name
by_name_file
by_id
namespace from qualified_name
```

`resolve_calls_global` does:

1. find candidates by simple name;
2. prefer same file;
3. disambiguate by explicit namespace or caller namespace;
4. otherwise choose the first candidate;
5. record an edge, or unresolved ref if there are no candidates.

This design is fast and compact. The problem is the last step: when candidates remain ambiguous, choosing the first candidate creates false authority.

## Target Contract

The redesigned resolver should satisfy these rules:

1. A call edge must carry how it was resolved.
2. Name-only matches must be distinguishable from exact or receiver-qualified matches.
3. Query commands should default to high-signal edges, while still allowing broad hints.
4. Incremental sync must remain selective and correct.
5. Parsers should add cheap facts first; no full compiler/typechecker dependency is required for the first version.

## Proposed Schema Direction

### Enrich `raw_calls`

Add fields that preserve AST-derived context:

```text
raw_calls
  caller_nid INTEGER NOT NULL
  callee_name TEXT NOT NULL
  callee_simple TEXT
  callee_scope TEXT
  call_kind TEXT               -- function, method, constructor, static_method, macro, unknown
  receiver_expr TEXT           -- ctx, cacher, this, TypeName, pkg, namespace
  receiver_type TEXT           -- best known type, if cheap to infer
  package_or_namespace TEXT    -- Go package, Java package, C++ namespace, Rust module-ish
  import_hint TEXT             -- Java import / Go package alias target, when available
  line INTEGER NOT NULL
  column INTEGER NOT NULL
```

Keep the existing fields for compatibility and indexing. New fields can be nullable at first.

### Add Resolution Metadata

Carry one `resolution_kind` column directly on `edges`:

```text
edges
  source_nid
  target_nid
  kind
  resolution_kind INTEGER NOT NULL DEFAULT 0   -- enum, see below
  PRIMARY KEY (source_nid, target_nid, kind)   -- unchanged, WITHOUT ROWID
```

`resolution_kind` is a small integer enum, not text, to stay consistent with the
0.0.4 interning/size work (the alternative — a `TEXT` label repeated across
~2.3M Linux edges — would undo it):

```text
0 name_only      30
1 same_file       60
2 namespace        80
3 package           80
4 receiver_type      90
5 exact               100
```

**Confidence is derived, not stored.** It is a pure function of
`resolution_kind` (the mapping above), so storing it as a second column would
duplicate data on every edge for no gain. Query code maps the enum to a
confidence number on read.

Why columns on `edges` rather than a side `edge_metadata` table:

- `.ochna/ochna.db` is a derived cache, rebuilt on schema-version bump — there
  is no precious DB to migrate, so the side-table "lower migration risk"
  argument does not apply here.
- *Every* edge carries a `resolution_kind`, so a side table saves no rows; it
  only duplicates the `(source_nid, target_nid, kind)` key per row and adds a
  join on the hot query path, fighting the recent size/latency work.
- Keeping the PK at `(source_nid, target_nid, kind)` is intentional: one edge
  per relation, holding its best `resolution_kind` — not multiple evidence rows.

### Add Symbol Ownership Indexes

The in-memory `SymbolIndex` should include:

```text
by_qualified_name
by_type_and_method
by_package_and_name
by_file_and_name
by_kind_and_name
```

For persisted lookup/debugging, consider a lightweight table later:

```text
symbol_owners
  node_nid
  owner_kind TEXT       -- class, struct, interface, package, namespace
  owner_name TEXT
  package_or_namespace TEXT
```

This table is optional for phase 1. The in-memory index is enough to prove the design.

## Language-Specific Context To Capture

### Java

High-value facts:

- package declaration;
- imports, including wildcard imports;
- class/interface/enum ownership;
- method invocation receiver expression;
- constructor type;
- `this.method()` and bare `method()` inside a class;
- superclass/interface names when cheap.

Resolution examples:

- `ReferenceCountUtil.safeRelease(msg)` -> static method on imported/qualified class.
- `promise.tryFailure(cause)` -> receiver expression `promise`; if method parameter type is `ChannelPromise`, prefer `ChannelPromise::tryFailure`.
- bare `releaseAndFailQueuedWrite(...)` inside subclass -> inherited/static helper on `AbstractTrafficShapingHandler` if class hierarchy is known; otherwise same class/package fallback.

Phase 1 does not need full generic typing. Method parameters, local variable declarations, fields, and `new Type(...)` assignments cover many practical cases.

### Go

High-value facts:

- package name;
- imports and aliases;
- selector expression receiver, e.g. `delegator.GetList`;
- receiver type for methods;
- local variables assigned from constructors when obvious;
- parameters and short declarations with explicit type.

Resolution examples:

- `cacher.watchCache.Add(...)` should not match every `Add` in Kubernetes. The raw call should preserve receiver chain `cacher.watchCache` and ideally type `watchCache`.
- `delegator.GetList(...)` should prefer `CacheDelegator::GetList` if `delegator := NewCacheDelegator(...)` is visible.
- `storage.ValidateListOptions(...)` should resolve through import/package alias rather than global simple name.

Phase 1 can implement receiver expression and package alias capture without solving every local type.

### C / C++

High-value facts:

- namespace/class parent already matters for C++;
- function pointer calls should be marked `indirect` instead of guessed;
- macro-like invocations should be marked `macro_or_function` or left unresolved when ambiguous;
- deleted-symbol explanations require a previous index or patch input, not only current AST.

For Linux, the biggest next win is not full C type resolution. It is better honesty:

- direct known function calls -> exact/name;
- macro-heavy calls -> lower confidence;
- removed APIs -> compare two indexes or use patch metadata.

### Rust / Zig

Keep current behavior, but align with the same confidence model:

- explicit `Type::method` or namespace path -> exact/namespace;
- bare method calls -> receiver/same-impl if known;
- fallback -> name-only.

## Resolver Algorithm

Replace the current single candidate selection with staged matching:

```text
for each raw_call:
  candidates = by simple name
  if none:
    unresolved

  stage 1: exact qualified hint
  stage 2: receiver static type + method
  stage 3: package/import/namespace + name
  stage 4: same caller namespace/type
  stage 5: same file
  stage 6: unique global name
  stage 7: ambiguous name-only
```

Each successful stage emits:

```text
edge(source, target, "calls", resolution_kind)   -- confidence derived on read
```

For stage 7, there are two policy options:

1. Emit no edge and record an unresolved/ambiguous ref.
2. Emit a low-confidence edge but mark it `name_only`.

Recommendation: emit an `ambiguous_refs` record for multi-candidate name-only cases, and only emit a `name_only` edge when the global candidate is unique. This makes default graph output more trustworthy.

## Query UX

Add filters without breaking existing commands:

```bash
ochna callers GetList --min-confidence 80   # opt-in filter
ochna node --symbol Foo --include-code --show-resolution
ochna explore watchCache --show-resolution
```

**Default behavior must not silently hide edges.** That would contradict the
"preserve, don't silently drop" contract above and regress existing `callers`
workflows that rely on broad results. Instead, the default **rank** edges
high-confidence-first and annotate each with its kind; `--min-confidence` is the
explicit opt-in for filtering. JSON output always includes the resolution kind
and its derived confidence:

```json
{
  "resolution_kind": "receiver_type",
  "confidence": 90
}
```

This is important for agents. They can decide when to trust an edge and when to inspect source directly — but nothing is hidden from them without asking.

## Migration Plan

### Phase 0: Baseline And Fixtures

Goal: make the current failure mode measurable.

Add fixtures for:

- Go: two unrelated types with `GetList`, `Add`, `Run`, plus one receiver-typed call.
- Java: two classes with `release`, `tryFailure`, `run`, plus an imported static/qualified call.
- C: one direct function, one macro-like invocation, one function pointer call.

Add corpus probes:

- Kubernetes: PR 139848 symbols.
- Netty: PR 16959 symbols.
- Linux: `strncpy` removal surface check.

Success criteria:

- tests demonstrate current noisy behavior before resolver changes;
- benchmark scripts can count high/low confidence edges after changes.

### Phase 1: RawCall Context Without Resolution Changes

Goal: enrich raw call records while preserving existing edge output.

Implement nullable raw-call fields:

- `call_kind`;
- `receiver_expr`;
- `receiver_type`;
- `package_or_namespace`;
- `import_hint`.

Populate only cheap facts:

- Java package/imports, receiver identifier, constructor type, parameter/local explicit types.
- Go package/import aliases and selector receiver text.
- C/C++ explicit namespaces and macro/indirect classification where obvious.

Success criteria:

- `make check` and `make test`;
- raw-call rows contain useful context on Netty/Kubernetes sample files;
- no edge-count parity requirement yet because resolver behavior is unchanged.

### Phase 2: Confidence-Aware Resolver

Goal: stop presenting weak guesses as strong graph facts.

Extend `SymbolIndex`:

- `by_qualified_name`;
- `by_owner_and_name`;
- `by_package_and_name`;
- keep existing `by_name`, `by_name_file`, `by_id`.

Implement staged resolution.

Add the `resolution_kind` column to `edges` and bump `SCHEMA_VERSION`. Because
the DB is a derived cache that rebuilds on a version bump, this needs no data
migration — old DBs are dropped and re-indexed from source. Confidence is
derived from `resolution_kind` on read, not stored.

Success criteria:

- Netty PR 16959 still resolves `releaseAndFailQueuedWrite` callers exactly.
- Kubernetes common-name callees drop from default high-confidence output.
- Unique global functions still resolve as before.
- Low-confidence/ambiguous refs are visible in JSON for audit.

### Phase 3: Query Output And Defaults

Goal: make improved data useful to humans and agents.

Add:

- `--min-confidence` (opt-in filter);
- `--show-resolution`.

Default command behavior:

- text output ranks high-confidence edges first and annotates each with its kind;
- JSON includes `resolution_kind` + derived `confidence`;
- no edges are hidden by default — name-only matches are ranked last, not dropped.

Success criteria:

- existing user workflows still work;
- agents can choose high-confidence-only exploration;
- Netty/Kubernetes examples are easier to explain from graph output.

### Phase 4: Incremental Sync Compatibility

Goal: preserve the hard-won selective re-resolution behavior.

Update selective invalidation to consider new raw-call fields:

- changed symbol name still invalidates by `callee_simple`;
- changed owner/type/package should invalidate receiver/type/package-indexed raw calls;
- ambiguous refs should be rechecked when new matching symbols appear.

Success criteria:

- selective sync edge set matches fresh rebuild for focused fixtures;
- existing parity-style test remains;
- giant-corpus validation uses `ochna status --json`, not huge file dumps.

### Phase 5: Optional Previous-Index / Patch Mode

Goal: explain deletion-heavy changes like Linux `strncpy` removal.

This is separate from call resolution and should not block phases 1-4.

Possible feature:

```bash
ochna diff --before .ochna/base.db --after .ochna/ochna.db
```

or a script-level workflow:

```bash
gh api repos/torvalds/linux/commits/<sha>
ochna search strncpy --json
```

Success criteria:

- removed symbols can be reported explicitly;
- current index stays simple and fast.

## Performance Considerations

The plan should not undo the 0.0.4 performance work.

Rules:

- intern repeated strings in memory during resolution;
- keep raw-call context nullable and indexed only where needed;
- avoid SQL lookups per call in the resolver;
- build all resolution maps once per run;
- preserve fresh-build bulk insert behavior;
- preserve selective incremental re-resolution.

Likely indexes:

```sql
CREATE INDEX idx_raw_calls_callee_simple ON raw_calls(callee_simple);
CREATE INDEX idx_raw_calls_receiver_type ON raw_calls(receiver_type);
CREATE INDEX idx_raw_calls_package ON raw_calls(package_or_namespace);
```

Add indexes only after measuring. `receiver_expr` may not need an index if it is only used in memory during a changed-file parse.

## Risk Register

| Risk | Mitigation |
| --- | --- |
| Schema change to `edges`/`raw_calls` | DB is a derived cache: bump `SCHEMA_VERSION`, drop + re-index from source — no in-place migration needed |
| `resolution_kind` bloats edge rows | Store as a small INTEGER enum, derive confidence on read — no second column |
| Resolver gets too clever and slow | Stage maps in memory; no per-call SQL |
| False confidence is worse than noisy edges | Prefer unresolved/ambiguous over low-confidence guessed edges |
| Java local type inference becomes a rabbit hole | Start with parameters, fields, explicit locals, constructor assignments |
| Go receiver inference becomes incomplete | Preserve receiver expression even when type is unknown |
| C macro/function ambiguity remains | Mark lower-confidence instead of pretending it is exact |
| Incremental sync misses incoming edges | Extend selective invalidation with raw-call context and parity tests |

## Recommended First Implementation Slice

The smallest high-value slice is Java + confidence metadata:

1. Add raw-call fields.
2. Populate Java package/import/receiver expression for method invocations.
3. Add `by_qualified_name` and `by_owner_and_name` to `SymbolIndex`.
4. Resolve:
   - exact qualified calls;
   - receiver type when parameter/local type is known;
   - same class;
   - unique global;
   - ambiguous as unresolved/ambiguous.
5. Validate on Netty PR 16959 and existing Java Spring route tests.

Why Java first:

- Netty showed the strongest immediate payoff.
- Java AST carries class/package/import structure cleanly.
- It is the user's most common ochna use case.
- Success is easy to explain and demonstrate before touching Go/C.

Second slice: Go selector receiver expression and package alias resolution.

Third slice: C/C++ confidence classification and deletion-aware workflow.

## Edge-Storage Decision

The candidate designs for where resolution metadata lives:

Option A (chosen): add a `resolution_kind` INTEGER enum directly on `edges`.

- no join on the hot query path;
- PK stays `(source_nid, target_nid, kind)` — one edge per relation, holding its
  best resolution kind;
- schema change is cheap because the DB rebuilds on a version bump.

Option B (rejected): a side `edge_metadata` table.

- its only real advantage — "lower migration risk" — does not apply: the DB is a
  derived cache with no data to migrate;
- every edge needs a `resolution_kind`, so it saves no rows; it duplicates the
  3-column key per row and adds a join, fighting the 0.0.4 size/latency work.

Decision: **Option A**, with `resolution_kind` as a small INTEGER enum and
confidence derived from it on read (never stored).

## Final Argument

Ochna should not try to become `gopls`, `javac`, `clangd`, and `rust-analyzer` in one binary. Its advantage is being fast, local, structural, and language-broad. The right next step is not full semantic compilation. It is a better data contract:

- preserve cheap context at parse time;
- resolve in stages;
- label certainty;
- expose uncertainty to users and agents.

That makes ochna more honest and more useful. It turns graph output from "a list of plausible edges" into "a ranked map of evidence." For agent workflows, that distinction matters.
