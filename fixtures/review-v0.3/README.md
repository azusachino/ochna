# v0.3 review fixture

This fixture is source data for the v0.3 contract tests. Materialize `before`
as a temporary Git repository and commit it. Replace its contents with `after`
without committing, then run the reviewed command against that temporary
workspace. Do not index both trees together: they model different revisions.

`expectations.json` names the stable acceptance facts. It deliberately includes
one unresolved call and one deleted API so review tooling must distinguish
missing structural evidence from a current symbol with a similar name.
