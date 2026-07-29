#!/usr/bin/env python3
"""Determined-state vs actual-state checks against the real test giants.

Formalizes the manual acceptance runs recorded in
docs/experiments/v0.3-acceptance-{netty,kubernetes,linux}.md and the pinned
corpus table in docs/v0.3-contract.md into repeatable assertions, so a parser
or resolution regression on a real giant is caught by `make case-sim`
instead of only being noticed the next time someone re-runs those experiments
by hand. Operates directly on the checked-out `clones/<giant>` submodule
(consistent with scripts/benchmark_report.py and AGENTS.md's documented
workflow) rather than the docs' original archive-to-temp-dir approach.

Two kinds of case:
- Known cases (netty/kubernetes/linux): assert ochna's *determined* state
  (live command output) against the *actual* state recorded in the
  acceptance docs for real historical PRs/removals on the pinned commit.
- Smoke cases (every other giant, including new ones like ghostty/flink):
  no known-answer key yet, so just assert ochna doesn't crash and produces a
  structurally sane doctor/status envelope on a real, large, diverse
  codebase -- this alone catches parser crashes/regressions that synthetic
  fixtures wouldn't.

Usage: scripts/case_simulation.py [giant ...]   (default: all giants)
       OCHNA_BIN=/path/to/ochna scripts/case_simulation.py
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
BIN_PATH = Path(os.environ.get("OCHNA_BIN", REPO_ROOT / "target" / "release" / "ochna"))

ALL_GIANTS = [
    "tokio",
    "netty",
    "spring-petclinic",
    "kubernetes",
    "linux",
    "zig",
    "ghostty",
    "flink",
]
KNOWN_CASES = {"netty", "kubernetes", "linux"}


class CaseFailure(AssertionError):
    pass


def run_json(args: list[str], cwd: Path) -> dict:
    result = subprocess.run([str(BIN_PATH), *args], cwd=cwd, capture_output=True, text=True)
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise CaseFailure(f"{args}: stdout was not JSON: {result.stdout!r} / stderr: {result.stderr!r}") from exc


def ensure_indexed(giant_dir: Path) -> None:
    db_path = giant_dir / ".ochna" / "ochna.db"
    cmd = "sync" if db_path.is_file() else "init"
    result = subprocess.run([str(BIN_PATH), cmd], cwd=giant_dir, capture_output=True, text=True)
    if result.returncode != 0:
        raise CaseFailure(f"{cmd} failed: {result.stderr.strip()}")


def check(condition: bool, message: str) -> None:
    if not condition:
        raise CaseFailure(message)


def case_netty(giant_dir: Path) -> list[str]:
    """PR 16959: releaseAndFailQueuedWrite's 3 handlerRemoved callers, and the
    documented tests-for blind spot (indirect-only coverage -> empty result)."""
    passed = []

    callers = run_json(
        ["--json", "callers", "releaseAndFailQueuedWrite", "--show-resolution"], giant_dir
    )
    names = {c["qualified_name"] for c in callers}
    prefix = "io.netty.handler.traffic::"
    expected = {
        f"{prefix}ChannelTrafficShapingHandler::handlerRemoved",
        f"{prefix}GlobalChannelTrafficShapingHandler::handlerRemoved",
        f"{prefix}GlobalTrafficShapingHandler::handlerRemoved",
    }
    check(
        expected <= names,
        f"expected {expected} in callers(releaseAndFailQueuedWrite), got {names}",
    )
    check(
        all(c["confidence"] == 80 for c in callers if c["qualified_name"] in expected),
        "expected the 3 handlerRemoved callers at confidence 80 (package resolution)",
    )
    passed.append("callers(releaseAndFailQueuedWrite) == 3 expected handlerRemoved overrides @ confidence 80")

    tests_for = run_json(["--json", "tests-for", "releaseAndFailQueuedWrite"], giant_dir)
    check(tests_for["ok"] is True, "tests-for releaseAndFailQueuedWrite should resolve the target")
    check(
        tests_for["data"]["target"]["id"].endswith("::releaseAndFailQueuedWrite"),
        "tests-for target should resolve to releaseAndFailQueuedWrite",
    )
    check(
        tests_for["data"]["tests"] == [],
        "documented blind spot: indirect-only test coverage should report no direct-call evidence",
    )
    passed.append("tests-for(releaseAndFailQueuedWrite) == documented blind spot (empty, honest)")

    return passed


def case_kubernetes(giant_dir: Path) -> list[str]:
    """PR 139848: the two rewritten test files map to their known test
    symbols, and the GetList common-name stress case refuses to guess."""
    passed = []

    whitebox = run_json(
        [
            "--json",
            "node",
            "--file",
            "staging/src/k8s.io/apiserver/pkg/storage/cacher/cacher_whitebox_test.go",
            "--symbols-only",
        ],
        giant_dir,
    )
    whitebox_names = {s["name"] for s in whitebox}
    check(
        {"TestShouldDelegateList", "TestMatchExactResourceVersionFallback"} <= whitebox_names,
        "cacher_whitebox_test.go should contain the PR's known test symbols",
    )
    passed.append("cacher_whitebox_test.go symbols include the PR 139848 test functions")

    watch_cache = run_json(
        [
            "--json",
            "node",
            "--file",
            "staging/src/k8s.io/apiserver/pkg/storage/cacher/watch_cache_storage_test.go",
            "--symbols-only",
        ],
        giant_dir,
    )
    watch_cache_names = {s["name"] for s in watch_cache}
    check(
        {"TestWatchCacheStorageMarkConsistent", "TestWatchCacheStorageMatchExactResourceVersionFallback"}
        <= watch_cache_names,
        "watch_cache_storage_test.go should contain the PR's new test functions",
    )
    passed.append("watch_cache_storage_test.go symbols include the PR 139848 new tests")

    bare = run_json(["impact", "GetList", "--direction", "callers", "--json"], giant_dir)
    check(bare["ok"] is False, "bare ambiguous GetList should refuse to guess (ok: false)")
    check(
        any(w["code"] == "ambiguous_symbol" for w in bare["warnings"]),
        "bare ambiguous GetList should report an ambiguous_symbol warning",
    )
    passed.append("impact GetList (bare, ambiguous) refuses to guess across 9+ candidates")

    scoped = run_json(
        ["impact", "CacheDelegator::GetList", "--direction", "callers", "--json"], giant_dir
    )
    check(scoped["ok"] is True, "disambiguated CacheDelegator::GetList should resolve")
    check(
        any(
            t["test"]["id"].endswith("::TestMatchExactResourceVersionFallback")
            for t in scoped["data"]["affected_tests"]
        ),
        "CacheDelegator::GetList impact should reconnect to TestMatchExactResourceVersionFallback",
    )
    passed.append("impact CacheDelegator::GetList reconnects to the PR's rewritten test")

    return passed


def case_linux(giant_dir: Path) -> list[str]:
    """strncpy removal: the core kernel API is gone from its former files and
    not conflated with the distinct strncpy_from_user/nolibc/safe_strncpy APIs."""
    passed = []

    search = run_json(["--json", "search", "strncpy"], giant_dir)
    exact = [s for s in search if s["name"] == "strncpy"]
    check(
        all(s["file_path"].endswith("tools/include/nolibc/string.h") for s in exact),
        f"the only exact-name 'strncpy' match should be the nolibc helper, got {[s['file_path'] for s in exact]}",
    )
    passed.append("search strncpy: only exact match is the distinct nolibc helper, not the removed core API")

    for path in ["lib/string.c", "include/linux/string.h", "include/linux/fortify-string.h"]:
        symbols = run_json(["--json", "node", "--file", path, "--symbols-only"], giant_dir)
        names = {s["name"] for s in symbols}
        check("strncpy" not in names, f"{path} should not define strncpy (removed)")
    passed.append("strncpy absent from all 3 of its former defining files")

    return passed


CASES = {
    "netty": case_netty,
    "kubernetes": case_kubernetes,
    "linux": case_linux,
}


def smoke_case(giant_dir: Path, giant: str) -> list[str]:
    doctor = run_json(["doctor", "--json"], giant_dir)
    check(doctor["data"]["schema"]["match"] is True, f"{giant}: schema mismatch")
    check(doctor["data"]["freshness"] == "fresh", f"{giant}: expected fresh immediately after sync")
    verdict = doctor["data"]["graph_quality"]["trust_verdict"]
    check(verdict in {"trusted", "degraded"}, f"{giant}: unexpected trust_verdict {verdict!r}")

    status = run_json(["status", "--json"], giant_dir)
    check(status["counts"]["files"] > 0, f"{giant}: zero files indexed")
    check(status["counts"]["nodes"] > 0, f"{giant}: zero nodes indexed")

    return [f"doctor/status: schema match, fresh, trust_verdict={verdict}, {status['counts']['files']} files indexed"]


def main() -> int:
    requested = sys.argv[1:] or ALL_GIANTS
    failures: list[tuple[str, str]] = []

    for giant in requested:
        giant_dir = REPO_ROOT / "clones" / giant
        if not giant_dir.is_dir():
            print(f"skip {giant} (submodule not checked out)", file=sys.stderr)
            continue

        print(f"== {giant} ==", file=sys.stderr)
        try:
            ensure_indexed(giant_dir)
            results = CASES[giant](giant_dir) if giant in CASES else smoke_case(giant_dir, giant)
        except CaseFailure as exc:
            print(f"FAIL: {exc}", file=sys.stderr)
            failures.append((giant, str(exc)))
            continue

        for line in results:
            print(f"  PASS: {line}", file=sys.stderr)

    print("", file=sys.stderr)
    if failures:
        print(f"{len(failures)} case(s) failed:", file=sys.stderr)
        for giant, message in failures:
            print(f"  - {giant}: {message}", file=sys.stderr)
        return 1

    print(f"all {len(requested)} case(s) passed", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
