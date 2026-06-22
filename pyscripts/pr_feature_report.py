#!/usr/bin/env python3
"""
Summarize a GitHub PR against an ochna-indexed checkout.

The script intentionally combines two data sources:
- GitHub PR metadata/files via `gh api`, because benchmark submodules are often
  shallow and may not have enough local parent history for `git diff`.
- The local `.ochna/ochna.db`, because changed-file symbols and line spans are
  already indexed and cheap to query.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sqlite3
import subprocess
import sys
from pathlib import Path
from typing import Any


def run_json(args: list[str], cwd: Path) -> Any:
    completed = subprocess.run(
        args,
        cwd=cwd,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    return json.loads(completed.stdout)


def run_text(args: list[str], cwd: Path) -> str:
    completed = subprocess.run(
        args,
        cwd=cwd,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    return completed.stdout.strip()


def gh_api(path: str, cwd: Path) -> Any:
    return run_json(["gh", "api", path], cwd)


def detect_shallow_head(workspace: Path) -> bool:
    try:
        parents = run_text(["git", "rev-list", "--parents", "-n", "1", "HEAD"], workspace)
    except subprocess.CalledProcessError:
        return False
    return len(parents.split()) == 1


def connect_db(workspace: Path) -> sqlite3.Connection:
    db_path = workspace / ".ochna" / "ochna.db"
    if not db_path.exists():
        raise SystemExit(f"ochna database not found at {db_path}; run `ochna init` first")
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row
    return conn


def db_counts(conn: sqlite3.Connection) -> dict[str, int]:
    counts = {}
    for table in ["files", "nodes", "edges"]:
        counts[table] = conn.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
    return counts


def symbols_for_file(conn: sqlite3.Connection, path: str) -> list[sqlite3.Row]:
    return conn.execute(
        """
        SELECT name, kind, qualified_name, start_line, end_line, signature, is_test
        FROM nodes
        WHERE file_path = ?
        ORDER BY start_line, name
        """,
        (path,),
    ).fetchall()


def symbols_named(conn: sqlite3.Connection, names: set[str]) -> dict[str, list[sqlite3.Row]]:
    if not names:
        return {}
    placeholders = ",".join("?" for _ in names)
    rows = conn.execute(
        f"""
        SELECT name, kind, qualified_name, file_path, start_line, end_line, signature, is_test
        FROM nodes
        WHERE name IN ({placeholders})
        ORDER BY name, is_test, file_path, start_line
        """,
        tuple(sorted(names)),
    ).fetchall()
    grouped: dict[str, list[sqlite3.Row]] = {}
    for row in rows:
        grouped.setdefault(row["name"], []).append(row)
    return grouped


def extract_added_test_symbols(files: list[dict[str, Any]]) -> set[str]:
    names: set[str] = set()
    for file in files:
        patch = file.get("patch") or ""
        for line in patch.splitlines():
            match = re.match(r"^\+\s*func\s+(Test[A-Za-z0-9_]+)\s*\(", line)
            if match:
                names.add(match.group(1))
    return names


def short_sha(sha: str | None) -> str:
    return sha[:12] if sha else "unknown"


def render_row(row: sqlite3.Row) -> str:
    signature = row["signature"] or row["qualified_name"] or row["name"]
    test_suffix = " test" if row["is_test"] else ""
    return (
        f"- `{signature}` ({row['kind']}{test_suffix}) "
        f"`{row['file_path']}:{row['start_line']}`"
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Summarize a GitHub PR using gh metadata and an ochna index."
    )
    parser.add_argument("--workspace", default=".", help="Path to the indexed checkout")
    parser.add_argument("--repo", required=True, help="GitHub repo, e.g. kubernetes/kubernetes")
    parser.add_argument("--pr", required=True, type=int, help="Pull request number")
    parser.add_argument("--max-file-symbols", type=int, default=12)
    args = parser.parse_args()

    workspace = Path(args.workspace).resolve()
    pr_path = f"repos/{args.repo}/pulls/{args.pr}"
    files_path = f"{pr_path}/files"

    try:
        pr = gh_api(pr_path, workspace)
        files = gh_api(files_path, workspace)
    except (subprocess.CalledProcessError, json.JSONDecodeError) as exc:
        raise SystemExit(f"failed to read PR data with gh api: {exc}") from exc

    conn = connect_db(workspace)
    counts = db_counts(conn)
    added_tests = extract_added_test_symbols(files)
    named = symbols_named(conn, added_tests)

    print(f"# PR {args.repo}#{args.pr}: {pr['title']}")
    print()
    print(f"- State: `{pr['state']}`, merged at `{pr.get('merged_at')}`")
    print(f"- Merge commit: `{short_sha(pr.get('merge_commit_sha'))}`")
    print(f"- Head commit: `{short_sha(pr.get('head', {}).get('sha'))}`")
    print(f"- Labels: {', '.join(label['name'] for label in pr.get('labels', []))}")
    print(f"- Release note: `{extract_release_note(pr.get('body') or '')}`")
    print(f"- Local checkout has shallow HEAD: `{str(detect_shallow_head(workspace)).lower()}`")
    print(f"- ochna index: `{counts['files']}` files, `{counts['nodes']}` nodes, `{counts['edges']}` edges")
    print()
    print("## Changed Files")
    for file in files:
        print(
            f"- `{file['filename']}` {file['status']} "
            f"(+{file['additions']}/-{file['deletions']}, {file['changes']} changes)"
        )
    print()
    print("## Changed-File Symbols")
    for file in files:
        rows = symbols_for_file(conn, file["filename"])
        print(f"### `{file['filename']}`")
        if not rows:
            print("- No indexed symbols found for this file.")
            continue
        for row in rows[: args.max_file_symbols]:
            print(f"- `{row['signature'] or row['name']}` lines {row['start_line']}-{row['end_line']}")
        if len(rows) > args.max_file_symbols:
            print(f"- ... {len(rows) - args.max_file_symbols} more symbols")
    print()
    print("## Added Test Symbols")
    if not added_tests:
        print("- No added `Test*` functions detected in PR patch.")
    for name in sorted(added_tests):
        rows = named.get(name, [])
        if not rows:
            print(f"- `{name}` not present in the local ochna index")
            continue
        for row in rows:
            print(render_row(row))


def extract_release_note(body: str) -> str:
    match = re.search(r"```release-note\s*(.*?)```", body, flags=re.DOTALL)
    if not match:
        return "unknown"
    return " ".join(match.group(1).strip().split()) or "empty"


if __name__ == "__main__":
    main()
