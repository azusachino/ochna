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

        # --- Java framework relationships: add and incrementally index a
        # source after the baseline health checks, then exercise the installed CLI.
        (tmp / "src" / "App.java").write_text(
            "@RestController class ApiController {\n"
            "  private final Orders orders;\n"
            "  ApiController(Orders orders) { this.orders = orders; }\n"
            "  @GetMapping(\"/orders\") public String list() { return \"ok\"; }\n"
            "}\ninterface Orders {}\n"
            "@ConfigurationProperties(\"billing\") class BillingProperties {}\n"
            "@FeignClient(name = \"catalog\") interface CatalogClient {\n"
            "  @GetMapping(\"/products\") Product getProduct();\n"
            "}\nclass Product {}\n"
            "@Component class RpcClient {\n"
            "  MissingGrpc.MissingBlockingStub stub;\n"
            "  void call() { stub.get(new Request()); }\n"
            "}\nclass Request {}\n",
            encoding="utf-8",
        )
        run([ochna, "sync"], tmp)
        route_impact = assert_json(run([ochna, "--json", "impact", "src/App.java::ApiController::list::route::GET /orders"], tmp).stdout)
        assert any(edge["relationship"] == "route_handler" and edge["resolution_kind"] == "framework_annotation" and edge["confidence"] == 85 for edge in route_impact["data"]["edges"])
        injection_impact = assert_json(run([ochna, "--json", "impact", "src/App.java::ApiController"], tmp).stdout)
        assert any(edge["relationship"] == "injected_into" and edge["resolution_kind"] == "framework_convention" for edge in injection_impact["data"]["edges"])
        feign_impact = assert_json(run([ochna, "--json", "impact", "src/App.java::CatalogClient::getProduct"], tmp).stdout)
        assert any(edge["relationship"] == "feign_calls" and edge["resolution_kind"] == "framework_annotation" for edge in feign_impact["data"]["edges"])
        grpc_boundary = assert_json(run([
            ochna, "--json", "impact", "src/App.java::RpcClient::call"
        ], tmp).stdout)
        assert any(
            boundary["relationship"] == "grpc_calls"
            and boundary["target"] is None
            and boundary["reason"] == "missing_target"
            for boundary in grpc_boundary["data"]["unresolved_boundaries"]
        )
        assert any(
            warning["code"] == "unresolved_framework_endpoint"
            for warning in grpc_boundary["warnings"]
        )

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

        # --- diff: materialized v0.3 before/after review story. Git ranges
        # build a bounded temporary base index, so removed identities and
        # relationships are evidence rather than placeholders.
        review = Path(tempfile.mkdtemp(prefix="ochna-review-v0.3."))
        try:
            fixture = repo_root / "fixtures" / "review-v0.3"
            shutil.copytree(fixture / "before", review, dirs_exist_ok=True)
            run(["git", "init"], review)
            run(["git", "config", "user.email", "ochna@example.invalid"], review)
            run(["git", "config", "user.name", "Ochna Verify"], review)
            run(["git", "add", "-A"], review)
            run(["git", "commit", "-m", "baseline"], review)
            shutil.copytree(fixture / "after", review, dirs_exist_ok=True)
            run([ochna, "init"], review)
            diff = assert_json(run([ochna, "diff", "--base", "HEAD", "--json"], review).stdout)
            assert diff["command"] == "diff"
            changed = {row["symbol"]["id"] for row in diff["data"]["symbols"] if row["symbol"]}
            assert {"src/lib.rs::render", "src/lib.rs::render_page"} <= changed
            assert any(
                row["symbol"]["id"] == "src/lib.rs::legacy_render" and row["change"] == "removed"
                for row in diff["data"]["symbols"]
            )
            assert diff["warnings"] == []
            assert diff["data"]["historical_snapshot"]["base_index"] == "temporary"
            assert diff["data"]["historical_snapshot"]["temporary_bytes"] > 0
            assert diff["data"]["historical_snapshot"]["elapsed_ms"] >= 0
            assert diff["data"]["historical_snapshot"]["cleanup"] == "removed"
            assert diff["data"]["newly_unresolved_callers"] == [{
                "source": "src/lib.rs::render_page", "specifier": "missing_renderer", "reason": "missing_target"
            }]
            assert {row["path"] for row in diff["data"]["unmapped_hunks"]} == {"README.md"}
            # --- impact: confidence-bounded reverse traversal preserves both
            # structural paths and stops at the unresolved frontier. ---
            impact = assert_json(run([
                ochna, "impact", "render", "--direction", "callers", "--depth", "2", "--json"
            ], review).stdout)
            assert impact["contract_version"] == "0.3"
            assert impact["command"] == "impact"
            assert impact["data"]["root"]["id"] == "src/lib.rs::render"
            assert impact["data"]["direction"] == "callers"
            assert impact["data"]["min_confidence"] == 30
            assert {node["id"] for node in impact["data"]["nodes"]} >= {
                "src/lib.rs::render_page", "tests/render_tests.rs::render_page_uses_render"
            }
            assert len(impact["data"]["paths"]) >= 2
            assert len(impact["data"]["affected_tests"]) == 1
            assert impact["data"]["affected_tests"][0]["test"]["id"] == "tests/render_tests.rs::render_page_uses_render"
            assert any(
                row["source"]["id"] == "src/lib.rs::render_page"
                and row["specifier"] == "missing_renderer"
                and row["reason"] == "missing_target"
                for row in impact["data"]["unresolved_boundaries"]
            )
            assert any(edge["confidence"] < 80 for edge in impact["data"]["edges"])
            no_tests_impact = assert_json(run([
                ochna, "--no-tests", "impact", "render", "--direction", "callers", "--depth", "2", "--json"
            ], review).stdout)
            assert no_tests_impact["data"]["affected_tests"] == []
            explicit = assert_json(run([ochna, "diff", "--files", "src/lib.rs", "tests/render_tests.rs", "--json"], review).stdout)
            assert explicit["data"]["base"] is None and explicit["data"]["head"] is None
            assert {row["path"] for row in explicit["data"]["files"]} == {"src/lib.rs", "tests/render_tests.rs"}
        finally:
            shutil.rmtree(review, ignore_errors=True)

        # Hunk ranges may span several current symbols. Only unresolved calls
        # on newly-added patch lines are called "newly" without task 6's
        # historical graph; an unchanged unresolved call is not relabelled.
        regression = Path(tempfile.mkdtemp(prefix="ochna-diff-regression."))
        try:
            (regression / "src").mkdir()
            (regression / "src" / "lib.rs").write_text(
                "fn unchanged() { existing_missing(); }\n"
                "fn first() { }\n"
                "fn second() { }\n", encoding="utf-8"
            )
            run(["git", "init"], regression)
            run(["git", "config", "user.email", "ochna@example.invalid"], regression)
            run(["git", "config", "user.name", "Ochna Verify"], regression)
            run(["git", "add", "-A"], regression)
            run(["git", "commit", "-m", "baseline"], regression)
            (regression / "src" / "lib.rs").write_text(
                "fn unchanged() { existing_missing(); }\n"
                "fn first() { newly_missing(); }\n"
                "fn second() { 1 + 1; }\n", encoding="utf-8"
            )
            run([ochna, "init"], regression)
            regression_diff = assert_json(run([ochna, "diff", "--base", "HEAD", "--json"], regression).stdout)
            regression_symbols = {row["symbol"]["id"] for row in regression_diff["data"]["symbols"] if row["symbol"]}
            assert {"src/lib.rs::first", "src/lib.rs::second"} <= regression_symbols
            assert regression_diff["data"]["newly_unresolved_callers"] == [{
                "source": "src/lib.rs::first", "specifier": "newly_missing", "reason": "missing_target"
            }]
        finally:
            shutil.rmtree(regression, ignore_errors=True)

        # Renames have no content hunk when Git detects a 100% move, while a
        # pure deletion has only a zero-count hunk. Historical indexing keeps
        # their actual pre-change identities available.
        lifecycle = Path(tempfile.mkdtemp(prefix="ochna-diff-lifecycle."))
        try:
            (lifecycle / "src").mkdir()
            (lifecycle / "src" / "original.rs").write_text("fn kept() {}\n", encoding="utf-8")
            (lifecycle / "src" / "deleted.rs").write_text("fn gone() {}\n", encoding="utf-8")
            run(["git", "init"], lifecycle)
            run(["git", "config", "user.email", "ochna@example.invalid"], lifecycle)
            run(["git", "config", "user.name", "Ochna Verify"], lifecycle)
            run(["git", "add", "-A"], lifecycle)
            run(["git", "commit", "-m", "baseline"], lifecycle)
            run(["git", "mv", "src/original.rs", "src/renamed.rs"], lifecycle)
            (lifecycle / "src" / "deleted.rs").unlink()
            run([ochna, "init"], lifecycle)
            lifecycle_diff = assert_json(run([ochna, "diff", "--base", "HEAD", "--json"], lifecycle).stdout)
            assert {row["status"] for row in lifecycle_diff["data"]["files"]} == {"renamed", "deleted"}
            assert "src/renamed.rs::kept" in {row["symbol"]["id"] for row in lifecycle_diff["data"]["symbols"] if row["symbol"]}
            assert any(
                row["symbol"]["id"] == "src/deleted.rs::gone" and row["change"] == "removed"
                for row in lifecycle_diff["data"]["symbols"]
            )
            missing = assert_json(run([ochna, "diff", "--files", "missing.rs", "--json"], lifecycle).stdout)
            assert missing["data"]["files"] == [{"path": "missing.rs", "status": "deleted"}]
            assert missing["data"]["symbols"][0]["symbol"] is None
            assert missing["warnings"][0]["code"] == "historical_index_required"
            too_many = run(
                [ochna, "diff", "--files", *[f"missing-{i}.rs" for i in range(501)]],
                lifecycle,
                check=False,
            )
            assert too_many.returncode != 0
            assert "at most 500" in too_many.stderr
        finally:
            shutil.rmtree(lifecycle, ignore_errors=True)

        # Deletion-heavy C history mirrors the Linux use case: the base has a
        # small obsolete API surface and its callers; the reviewed tree removes
        # it wholesale. The temporary archive/index must leave no directory
        # behind after returning JSON.
        linux_like = Path(tempfile.mkdtemp(prefix="ochna-diff-linux-like."))
        temporary_before = set(Path(tempfile.gettempdir()).glob("ochna-historical-*"))
        try:
            (linux_like / "kernel").mkdir()
            (linux_like / "kernel" / "legacy.c").write_text(
                "static int strncpy_legacy(char *dst) { return dst[0]; }\n"
                "int copy_user(void) { return strncpy_legacy(0); }\n"
                "int copy_name(void) { return strncpy_legacy(0); }\n",
                encoding="utf-8",
            )
            run(["git", "init"], linux_like)
            run(["git", "config", "user.email", "ochna@example.invalid"], linux_like)
            run(["git", "config", "user.name", "Ochna Verify"], linux_like)
            run(["git", "add", "-A"], linux_like)
            run(["git", "commit", "-m", "legacy copy helpers"], linux_like)
            (linux_like / "kernel" / "legacy.c").unlink()
            (linux_like / "kernel" / "safe.c").write_text(
                "int copy_user(void) { return 0; }\n", encoding="utf-8"
            )
            run([ochna, "init"], linux_like)
            linux_diff = assert_json(run([ochna, "diff", "--base", "HEAD", "--json"], linux_like).stdout)
            removed = {
                row["symbol"]["id"]
                for row in linux_diff["data"]["symbols"]
                if row["change"] == "removed"
            }
            assert {"kernel/legacy.c::strncpy_legacy", "kernel/legacy.c::copy_user", "kernel/legacy.c::copy_name"} <= removed
            assert len([row for row in linux_diff["data"]["edges"] if row["change"] == "removed"]) >= 2
            assert linux_diff["data"]["historical_snapshot"]["temporary_bytes"] > 0
            assert linux_diff["data"]["historical_snapshot"]["cleanup"] == "removed"
            assert set(Path(tempfile.gettempdir()).glob("ochna-historical-*")) == temporary_before
        finally:
            shutil.rmtree(linux_like, ignore_errors=True)

        # A named --head is a revision selector, not an alias for the dirty
        # workspace. Its temporary graph must win even when the current index
        # contains a later, unrelated symbol.
        named_head = Path(tempfile.mkdtemp(prefix="ochna-diff-named-head."))
        try:
            (named_head / "src").mkdir()
            (named_head / "src" / "lib.rs").write_text("fn base() {}\n", encoding="utf-8")
            run(["git", "init"], named_head)
            run(["git", "config", "user.email", "ochna@example.invalid"], named_head)
            run(["git", "config", "user.name", "Ochna Verify"], named_head)
            run(["git", "add", "-A"], named_head)
            run(["git", "commit", "-m", "base"], named_head)
            base_sha = run(["git", "rev-parse", "HEAD"], named_head).stdout.strip()
            (named_head / "src" / "lib.rs").write_text(
                "fn head_only() { missing_one(); missing_two(); }\n",
                encoding="utf-8",
            )
            run(["git", "add", "-A"], named_head)
            run(["git", "commit", "-m", "head"], named_head)
            head_sha = run(["git", "rev-parse", "HEAD"], named_head).stdout.strip()
            (named_head / "src" / "lib.rs").write_text("fn dirty_workspace_only() {}\n", encoding="utf-8")
            run([ochna, "init"], named_head)
            named_diff = assert_json(run([
                ochna, "diff", "--base", base_sha, "--head", head_sha, "--json"
            ], named_head).stdout)
            named_symbols = {row["symbol"]["id"] for row in named_diff["data"]["symbols"] if row["symbol"]}
            assert "src/lib.rs::head_only" in named_symbols
            assert "src/lib.rs::dirty_workspace_only" not in named_symbols
            assert named_diff["data"]["historical_snapshot"]["head_index"] == "temporary"
            assert named_diff["data"]["historical_snapshot"]["base"] is not None
            assert named_diff["data"]["historical_snapshot"]["head"] is not None
            assert named_diff["data"]["historical_snapshot"]["temporary_bytes"] == (
                named_diff["data"]["historical_snapshot"]["base"]["temporary_bytes"]
                + named_diff["data"]["historical_snapshot"]["head"]["temporary_bytes"]
            )
            limited_named_diff = assert_json(run([
                ochna, "diff", "--base", base_sha, "--head", head_sha,
                "--limit", "1", "--json"
            ], named_head).stdout)
            assert len(limited_named_diff["data"]["newly_unresolved_callers"]) == 1
            assert limited_named_diff["truncated"] is True
            missing_head = run([
                ochna, "diff", "--base", base_sha, "--head", "definitely-missing", "--json"
            ], named_head, check=False)
            assert missing_head.returncode != 0
            missing_head_json = assert_json(missing_head.stdout)
            assert missing_head_json["ok"] is False
            assert missing_head_json["warnings"][0]["code"] == "head_revision_unavailable"
            assert missing_head_json["data"]["symbols"] == []
            (named_head / "src" / "lib.rs").write_text("fn stale() {}\n", encoding="utf-8")
            stale_diff = run([ochna, "diff", "--base", head_sha, "--json"], named_head, check=False)
            assert stale_diff.returncode != 0
            stale_diff_json = assert_json(stale_diff.stdout)
            assert stale_diff_json["ok"] is False
            assert stale_diff_json["warnings"][0]["code"] == "current_index_stale"
            assert stale_diff_json["next_action"] == "ochna sync"
            assert stale_diff_json["data"]["symbols"] == []
        finally:
            shutil.rmtree(named_head, ignore_errors=True)

        print("verify_clis ok")
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
