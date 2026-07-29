# Architecture Decision Records

Durable architectural decisions for ochna, one file per decision, numbered
sequentially. An ADR is a record of a *choice* (context, decision,
consequences, alternatives considered) — not a design doc, a plan, or an
experiment log.

Template:

```markdown
# ADR-NNNN: Title

Status: Proposed | Accepted | Implemented (vX.Y.Z) | Deferred | Rejected
Date: original decision date — recorded YYYY-MM-DD

## Context
...

## Decision
...

## Consequences
...

## Alternatives considered
...
```

## What doesn't belong here

- `docs/experiments/*.md` — point-in-time runs against a pinned corpus
  commit. They're evidence a decision was tested against, not decisions
  themselves; rewriting them as ADRs would misrepresent what they are.
- `docs/dives/*.md` — raw investigation notes that motivated a decision
  (e.g. `docs/dives/kubernetes-hpa.md` fed ADR-0004). Keep them as source
  material, referenced from the ADR, not folded into it.
- `docs/v0.3-contract.md` — the living, frozen command-surface spec itself.
  It's what a decision produced, not the decision record. See ADR-0005 for
  the decision to freeze it.

## Index

| ADR | Title | Status |
| --- | --- | --- |
| [0001](0001-confidence-tiered-call-resolution.md) | Confidence-tiered call resolution | Implemented (v0.3.0) |
| [0002](0002-indexing-pipeline-performance-strategy.md) | Indexing pipeline performance strategy | Implemented (v0.0.4) |
| [0003](0003-agent-ergonomics-surface.md) | Agent ergonomics surface | Partially implemented (v0.1.0) |
| [0004](0004-dive-ergonomics-query-surface.md) | Dive-ergonomics query surface | Implemented (v0.2.0) |
| [0005](0005-v0.3-contract-freeze-and-mcp-adapter-deferral.md) | v0.3 contract freeze and MCP adapter deferral | Accepted |
| [0006](0006-freshness-checks-reuse-the-indexing-pipeline.md) | Freshness checks must reuse the indexing pipeline | Accepted (v0.3.1) |
