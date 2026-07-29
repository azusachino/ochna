#!/usr/bin/env python3
"""Real-project benchmark report over the checked-out test giants.

Acts as a quality gate as the project grows: the giants are pinned submodules,
so files/nodes/edges are reproducible for a given commit. Re-run after a parser
change and diff against prior runs -- a regression shows up as a count delta,
a trust-verdict drop, or a giant that fails to index. (The "Index (s)" column
is machine-dependent and only informational.)

Trust comes from `doctor --json`'s `graph_quality.trust_verdict`, not
`status --json`. `status`'s freshness check is git-porcelain-based: any
git-dirty file anywhere in a giant (even one unrelated to indexing, already
reflected in the index) makes it report stale. `doctor` instead recomputes
each indexed file's content hash and only calls it stale on an actual content
mismatch, so an incidentally dirty giant is still trustworthy.

Every run appends a new dated section rather than overwriting the file, so
the report is a history, not just the latest snapshot.

Usage: scripts/benchmark_report.py [output.md]   (default: BENCHMARK.md)
       OCHNA_BIN=/path/to/ochna scripts/benchmark_report.py
       REINDEX=1 scripts/benchmark_report.py     (force a clean re-index of each giant)
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

# giant -> language label
GIANTS = {
    "tokio": "Rust",
    "netty": "Java",
    "spring-petclinic": "Spring",
    "kubernetes": "Go",
    "linux": "C",
    "zig": "Zig",
}


def run(args: list[str], cwd: Path) -> None:
    subprocess.run(args, cwd=cwd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def run_json(args: list[str], cwd: Path) -> dict:
    """Run an ochna --json command and parse its stdout regardless of exit
    code: doctor/status intentionally exit non-zero on degraded/stale
    verdicts while still emitting a complete JSON body."""
    result = subprocess.run(args, cwd=cwd, capture_output=True, text=True)
    return json.loads(result.stdout)


def main() -> int:
    repo_root = Path(__file__).resolve().parents[1]
    bin_path = Path(os.environ.get("OCHNA_BIN", repo_root / "target" / "release" / "ochna"))
    out_path = repo_root / (sys.argv[1] if len(sys.argv) > 1 else "BENCHMARK.md")
    reindex = os.environ.get("REINDEX", "0") == "1"
    is_new_file = not out_path.is_file()

    # Appended, not overwritten -- each run is a dated section, so the file
    # is a history rather than only ever the latest snapshot. Written
    # incrementally so a giant that fails partway through still leaves prior
    # giants' rows on disk.
    with out_path.open("a", encoding="utf-8") as out:
        def emit(line: str) -> None:
            out.write(line + "\n")
            out.flush()

        if is_new_file:
            emit("# Benchmark report")
            emit("")

        emit(f"## {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}")
        emit("")
        emit("| Giant | Lang | Commit | Files | Nodes | Edges | Index (s) | Re-sync (s) | Trust |")
        emit("| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | --- |")

        for giant, lang in GIANTS.items():
            giant_dir = repo_root / "clones" / giant
            if not giant_dir.is_dir():
                print(f"skip {giant} (submodule not checked out)", file=sys.stderr)
                continue

            print(f"indexing {giant}...", file=sys.stderr)
            db_path = giant_dir / ".ochna" / "ochna.db"
            secs = "-"
            if reindex or not db_path.is_file():
                shutil.rmtree(giant_dir / ".ochna", ignore_errors=True)
                start = time.monotonic()
                run([str(bin_path), "init"], giant_dir)
                secs = str(round(time.monotonic() - start))

            # No-op incremental re-sync: nothing changed on disk, so the selective
            # re-resolution path should touch zero call sources. Contrasts the old
            # always-global delete-all-edges + re-resolve-all-raw_calls behaviour.
            start = time.monotonic()
            run([str(bin_path), "sync"], giant_dir)
            resync = str(round(time.monotonic() - start))

            try:
                status = run_json([str(bin_path), "status", "--json"], giant_dir)
                doctor = run_json([str(bin_path), "doctor", "--json"], giant_dir)
            except (json.JSONDecodeError, FileNotFoundError) as exc:
                print(f"skip {giant} ({exc})", file=sys.stderr)
                continue

            commit = status["git"]["commit_sha"][:12]
            counts = status["counts"]
            trust = doctor["data"]["graph_quality"]["trust_verdict"]
            emit(
                f"| {giant} | {lang} | {commit} | {counts['files']} | {counts['nodes']} | "
                f"{counts['edges']} | {secs} | {resync} | {trust} |"
            )

        emit("")

    print(f"wrote {out_path}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
