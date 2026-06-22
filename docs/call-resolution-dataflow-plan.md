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

These findings came from using the installed `ochna` binary against the checked-out benchmark submodules. All three corpora were shallow enough that local parent history was missing, so the reliable workflow was:

1. verify the local ochna index with `ochna status --json`;
2. use local `git log --oneline -1` only to identify the current commit/PR;
3. use `gh api` for PR or commit metadata and patch files;
4. use ochna for current-tree symbols, line spans, callers, and callees.

### Kubernetes

PR `kubernetes/kubernetes#139848` rewrote apiserver watch-cache tests to use real snapshots.

Local index state:

```json
{
  "files": 12890,
  "nodes": 122380,
  "edges": 339798,
  "git": {
    "branch": "master",
    "commit_sha": "b58546d7b34d0217171dd0d36a6e60c2eb603a77",
    "commit_subject": "Merge pull request #139848 from serathius/watchcache-rewrite-should-delegate-list",
    "status": "dirty"
  }
}
```

The submodule was shallow to the merge commit:

```bash
git rev-list --parents -n 1 HEAD
# b58546d7b34d0217171dd0d36a6e60c2eb603a77
```

That meant `git log --merges` and local parent diffs were not useful. The PR number in the merge subject was the right bridge to GitHub metadata:

```bash
gh api repos/kubernetes/kubernetes/pulls/139848
gh api repos/kubernetes/kubernetes/pulls/139848/files --paginate
```

PR metadata:

- title: `cacher: rewrite whitebox fallback tests to use real snapshots`;
- labels: `kind/cleanup`, `area/apiserver`, `sig/api-machinery`, `size/L`, `release-note-none`;
- milestone: `v1.37`;
- release note: `NONE`;
- merged at: `2026-06-19T11:06:43Z`;
- changed files:
  - `staging/src/k8s.io/apiserver/pkg/storage/cacher/cacher_whitebox_test.go` (`+47/-107`);
  - `staging/src/k8s.io/apiserver/pkg/storage/cacher/watch_cache_storage_test.go` (`+65/-0`).

What worked:

- `ochna node --file ... --symbols-only --json` found `TestShouldDelegateList`, `TestMatchExactResourceVersionFallback`, and `TestWatchCacheStorageMatchExactResourceVersionFallback`.
- `ochna node --symbol ShouldDelegateList --include-code --json` and `ochna node --symbol GetExactSnapshotLocked --include-code --json` quickly showed the production logic.

What failed:

- `ochna node --symbol TestShouldDelegateList --include-code --json` reported callees with common names from unrelated packages because many Go methods share names like `Run`, `Add`, `Fatal`, and `Stop`.
- The user-visible workaround was to anchor on changed files and exact production symbols instead of trusting broad callees.

Root cause:

- Go selector calls currently store only the field name, e.g. `GetList`, not the receiver expression or inferred receiver type.

Detailed behavior:

- `TestShouldDelegateList` now creates real `example.Pod` values and drives the watch cache with `cacher.watchCache.Add(oldPod)` and `cacher.watchCache.Update(latestPod)` instead of injecting fake snapshots through `cacher.watchCache.storage.snapshots`.
- `TestMatchExactResourceVersionFallback` no longer uses a fake snapshotter table. It tests `SnapshotAvailable=false` and `SnapshotAvailable=true` through `NewCacheDelegator(...).GetList(...)`.
- The new `TestWatchCacheStorageMatchExactResourceVersionFallback` directly verifies `watchCacheStorage.GetExactSnapshotLocked`:
  - no snapshot returns `ResourceExpired`;
  - adding RV 20 makes RV 20 retrievable;
  - compacting to RV 30 expires RV 20;
  - RV 30 remains retrievable.

Ochna was good at locating these exact symbols. It was weaker when asked for broad callees from a large Go test because raw call data did not know that `cacher.watchCache.Add` was not every `Add` in the repository.

### Linux

Merge `1a3746ccb` removed core kernel `strncpy()`.

Local index state:

```json
{
  "files": 65221,
  "nodes": 1403920,
  "edges": 2314240,
  "git": {
    "branch": "master",
    "commit_sha": "1a3746ccbb0a97bed3c06ccde6b880013b1dddc1",
    "commit_subject": "Merge tag 'strncpy-removal-v7.2-rc1' of git://git.kernel.org/pub/scm/linux/kernel/git/kees/linux",
    "status": "dirty"
  }
}
```

The submodule was also shallow to one commit:

```bash
git rev-list --parents -n 1 HEAD
# 1a3746ccbb0a97bed3c06ccde6b880013b1dddc1
```

Unlike Kubernetes/Netty, this was not PR-shaped. The correct metadata source was the GitHub commit API:

```bash
gh api repos/torvalds/linux/commits/1a3746ccbb0a97bed3c06ccde6b880013b1dddc1
```

Commit metadata:

- subject: `Merge tag 'strncpy-removal-v7.2-rc1'`;
- author date: `2026-06-19T21:56:45Z`;
- files changed: `19`;
- message summary:
  - remove per-arch `strncpy` implementations in alpha, m68k, powerpc, x86, and xtensa;
  - remove `strncpy` API;
  - close out a six-year migration effort across hundreds of commits.

What worked:

- `ochna search strncpy --json` showed that core `lib/string.c::strncpy` and `include/linux/string.h::strncpy` were absent after the merge.
- It still found distinct APIs such as `strncpy_from_user`, which correctly remain.

What failed or needed external data:

- The most important changes were deletions of declarations, implementations, FORTIFY wrappers, and assembly implementations. A symbol graph alone cannot explain a removed symbol without patch metadata or a previous index.

Root cause:

- Current index represents the current tree. It does not compare against a prior tree or store removed symbols.

Detailed patch findings:

- `Documentation/process/deprecated.rst` changed from warning about `strncpy()` to stating that `strncpy()` has been removed from the kernel.
- Replacement guidance now points to:
  - `strscpy()` for NUL-terminated destinations;
  - `strscpy_pad()` for NUL-terminated and zero-padded destinations;
  - `memtostr()` / `memtostr_pad()` for fixed-width non-NUL source data;
  - `strtomem()` / `strtomem_pad()` for fixed-width non-NUL destinations;
  - `memcpy_and_pad()` for bounded runtime-size padded copies.
- `include/linux/string.h` removed the `extern char *strncpy(...)` declaration.
- `lib/string.c` removed the `strncpy` implementation and `EXPORT_SYMBOL(strncpy)`.
- `include/linux/fortify-string.h` removed:
  - `FORTIFY_FUNC(strncpy)`;
  - `__underlying_strncpy`;
  - the FORTIFY inline `strncpy` wrapper and its documentation;
  - the final `#undef __underlying_strncpy`.
- `lib/tests/fortify_kunit.c` removed `fortify_test_strncpy` and dropped it from `fortify_test_cases`.
- Two focused fortify test files were removed:
  - `lib/test_fortify/write_overflow-strncpy-src.c`;
  - `lib/test_fortify/write_overflow-strncpy.c`.
- Several architecture files removed declarations or implementations:
  - `arch/alpha/lib/strncpy.S` removed entirely;
  - alpha, m68k, powerpc, x86, and xtensa string headers / assembly no longer expose arch-specific `strncpy`.

Ochna helped after the patch was known:

```bash
ochna search strncpy --json
ochna node --file lib/string.c --symbols-only --json
ochna node --file include/linux/string.h --symbols-only --json
ochna node --file include/linux/fortify-string.h --symbols-only --json
ochna node --file lib/tests/fortify_kunit.c --symbols-only --json
```

The important distinction was that `strncpy` itself was gone from the kernel API, while related but different APIs remained:

- `strncpy_from_user`;
- `strncpy_from_kernel_nofault`;
- `strncpy_from_user_nofault`;
- `tools/include/nolibc/string.h::strncpy`;
- helper names like `safe_strncpy`.

This is a good example of why deletion-heavy explanations need either patch metadata or two index snapshots. A current-tree graph cannot tell the story alone.

### Netty

PR `netty/netty#16959` fixed queued traffic-shaping writes leaking and leaving promises incomplete on close.

Local index state:

```json
{
  "files": 3561,
  "nodes": 41087,
  "edges": 104893,
  "git": {
    "branch": "4.2",
    "commit_sha": "ec4efdbbeebf024b64e0fb782184989835c9ab92",
    "commit_subject": "Correctly release and fail queued traffic-shaping writes on close (#16959)",
    "status": "dirty"
  }
}
```

The submodule was shallow to the PR merge commit:

```bash
git rev-list --parents -n 1 HEAD
# ec4efdbbeebf024b64e0fb782184989835c9ab92
```

The commit subject carried the PR number, so metadata came from:

```bash
gh api repos/netty/netty/pulls/16959
gh api repos/netty/netty/pulls/16959/files --paginate
```

PR metadata:

- title: `Correctly release and fail queued traffic-shaping writes on close`;
- base branch: `4.2`;
- merged at: `2026-06-18T16:09:27Z`;
- changed files:
  - `handler/src/main/java/io/netty/handler/traffic/AbstractTrafficShapingHandler.java` (`+9/-0`);
  - `handler/src/main/java/io/netty/handler/traffic/ChannelTrafficShapingHandler.java` (`+5/-5`);
  - `handler/src/main/java/io/netty/handler/traffic/GlobalChannelTrafficShapingHandler.java` (`+5/-5`);
  - `handler/src/main/java/io/netty/handler/traffic/GlobalTrafficShapingHandler.java` (`+5/-5`);
  - `handler/src/test/java/io/netty/handler/traffic/TrafficShapingHandlerTest.java` (`+49/-0`).

What worked very well:

- `ochna node --symbol releaseAndFailQueuedWrite --include-code --json` found the new helper.
- `ochna callers releaseAndFailQueuedWrite --json` found exactly the three handler cleanup callers.
- `ochna node --symbol testQueuedWritesReleasedAndFailedOnClose --include-code --json` found the new regression test.

Why Java was better:

- Java class and method nodes already carry useful `qualified_name` values.
- The changed code used a distinctive helper name, so name-only fallback was still accurate.

Remaining concern:

- Java APIs with overloaded or common method names still need receiver/import/class context to be reliable at Netty/Spring scale.

Detailed behavior:

The bug:

- `AbstractTrafficShapingHandler#calculateSize` supports `ByteBuf`, `ByteBufHolder`, and `FileRegion`.
- Traffic-shaping handlers can queue delayed writes for `ByteBufHolder` messages such as HTTP content.
- On close/handler removal, the handlers previously released only direct `ByteBuf` queued messages.
- Queued `ByteBufHolder` or other `ReferenceCounted` messages could leak.
- Their `ChannelPromise`s were also left incomplete even though the messages would never be written.

The fix:

- `AbstractTrafficShapingHandler` added:

```java
static void releaseAndFailQueuedWrite(Object msg, ChannelPromise promise, Throwable cause) {
    ReferenceCountUtil.safeRelease(msg);
    promise.tryFailure(cause);
}
```

- `ChannelTrafficShapingHandler.handlerRemoved`;
- `GlobalTrafficShapingHandler.handlerRemoved`;
- `GlobalChannelTrafficShapingHandler.handlerRemoved`;

all now:

- create a `ClosedChannelException`;
- call `releaseAndFailQueuedWrite(...)` for queued messages;
- reset local/per-channel queue size after cleanup;
- clear the queue.

The regression test:

- creates an `EmbeddedChannel` with each traffic-shaping handler variant;
- writes a delayed `DefaultByteBufHolder`;
- verifies the promise is not initially done and no outbound write is produced;
- closes the channel;
- verifies reference count drops to `0`;
- verifies the promise fails with `ClosedChannelException`;
- verifies there is no leftover outbound data.

Ochna commands that gave high signal:

```bash
ochna node --file handler/src/main/java/io/netty/handler/traffic/AbstractTrafficShapingHandler.java --symbols-only --json
ochna node --symbol releaseAndFailQueuedWrite --include-code --json
ochna callers releaseAndFailQueuedWrite --json
ochna node --symbol testQueuedWritesReleasedAndFailedOnClose --include-code --json
```

The `callers releaseAndFailQueuedWrite` result was exactly what we want from ochna:

- `ChannelTrafficShapingHandler::handlerRemoved`;
- `GlobalTrafficShapingHandler::handlerRemoved`;
- `GlobalChannelTrafficShapingHandler::handlerRemoved`.

That is the positive model for the redesign: when names are distinctive and ownership is clear, ochna already behaves like a strong structural graph. The redesign should make more common method calls behave like this case.

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

Add metadata to edges, or add a side table keyed by edge:

```text
edges
  source_nid
  target_nid
  kind
  resolution_kind TEXT       -- exact, receiver_type, package, same_file, namespace, name_only
  confidence INTEGER         -- suggested 100, 90, 80, 60, 30
```

If changing the `edges` primary key is too risky, use:

```text
edge_metadata
  source_nid
  target_nid
  kind
  resolution_kind TEXT NOT NULL
  confidence INTEGER NOT NULL
```

The side-table option is lower-risk for migration because query commands can join it only where needed.

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
edge(source, target, "calls", resolution_kind, confidence)
```

For stage 7, there are two policy options:

1. Emit no edge and record an unresolved/ambiguous ref.
2. Emit a low-confidence edge but mark it `name_only`.

Recommendation: emit an `ambiguous_refs` record for multi-candidate name-only cases, and only emit a `name_only` edge when the global candidate is unique. This makes default graph output more trustworthy.

## Query UX

Add filters without breaking existing commands:

```bash
ochna callers GetList --min-confidence 80
ochna callers GetList --include-low-confidence
ochna node --symbol Foo --include-code --show-resolution
ochna explore watchCache --show-resolution
```

Default behavior should hide or visually down-rank low-confidence edges. JSON output should include:

```json
{
  "resolution_kind": "receiver_type",
  "confidence": 90
}
```

This is important for agents. They can decide when to trust an edge and when to inspect source directly.

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

Add `edge_metadata` or edge columns for:

- `resolution_kind`;
- `confidence`.

Recommendation: use `edge_metadata` first if schema churn risk is high.

Success criteria:

- Netty PR 16959 still resolves `releaseAndFailQueuedWrite` callers exactly.
- Kubernetes common-name callees drop from default high-confidence output.
- Unique global functions still resolve as before.
- Low-confidence/ambiguous refs are visible in JSON for audit.

### Phase 3: Query Output And Defaults

Goal: make improved data useful to humans and agents.

Add:

- `--min-confidence`;
- `--include-low-confidence`;
- `--show-resolution`.

Default command behavior:

- text output shows high-confidence edges first;
- JSON includes confidence fields;
- name-only ambiguous matches are not silently mixed with exact edges.

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
| Schema churn breaks existing DBs | Add migration tests and keep nullable fields first |
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

## Decision To Make Before Implementation

The main design decision is edge storage:

Option A: add `resolution_kind` and `confidence` directly to `edges`.

- simpler query joins;
- bigger schema change;
- primary key remains source/target/kind, so multiple evidence paths collapse.

Option B: add `edge_metadata`.

- lower-risk migration;
- can store extra fields later;
- requires joins in query code.

Recommendation: Option B for the first implementation. Once stable, fold into `edges` only if query performance or simplicity demands it.

## Final Argument

Ochna should not try to become `gopls`, `javac`, `clangd`, and `rust-analyzer` in one binary. Its advantage is being fast, local, structural, and language-broad. The right next step is not full semantic compilation. It is a better data contract:

- preserve cheap context at parse time;
- resolve in stages;
- label certainty;
- expose uncertainty to users and agents.

That makes ochna more honest and more useful. It turns graph output from "a list of plausible edges" into "a ranked map of evidence." For agent workflows, that distinction matters.
