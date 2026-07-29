# ADR-0003: Agent ergonomics surface

Status: Partially implemented — workstreams 1–2 shipped v0.1.0; 3 deferred; 4 rejected from core
Date: 2026-06-24 (original plan) — recorded 2026-07-29

## Context

An agent landing in a repo with a `.ochna/` directory has no way to know
ochna exists, whether its index is fresh, or how to drive it — that
knowledge lived only in this repo's own `AGENTS.md`. Four workstreams were
proposed, ordered by leverage ascending cost. Storage was constrained to
stay single-file SQLite throughout — these are ergonomics and one optional
capability, not a second store (point queries were already 2–4ms even on
the 1.25GB Linux index, so no format change was justified for speed).

## Decision

**1. `ochna howto` + `.ochna/AGENT.md` pointer — implemented.** `howto` is
the single source of usage truth (a command, not a static doc, so it can
never drift from the installed binary and supports `--json` for machine
consumption). `init`/`sync` write a thin, tool-owned `.ochna/AGENT.md` —
provenance (indexed SHA, branch, dirty/clean, `indexed_at`) plus one line
pointing at `howto`/`status`. It holds no usage content itself, so it can't
drift from the command it points to. The repo's root `AGENTS.md` is never
touched — it's human-owned.

**2. `status --json` preflight verdict — implemented.** Folded into the
existing `status` command rather than a new subcommand: an `ok` boolean and
non-zero exit code on failure, so an agent (or shell `&&`) can gate without
parsing. See ADR-0006 for how this command's freshness semantics were later
corrected.

**3. Repo-local findings log — deferred.** The proposal: `.ochna/
findings.jsonl`, append-only, deliberately outside `ochna.db` (which is a
derived cache dropped on schema migration; findings are knowledge that must
survive a reindex), resurfacing contextually when `node`/`search` returns a
symbol with existing findings. Scoped deliberately small — symbol-anchored
provenance, not a knowledge graph; cross-session/cross-repo decisions belong
in an external decision log (this project uses asobi for that), not a
reinvention inside ochna. Shipped 1–2 first to see if the need was real
before adding a write path; the need has not yet been revisited. Tracked as
a v0.4 roadmap candidate.

**4. `sqlite-vec` semantic search — rejected from core, pilot-only.**
Genuinely valuable (natural-language symbol search, "more like this") but
highest cost and risk: brute-force KNN is fine on Netty-scale (a few ms) but
100ms–seconds on Linux-scale without an ANN index or a prefilter; embedding
every node at 384-dim float32 on Linux would be ~2.1GB, larger than the
entire current DB. The dominant cost isn't storage — it's that embedding
generation breaks the pure-Rust, offline, tree-sitter-only simplicity ochna
otherwise has (needs a bundled model or an external API). Never evaluate
this as a performance fix; evaluate it as a new feature on its own merits,
gated behind a `--features semantic` build flag, pilot on Netty/tokio only.

## Consequences

- `howto`/`AGENT.md`/`status --json` are the stable, always-current
  self-description surface every other agent-facing feature (this ADR set
  included) should point at rather than duplicating.
- The findings-log write path remains unbuilt; if revisited, dedup on
  `(symbol, note)` and a file-size cap (mirroring asobi's own retention
  policy) are the open design questions.
- `sqlite-vec` remains unstarted; the blocking decision before any work
  starts is where embeddings come from (bundled local model vs. API).

## Alternatives considered

- A findings log as a full knowledge graph inside ochna — rejected as scope
  creep that would duplicate asobi's decision-log role.
- Bundling `sqlite-vec` unconditionally into core — rejected: the giants'
  scale makes brute-force KNN and unquantized storage genuinely costly, and
  the embedding-source dependency conflicts with ochna's offline-by-default
  design.
