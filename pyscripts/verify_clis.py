#!/usr/bin/env python3
"""Smoke-test agent-facing ochna CLI surfaces against a real binary."""

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


def assert_json(stdout: str) -> dict:
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

    tmp = Path(tempfile.mkdtemp(prefix="ochna-verify-clis."))
    try:
        run(["git", "init"], tmp)
        run(["git", "config", "user.email", "ochna@example.invalid"], tmp)
        run(["git", "config", "user.name", "Ochna Verify"], tmp)
        (tmp / "main.rs").write_text(
            "fn main() {\n    helper();\n}\n\nfn helper() {}\n",
            encoding="utf-8",
        )
        run(["git", "add", "main.rs"], tmp)
        run(["git", "commit", "-m", "baseline"], tmp)

        howto = run([str(bin_path), "howto"], tmp).stdout
        assert "ochna usage flow" in howto
        howto_json = assert_json(run([str(bin_path), "howto", "--json"], tmp).stdout)
        assert howto_json["flow"] == ["status", "search", "callers", "node"]
        assert "status" in howto_json["commands"]

        run([str(bin_path), "init"], tmp)
        pointer = tmp / ".ochna" / "AGENT.md"
        assert pointer.is_file()
        assert "ochna howto" in pointer.read_text(encoding="utf-8")

        status = assert_json(run([str(bin_path), "status", "--json"], tmp).stdout)
        assert status["ok"] is True
        assert status["db_present"] is True
        assert status["schema"]["match"] is True
        assert status["counts"]["nodes"] >= 2
        assert status["freshness"] == "fresh"

        (tmp / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
        run(["git", "add", "main.rs"], tmp)
        run(["git", "commit", "-m", "change source"], tmp)

        stale = run([str(bin_path), "status", "--json"], tmp, check=False)
        assert stale.returncode != 0
        stale_json = assert_json(stale.stdout)
        assert stale_json["ok"] is False
        assert stale_json["freshness"] == "stale"
        assert stale_json["action"] == "ochna sync"

        run([str(bin_path), "sync"], tmp)
        fresh = assert_json(run([str(bin_path), "status", "--json"], tmp).stdout)
        assert fresh["ok"] is True
        assert fresh["freshness"] == "fresh"
        assert fresh["action"] == "none"

        print("verify_clis ok")
        return 0
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
