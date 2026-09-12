# Dive: Kubernetes Horizontal Pod Autoscaler (via ochna)

A worked example of using ochna to investigate an unfamiliar subsystem in a large
codebase, and an honest log of where the tool helped vs. fought back.

- **Target:** `clones/kubernetes` @ `b58546d` (HPA in `pkg/controller/podautoscaler/`)
- **Index:** 12,890 files · 122,380 nodes · 153,146 edges
- **Tooling:** ochna only (run from repo root via `ochna -C clones/kubernetes ...`), no `rg`/file-opens.

## Plan

Understand the HPA controller end to end — entry points, reconcile loop, metric
collection, scale computation — using the intended ochna flow and noting where it
pays off vs. where a fallback is needed.

1. **Locate the controller** — `search HorizontalController` to find the core type + its methods.
2. **Find the reconcile loop** — locate the worker/reconcile entry; read it with `node --include-code`.
3. **Walk outward via the graph** — `callers` on the constructor (wiring) and reconcile method (cross-file edges).
4. **Drill the algorithm** — locate `computeReplicas*` / the replica calculator, read the math, chase callers.
5. **Metrics path** — trace the metrics client boundary.
6. Capture pitfalls/right choices, then synthesize the report.

## Execution log (queries that mattered)

```text
ochna -C clones/kubernetes search HorizontalController          # → type + 30 methods, all in horizontal.go
ochna -C clones/kubernetes --no-tests callers NewHorizontalController
                                                                # → newHorizontalPodAutoscalerController (cmd/kube-controller-manager)
ochna -C clones/kubernetes --no-tests callers reconcileAutoscaler   # → reconcileKey
ochna -C clones/kubernetes --no-tests callers reconcileKey          # → processNextWorkItem
ochna -C clones/kubernetes --no-tests search computeReplicas
ochna -C clones/kubernetes --no-tests callers computeReplicasForMetrics  # → reconcileAutoscaler
ochna -C clones/kubernetes --no-tests callers GetResourceReplicas   # → computeStatusForResourceMetricGeneric
ochna -C clones/kubernetes --no-tests callers calcPlainMetricReplicas
ochna -C clones/kubernetes --no-tests node --symbol calcPlainMetricReplicas --include-code
```

Roughly 8 targeted queries replaced ~10 file-opens and several recursive greps.

## Pitfalls & right choices

**Pitfalls**

1. **Bare-name collision in `callers`/`search`.** ochna resolves by *symbol name*, not
   receiver type/package. `callers worker` returned hits from a dozen controllers
   (deployment, cronjob, daemon, disruption…). The `worker → processNextWorkItem → reconcileKey`
   loop could only be *partially* confirmed: `reconcileKey ← processNextWorkItem` resolved
   cleanly (distinctive name), but `processNextWorkItem ← worker ← Run` had to be inferred
   from the standard controller pattern + same-file adjacency. **Biggest caveat of the dive.**
2. **Test noise by default.** `search HorizontalController` mixed in `horizontal_test.go`
   symbols; `--no-tests` should be reflexive on a dive.
3. **`search` is a definition index, not a usage finder.** Great for "where is this declared",
   useless for "where is this field read" — still `rg` territory.

**Right choices**

1. **Query distinctive names, pivot from there** (`reconcileAutoscaler`,
   `computeReplicasForMetrics`, `GetResourceReplicas`, `calcPlainMetricReplicas`) — each
   resolved to one node and chained cleanly across files.
2. **`callers` for cross-package wiring** — `NewHorizontalController ← newHorizontalPodAutoscalerController`
   in one query; `rg` makes you grep several times.
3. **`node --include-code`** instead of opening the file — signature + doc + exact line range.
4. **`-C clones/kubernetes`** — whole dive from the ochna repo root, no `cd`.

**Verdict:** strong for structural skeleton + call chains between distinctively-named
functions; weak whenever a name is generic (use a qualified pivot or fall back to `rg`).

## Report — How HPA works

### Where it lives

`pkg/controller/podautoscaler/`: orchestration in `horizontal.go`, scaling math in
`replica_calculator.go`, metrics access in the `metrics/` sub-package.

### Wiring & lifecycle

- **Construction:** `NewHorizontalController` (`horizontal.go:138`), started by
  `newHorizontalPodAutoscalerController` (`cmd/kube-controller-manager/app/autoscaling.go:43`)
  — i.e. launched by kube-controller-manager like every built-in controller.
- **Run loop:** `Run` (`:219`) → `worker` (`:338`) → `processNextWorkItem` (`:345`) →
  `reconcileKey` (`:576`) → **`reconcileAutoscaler`** (`:853`).
  *(reconcileKey←processNextWorkItem confirmed by ochna; links above it are the standard
  controller pattern — see pitfall #1.)*

### Reconcile → desired replicas

```text
reconcileAutoscaler            horizontal.go:853   per-HPA reconcile
 └─ computeReplicasForMetrics  horizontal.go:378   loop over metric specs, take max
     └─ computeReplicasForMetric        :504       dispatch by metric type
         └─ computeStatusForResourceMetricGeneric :686
             └─ GetResourceReplicas  replica_calculator.go:80   ── crosses file
                 └─ calcPlainMetricReplicas        :193   the math
```

The controller evaluates every metric in the HPA spec and takes the **max** desired count.

### The scaling formula (`calcPlainMetricReplicas`, `replica_calculator.go:193`)

- `usageRatio = currentUsage / targetUsage` (`:212`).
- **Base formula:** `desired = ceil(usageRatio * readyPodCount)` (`:223`).
- **Tolerance band:** if `tolerances.isWithin(usageRatio)`, return current replicas (`:217`) — anti-flap.
- **Unready/missing pods** handled conservatively (`:226-245`): scale-up counts them as 0%,
  scale-down counts missing as target; ratio recomputed (`:248`); a recompute that would
  *flip scale direction* is suppressed (`:250`).

### Post-computation shaping (`horizontal.go`)

- `normalizeDesiredReplicas` (`:1138`) — min/max bounds + disabled conditions.
- Behavior/rate: `convertDesiredReplicasWithBehaviorRate` (`:1346`), history in `storeScaleEvent` (`:1235`).
- Stabilization window: `stabilizeRecommendation` (`:1112`) / `stabilizeRecommendationWithBehaviors` (`:1286`).
- Tolerances: `tolerancesForHpa` (`:1604`). Status: `setStatus` (`:1555`) / `updateStatusIfNeeded` (`:1575`).

### Metrics source

`computeStatusForResourceMetricGeneric` reads usage via `GetResourceMetric`
(`podautoscaler/metrics/client.go:67`) — the boundary to metrics-server / custom-metrics APIs.

### Mental model

> A worker dequeues an HPA key → `reconcileAutoscaler` fetches current scale and, per metric
> spec, asks the replica calculator for a desired count (`ceil(currentReplicas · usage/target)`,
> with tolerance + unready/missing guards) → takes the max across metrics → clamps through
> min/max, behavior rate limits, and a stabilization window → writes status and scales the target.
