# Kubernetes PR 139848 Experiment

This experiment tested whether `ochna` can help explain recent merged work in a large Kubernetes checkout.

## Subject

- Repo: `kubernetes/kubernetes`
- PR: `139848`
- Merge commit in local submodule: `b58546d7b34d0217171dd0d36a6e60c2eb603a77`
- Title: `cacher: rewrite whitebox fallback tests to use real snapshots`
- Labels: `kind/cleanup`, `area/apiserver`, `sig/api-machinery`, `size/L`, `release-note-none`
- Milestone: `v1.37`
- Release note: `NONE`

This was not a user-facing feature. It was a cleanup of apiserver watch-cache tests: the PR rewrote tests to use production watch-cache APIs instead of fake snapshot internals, and added direct coverage for exact resource-version snapshot fallback behavior.

## What Worked

The local Kubernetes submodule already had an ochna index:

- `12890` files
- `122380` nodes
- `339798` edges

The most useful commands were targeted:

```bash
ochna status --json
ochna node --file staging/src/k8s.io/apiserver/pkg/storage/cacher/cacher_whitebox_test.go --symbols-only --json
ochna node --file staging/src/k8s.io/apiserver/pkg/storage/cacher/watch_cache_storage_test.go --symbols-only --json
ochna search TestWatchCacheStorageMatchExactResourceVersionFallback --json
ochna node --symbol ShouldDelegateList --include-code --json
ochna node --symbol GetExactSnapshotLocked --include-code --json
```

`ochna` quickly located the changed test symbols and the production functions they are exercising:

- `TestShouldDelegateList` at `staging/src/k8s.io/apiserver/pkg/storage/cacher/cacher_whitebox_test.go:118`
- `TestMatchExactResourceVersionFallback` at `staging/src/k8s.io/apiserver/pkg/storage/cacher/cacher_whitebox_test.go:545`
- `TestWatchCacheStorageMatchExactResourceVersionFallback` at `staging/src/k8s.io/apiserver/pkg/storage/cacher/watch_cache_storage_test.go:73`
- `ShouldDelegateList` at `staging/src/k8s.io/apiserver/pkg/storage/cacher/delegator/interface.go:38`
- `GetExactSnapshotLocked` at `staging/src/k8s.io/apiserver/pkg/storage/cacher/watch_cache_storage.go:254`

## What The PR Changed

The PR touched two files:

- `staging/src/k8s.io/apiserver/pkg/storage/cacher/cacher_whitebox_test.go`: `+47/-107`
- `staging/src/k8s.io/apiserver/pkg/storage/cacher/watch_cache_storage_test.go`: `+65/-0`

In `TestShouldDelegateList`, the test stopped injecting fake snapshots directly through `cacher.watchCache.storage.snapshots`. It now creates real pod objects and drives the watch cache through `cacher.watchCache.Add` and `cacher.watchCache.Update`. That makes the whitebox test closer to production watch-cache behavior.

In `TestMatchExactResourceVersionFallback`, the old fake snapshotter table was replaced by two direct cases: `SnapshotAvailable=false` and `SnapshotAvailable=true`. The test now checks actual storage request count through `CacheDelegator.GetList` and real watch-cache state.

The new `TestWatchCacheStorageMatchExactResourceVersionFallback` directly verifies `watchCacheStorage.GetExactSnapshotLocked`: no snapshot returns `ResourceExpired`, adding an object at RV 20 makes RV 20 retrievable, compacting to RV 30 makes RV 20 expire, and RV 30 remains retrievable.

## Plan Adjustment

The original plan assumed local `git log --merges` plus parent diffs would identify recent merged work. That did not work in this checkout: the Kubernetes submodule is shallow to a single merge commit, so `git log --merges` returned no entries and `git rev-list --parents -n 1 HEAD` showed no local parent.

The adjusted workflow is:

1. Use `ochna status --json` to verify the local index and baseline counts.
2. Use local `git log --oneline -1` only to identify the merge PR number and local commit.
3. Use `gh api repos/<owner>/<repo>/pulls/<pr>` and `gh api repos/<owner>/<repo>/pulls/<pr>/files` for PR title, body, labels, release note, changed files, and patch intent.
4. Use `ochna node --file ... --symbols-only --json` on changed files to locate indexed symbols cheaply.
5. Use `ochna search <symbol> --json` and `ochna node --symbol <symbol> --include-code --json` for the small number of relevant test and production symbols.
6. Treat broad callees on common Go names like `GetList`, `Run`, `Add`, and `Stop` as noisy unless anchored to the changed file or exact production symbol.

## Repeatable Helper

The helper script captures the adjusted workflow:

```bash
uv run python pyscripts/pr_feature_report.py \
  --workspace clones/kubernetes \
  --repo kubernetes/kubernetes \
  --pr 139848
```

It uses `gh api` for PR metadata/files and the local `.ochna/ochna.db` for changed-file symbols. This is more reliable than local git diffs for shallow benchmark submodules.
