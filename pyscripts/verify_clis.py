#!/usr/bin/env python3
"""Behavioral smoke test for ochna's agent-facing CLI against a real binary.

Builds a known fixture repo, indexes it, then asserts each command returns the
*right* result (not merely that it exits 0): search/no-tests filtering, callers
with confidence + --min-confidence filtering + --show-resolution, node file/slice/
symbol modes, explore, plus howto, status --json preflight, and the AGENT.md
pointer. Every public CLI surface gets a content assertion so a behavioral
regression fails the PR gate.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def run(
    args: list[str],
    cwd: Path,
    *,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=cwd,
        check=check,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def assert_json(stdout: str) -> object:
    try:
        return json.loads(stdout)
    except json.JSONDecodeError as exc:
        raise AssertionError(f"stdout was not JSON: {stdout!r}") from exc


def main() -> int:
    repo_root = Path(__file__).resolve().parents[1]
    bin_path = Path(os.environ.get("OCHNA_BIN", repo_root / "target" / "release" / "ochna"))
    if not bin_path.is_file():
        print(f"missing executable: {bin_path}", file=sys.stderr)
        return 1
    ochna = str(bin_path)

    tmp = Path(tempfile.mkdtemp(prefix="ochna-verify-clis."))
    try:
        run(["git", "init"], tmp)
        run(["git", "config", "user.email", "ochna@example.invalid"], tmp)
        run(["git", "config", "user.name", "Ochna Verify"], tmp)

        # Fixture: `helper` with two in-file callers (resolved call edges) plus
        # a test-path caller for `tests-for` and --no-tests coverage.
        (tmp / "src").mkdir()
        (tmp / "tests").mkdir()
        (tmp / "src" / "lib.rs").write_text(
            "pub fn helper() {}\n\n"
            "pub fn caller_one() {\n    helper();\n    missing();\n    missing_two();\n}\n\n"
            "pub fn caller_two() {\n    helper();\n}\n",
            encoding="utf-8",
        )
        (tmp / "tests" / "extra.rs").write_text(
            "pub fn helper_in_tests() {\n    helper();\n}\n", encoding="utf-8"
        )
        run(["git", "add", "-A"], tmp)
        run(["git", "commit", "-m", "baseline"], tmp)

        # --- howto: self-describing surface (human + json descriptor) ---
        howto = run([ochna, "howto"], tmp).stdout
        assert "ochna usage flow" in howto
        howto_json = assert_json(run([ochna, "howto", "--json"], tmp).stdout)
        assert howto_json["flow"] == [
            "status",
            "search",
            "callers",
            "callees",
            "node",
            "explore",
        ]
        assert "status" in howto_json["commands"]
        assert "callees" in howto_json["commands"]
        assert howto_json["flags"]["min_confidence"]
        assert howto_json["flags"]["show_resolution"]
        assert howto_json["flags"]["include_library"]
        assert howto_json["flags"]["in"]
        assert howto_json["flags"]["limit"]
        assert howto_json["flags"]["workspace"]
        assert "--workspace" in howto_json["globals"]
        assert any("symbols-only" in mode for mode in howto_json["node_modes"])
        assert howto_json["confidence_cascade"][0] == "exact=100"

        # --- init writes the index and the AGENT.md pointer ---
        run([ochna, "init"], tmp)
        pointer = tmp / ".ochna" / "AGENT.md"
        assert pointer.is_file()
        assert "ochna howto" in pointer.read_text(encoding="utf-8")

        # --- doctor/unresolved: review preflight and stable unresolved evidence ---
        doctor_process = run([ochna, "doctor", "--json"], tmp, check=False)
        assert doctor_process.returncode == 0, doctor_process.stdout + doctor_process.stderr
        doctor = assert_json(doctor_process.stdout)
        assert doctor["ok"] is True
        assert doctor["data"]["schema"]["expected"] == doctor["data"]["schema"]["actual"]
        assert doctor["data"]["graph_quality"]["trust_verdict"] == "trusted"
        assert "resolution_tiers" in doctor["data"]["graph_quality"]
        unresolved = assert_json(run([ochna, "unresolved", "--json"], tmp).stdout)
        references = unresolved["data"]["references"]
        assert len(references) == 2
        assert references[0]["specifier"] == "missing"
        assert references[0]["reason"] == "missing_target"
        unresolved_limited = assert_json(
            run([ochna, "unresolved", "--json", "--limit", "1"], tmp).stdout
        )
        assert len(unresolved_limited["data"]["references"]) == 1
        assert unresolved_limited["truncated"] is True

        # A dirty non-source file is a warning, not an index-health failure.
        (tmp / "README.md").write_text("review notes\n", encoding="utf-8")
        dirty_doctor = assert_json(run([ochna, "doctor", "--json"], tmp).stdout)
        assert dirty_doctor["ok"] is True
        assert dirty_doctor["data"]["freshness"] == "fresh"
        assert dirty_doctor["warnings"][0]["code"] == "dirty_worktree"
        (tmp / "README.md").unlink()

        # A newly added supported source is stale even before Git commits it.
        (tmp / "src" / "added.rs").write_text("pub fn added() {}\n", encoding="utf-8")
        added_source = run([ochna, "doctor", "--json"], tmp, check=False)
        assert added_source.returncode != 0
        added_source_json = assert_json(added_source.stdout)
        assert added_source_json["data"]["freshness"] == "stale"
        assert added_source_json["next_action"] == "ochna sync"
        (tmp / "src" / "added.rs").unlink()

        # Unsupported source formats require review without an impossible action.
        (tmp / "unsupported.py").write_text("def unsupported(): pass\n", encoding="utf-8")
        quality_degraded = run([ochna, "doctor", "--json"], tmp, check=False)
        assert quality_degraded.returncode != 0
        quality_json = assert_json(quality_degraded.stdout)
        assert quality_json["data"]["freshness"] == "fresh"
        assert quality_json["data"]["graph_quality"]["trust_verdict"] == "degraded"
        assert quality_json["next_action"] == "none"
        assert "Graph quality is degraded" in quality_degraded.stderr
        assert "Run 'none'" not in quality_degraded.stderr
        (tmp / "unsupported.py").unlink()

        # --- status --json preflight verdict: fresh index is ok ---
        status = assert_json(run([ochna, "status", "--json"], tmp).stdout)
        assert status["ok"] is True
        assert status["db_present"] is True
        assert status["schema"]["match"] is True
        assert status["counts"]["nodes"] >= 3
        assert status["freshness"] == "fresh"

        # --- search: finds the symbol; --no-tests drops test-path symbols ---
        search = assert_json(run([ochna, "--json", "search", "helper"], tmp).stdout)
        names = {n["name"] for n in search}
        assert "helper" in names
        assert "helper_in_tests" in names
        search_nt = assert_json(run([ochna, "--json", "--no-tests", "search", "helper"], tmp).stdout)
        nt_names = {n["name"] for n in search_nt}
        assert "helper" in nt_names
        assert "helper_in_tests" not in nt_names

        # --- tests-for: direct AST evidence, stable envelope, and explicit
        # --no-tests suppression (not a claim that no tests exist) ---
        tests_for = assert_json(run([ochna, "--json", "tests-for", "helper"], tmp).stdout)
        assert tests_for["contract_version"] == "0.3"
        assert tests_for["command"] == "tests-for"
        assert tests_for["ok"] is True
        assert tests_for["data"]["target"]["id"] == "src/lib.rs::helper"
        assert len(tests_for["data"]["tests"]) == 1
        direct = tests_for["data"]["tests"][0]
        assert direct["test"]["id"] == "tests/extra.rs::helper_in_tests"
        assert direct["evidence"] == "direct_call"
        assert direct["confidence"] == direct["path"][0]["confidence"]
        assert direct["path"][0]["relationship"] == "calls"
        assert direct["path"][0]["source"]["id"] == direct["test"]["id"]
        assert direct["path"][0]["target"]["id"] == "src/lib.rs::helper"
        tests_for_no_tests = assert_json(
            run([ochna, "--json", "--no-tests", "tests-for", "helper"], tmp).stdout
        )
        assert tests_for_no_tests["ok"] is True
        assert tests_for_no_tests["data"]["target"]["id"] == "src/lib.rs::helper"
        assert tests_for_no_tests["data"]["tests"] == []

        # --- callers: returns known callers w/ confidence; --min-confidence filters ---
        callers = assert_json(run([ochna, "--json", "callers", "helper"], tmp).stdout)
        caller_names = {n["name"] for n in callers}
        assert {"caller_one", "caller_two"} <= caller_names
        assert all(c["confidence"] is not None for c in callers)
        filtered = assert_json(
            run([ochna, "--json", "callers", "helper", "--min-confidence", "101"], tmp).stdout
        )
        assert len(filtered) < len(callers)
        res_text = run([ochna, "callers", "helper", "--show-resolution"], tmp).stdout
        assert "resolution:" in res_text and "confidence:" in res_text

        # --- callees: forward edges (what a symbol calls); --in scopes by path ---
        callees = assert_json(run([ochna, "--json", "callees", "caller_one"], tmp).stdout)
        assert "helper" in {n["name"] for n in callees}
        scoped = assert_json(
            run([ochna, "--json", "callees", "caller_one", "--in", "src"], tmp).stdout
        )
        assert "helper" in {n["name"] for n in scoped}
        unscoped_out = assert_json(
            run([ochna, "--json", "callees", "caller_one", "--in", "nonexistent"], tmp).stdout
        )
        assert unscoped_out == []

        # --- node: definition source, file symbol listing, and line slicing ---
        node_code = run([ochna, "node", "--symbol", "helper", "--include-code"], tmp).stdout
        assert "pub fn helper() {}" in node_code
        assert "Callers:" in node_code
        symbols = assert_json(
            run([ochna, "--json", "node", "--file", "src/lib.rs", "--symbols-only"], tmp).stdout
        )
        assert {"helper", "caller_one", "caller_two"} <= {s["name"] for s in symbols}
        sliced = assert_json(
            run([ochna, "--json", "node", "--file", "src/lib.rs", "--offset", "1", "--limit", "1"], tmp).stdout
        )
        assert sliced["lines"][0] == {"line": 1, "text": "pub fn helper() {}"}

        # --- explore: combined view includes callers ---
        explore = run([ochna, "explore", "helper"], tmp).stdout
        assert "caller_one" in explore and "Callers:" in explore

        # --- --workspace/-C resolves the db from a different cwd ---
        other = tmp / "elsewhere"
        other.mkdir()
        ws_search = assert_json(
            run([ochna, "--json", "-C", str(tmp), "search", "helper"], other).stdout
        )
        assert "helper" in {n["name"] for n in ws_search}

        # --- report.py analytics stays runnable against the live schema ---
        # (its hotspots query joins edges.target_nid; guards against the schema
        # drift that previously broke it silently). helper has two incoming calls.
        report = run([sys.executable, str(repo_root / "pyscripts" / "report.py")], tmp)
        assert "Hotspots" in report.stdout
        assert "helper" in report.stdout

        # --- status --json gates non-zero when the index goes stale, then sync clears it ---
        (tmp / "src" / "lib.rs").write_text("pub fn helper() {}\n", encoding="utf-8")
        run(["git", "add", "-A"], tmp)
        run(["git", "commit", "-m", "change source"], tmp)

        stale = run([ochna, "status", "--json"], tmp, check=False)
        assert stale.returncode != 0
        stale_json = assert_json(stale.stdout)
        assert stale_json["ok"] is False
        assert stale_json["freshness"] == "stale"
        assert stale_json["action"] == "ochna sync"

        run([ochna, "sync"], tmp)
        fresh = assert_json(run([ochna, "status", "--json"], tmp).stdout)
        assert fresh["ok"] is True
        assert fresh["freshness"] == "fresh"
        assert fresh["action"] == "none"

        print("verify_clis ok")
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
