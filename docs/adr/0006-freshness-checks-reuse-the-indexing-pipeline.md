# ADR-0006: Freshness checks must reuse the indexing pipeline

Status: Accepted (v0.3.1)
Date: 2026-07-29

## Context

`status` and `doctor` both need to answer "does the index still match the
workspace" — but they arrived at that answer through two different,
independently-maintained implementations, and both turned out to be wrong
in different ways:

- `status`'s `classify_freshness` compared `git status --porcelain`
  clean/dirty state and commit SHAs. Any git-dirty file *anywhere* in the
  workspace forced `freshness: "stale"`, even a file ochna had already
  indexed whose on-disk content matched exactly — found via `clones/linux`,
  which has permanent, legitimate git-dirty state on case-insensitive
  filesystems (the kernel tree genuinely contains case-colliding path pairs
  like `xt_DSCP.h`/`xt_dscp.h` across its history).
- `doctor`'s `indexed_sources_are_fresh` got the git-dirty case right
  (content-hash comparison, not git state) but had its own gap: a file
  that's *discovered* (matches a supported extension) but never
  *indexed* — because `fs::read_to_string` failed on it, or its extension
  didn't resolve to a grammar, or it hit a genuine parser error — was
  always treated as staleness. Since `init`/`sync` would skip that same
  file identically on every future run, the workspace could never actually
  reach `ok: true`, no matter how many times it was synced. Found via
  `clones/zig`, which vendors 2 legacy Latin-1 (non-UTF-8) libc headers.

Both bugs were only found by running `scripts/case_simulation.py` (ADR
material of its own scope: a determined-state-vs-actual-state harness
against real giants, not synthetic fixtures) against real, large,
diverse corpora — neither was reachable from the existing unit-test
fixtures, which are all small and entirely valid UTF-8.

## Decision

`status` now shares `doctor`'s `indexed_sources_are_fresh` check instead of
its own git-comparison path, so there is exactly one freshness
implementation, not two that can independently drift out of sync with each
other.

That shared check's "was this discovered file supposed to be indexed"
question is answered by *re-running the same three-step decision*
`run_init`/`run_sync` already make for every file — read as UTF-8, resolve
a language from the extension, parse — rather than re-implementing a
narrower proxy for it (e.g. just checking readability, which was the first,
insufficient fix attempted here). A discovered-but-unindexed file only
counts as real staleness if that pipeline would actually succeed on it now;
if it fails identically to how it failed at index time, no sync could ever
change the outcome, so it isn't staleness.

`language_for_path` (previously private to `index.rs`) was made
`pub(crate)` and re-exported from `commands` so `status.rs` could call the
real function instead of duplicating its extension-to-grammar mapping.

## Consequences

- A freshness/trust-verdict check must never re-derive "would this file be
  indexed" via a parallel implementation of any subset of the indexing
  pipeline's decision — it must call the pipeline's own functions. This is
  the general rule this ADR exists to record, not just the specific fix.
- `clones/linux` and `clones/zig` are now permanent regression coverage for
  the two failure modes (git-dirty-but-content-matching; discovered-but-
  unindexable) via `make case-sim`, in addition to the two unit-test
  regressions added directly (`test_status_json_fresh_immediately_after_
  init_on_dirty_worktree`,
  `indexed_sources_are_fresh_excuses_unreadable_non_utf8_files`).
- A future third failure mode in the indexing pipeline (a new kind of
  skip/error) is automatically covered by this check without a corresponding
  freshness-logic change, because it delegates to the pipeline rather than
  re-implementing it.

## Alternatives considered

- Special-casing "unreadable (non-UTF-8)" as the one excused failure mode —
  the first fix attempted here. Rejected once it became clear a parser
  error or an unmappable extension would hit the identical permanent-
  staleness trap through a different path; the pattern, not the specific
  symptom, needed fixing.
- Keeping `status` and `doctor` as two independent freshness
  implementations, patching each bug in both places — rejected: that's how
  the two implementations diverged in the first place.
