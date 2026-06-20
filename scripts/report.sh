#!/usr/bin/env bash
# Real-project benchmark report over the checked-out test giants.
#
# Acts as a quality gate as the project grows: the giants are pinned submodules,
# so files/nodes/edges are reproducible for a given commit. Re-run after a parser
# change and diff the report — a regression shows up as a count delta or a giant
# that fails to index. (The "Index (s)" column is machine-dependent and only
# informational.)
#
# Usage: scripts/report.sh [output.md]   (default: BENCHMARK.md)
#        OCHNA_BIN=/path/to/ochna scripts/report.sh
#        REINDEX=1 scripts/report.sh     (force a clean re-index of each giant)
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

bin="${OCHNA_BIN:-$repo_root/target/release/ochna}"
out="${1:-BENCHMARK.md}"

# giant -> language label
langs="tokio=Rust netty=Java kubernetes=Go linux=C zig=Zig"

{
  echo "# Benchmark report"
  echo
  echo "Generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo
  echo "| Giant | Lang | Commit | Files | Nodes | Edges | Index (s) |"
  echo "| --- | --- | --- | ---: | ---: | ---: | ---: |"
} >"$out"

for pair in $langs; do
  giant="${pair%%=*}"
  lang="${pair##*=}"
  dir="clones/$giant"
  if [ ! -d "$dir" ]; then
    echo "skip $giant (submodule not checked out)" >&2
    continue
  fi

  echo "indexing $giant..." >&2
  secs="-"
  if [ "${REINDEX:-0}" = "1" ] || [ ! -f "$dir/.ochna/ochna.db" ]; then
    start=$(date +%s)
    (cd "$dir" && rm -rf .ochna && "$bin" init >/dev/null 2>&1)
    secs=$(( $(date +%s) - start ))
  fi

  json="$(cd "$dir" && "$bin" status --json)"
  echo "$json" | jq -r --arg g "$giant" --arg l "$lang" --arg s "$secs" \
    '"| \($g) | \($l) | \(.git.commit_sha[0:12]) | \(.files) | \(.nodes) | \(.edges) | \($s) |"' \
    >>"$out"
done

echo "wrote $out" >&2
