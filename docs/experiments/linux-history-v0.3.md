# Linux historical-diff acceptance

Date: 2026-07-29

The pinned `clones/linux` checkout is intentionally depth 1 at
`1a3746ccbb0a97bed3c06ccde6b880013b1dddc1`. Acceptance used a shared temporary
clone under `/private/tmp`; the submodule itself was not initialized, fetched,
or modified.

The installed `ochna` binary indexed the temporary checkout in approximately
148 seconds:

- 65,221 source files
- 4,477,237 raw calls
- 1,645,608 resolved call edges
- 2,355,864 unresolved references
- 1.2 GB generated `.ochna` index

The local object store does not contain `HEAD^`. The installed-CLI probe

```text
ochna -C <temporary-linux-copy> diff --base HEAD^ --json
```

completed in approximately 10 ms with a nonzero exit, `ok: false`,
`base_revision_unavailable`, empty symbol and edge deltas, and
`next_action: "provide a base revision"`. This proves the real shallow-corpus
boundary without silently substituting the worktree graph or deepening the
checkout.

Successful deletion-heavy graph comparison is covered by the deterministic C
fixture in `pyscripts/verify_clis.py`: it removes three symbols and at least two
edges, reports temporary-index cost, and verifies cleanup. A successful
full-Linux parent-to-merge comparison remains conditional on both revisions
already being available locally.
