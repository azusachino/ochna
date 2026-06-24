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

        # Fixture: `helper` with two in-file callers (resolved call edges) plus a
        # test-path symbol so --no-tests has something to drop.
        (tmp / "src").mkdir()
        (tmp / "tests").mkdir()
        (tmp / "src" / "lib.rs").write_text(
            "pub fn helper() {}\n\n"
            "pub fn caller_one() {\n    helper();\n}\n\n"
            "pub fn caller_two() {\n    helper();\n}\n",
            encoding="utf-8",
        )
        (tmp / "tests" / "extra.rs").write_text(
            "pub fn helper_in_tests() {}\n", encoding="utf-8"
        )
        run(["git", "add", "-A"], tmp)
        run(["git", "commit", "-m", "baseline"], tmp)

        # --- howto: self-describing surface (human + json descriptor) ---
        howto = run([ochna, "howto"], tmp).stdout
        assert "ochna usage flow" in howto
        howto_json = assert_json(run([ochna, "howto", "--json"], tmp).stdout)
        assert howto_json["flow"] == ["status", "search", "callers", "node", "explore"]
        assert "status" in howto_json["commands"]
        assert howto_json["flags"]["min_confidence"]
        assert howto_json["flags"]["show_resolution"]
        assert howto_json["flags"]["include_library"]
        assert any("symbols-only" in mode for mode in howto_json["node_modes"])
        assert howto_json["confidence_cascade"][0] == "exact=100"

        # --- init writes the index and the AGENT.md pointer ---
        run([ochna, "init"], tmp)
        pointer = tmp / ".ochna" / "AGENT.md"
        assert pointer.is_file()
        assert "ochna howto" in pointer.read_text(encoding="utf-8")

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
