#!/usr/bin/env python3
"""
ochna Python Companion Tool (Requires Python 3.14+)
Directly queries the ochna SQLite relational index to extract advanced codebase statistics,
including Call-Graph PageRank Centrality (CS224W) and Tarjan Strongly Connected Components (CS 6120).
"""

from __future__ import annotations

from collections import defaultdict
import os
import sqlite3
import sys


def compute_callgraph_pagerank(
    node_ids: dict[int, str],
    call_edges: list[tuple[int, int]],
    damping: float = 0.85,
    iterations: int = 30,
) -> list[tuple[str, float]]:
    """Compute PageRank centrality over the directed call graph (caller -> callee)."""
    if not node_ids:
        return []

    n = len(node_ids)
    nids = list(node_ids.keys())
    out_targets: dict[int, list[int]] = defaultdict(list)
    for src, dst in call_edges:
        if src in node_ids and dst in node_ids:
            out_targets[src].append(dst)

    scores: dict[int, float] = {nid: 1.0 / n for nid in nids}
    base_teleport = (1.0 - damping) / n

    for _ in range(iterations):
        dangling_mass = sum(scores[nid] for nid in nids if not out_targets.get(nid))
        dangling_share = damping * dangling_mass / n
        next_scores: dict[int, float] = {nid: base_teleport + dangling_share for nid in nids}
        for src, targets in out_targets.items():
            if not targets:
                continue
            share = damping * scores[src] / len(targets)
            for dst in targets:
                next_scores[dst] += share
        scores = next_scores

    ranked = sorted(
        ((node_ids[nid], score) for nid, score in scores.items()),
        key=lambda item: (-item[1], item[0]),
    )
    return ranked


def find_recursive_sccs(
    node_ids: dict[int, str],
    call_edges: list[tuple[int, int]],
) -> list[list[str]]:
    """Find self-recursive and mutually recursive call cycles via Tarjan's O(V + E) SCC algorithm."""
    adj: dict[int, list[int]] = defaultdict(list)
    self_loops: set[int] = set()
    for src, dst in call_edges:
        if src in node_ids and dst in node_ids:
            adj[src].append(dst)
            if src == dst:
                self_loops.add(src)

    index_counter = 0
    indices: dict[int, int] = {}
    lowlink: dict[int, int] = {}
    stack: list[int] = []
    on_stack: set[int] = set()
    cycles: list[list[str]] = []

    def strongconnect(v: int) -> None:
        nonlocal index_counter
        indices[v] = index_counter
        lowlink[v] = index_counter
        index_counter += 1
        stack.append(v)
        on_stack.add(v)

        for w in adj.get(v, ()):
            if w not in indices:
                strongconnect(w)
                lowlink[v] = min(lowlink[v], lowlink[w])
            elif w in on_stack:
                lowlink[v] = min(lowlink[v], indices[w])

        if lowlink[v] == indices[v]:
            scc: list[int] = []
            while True:
                w = stack.pop()
                on_stack.remove(w)
                scc.append(w)
                if w == v:
                    break
            if len(scc) > 1 or (len(scc) == 1 and scc[0] in self_loops):
                cycles.append(sorted(node_ids[nid] for nid in scc))

    for nid in sorted(node_ids.keys()):
        if nid not in indices:
            strongconnect(nid)

    cycles.sort(key=lambda c: (-len(c), c[0]))
    return cycles


def main() -> None:
    db_path = os.path.join(".ochna", "ochna.db")
    if not os.path.exists(db_path):
        print(f"Error: ochna database not found at {db_path}", file=sys.stderr)
        print("Please run 'ochna init' to index the workspace first.", file=sys.stderr)
        sys.exit(1)

    print("==================================================")
    print("           ochna Codebase Analysis Report         ")
    print("==================================================")

    try:
        conn = sqlite3.connect(db_path)
        cursor = conn.cursor()

        # 1. Project Baseline Info
        print("\n--- Project Baseline Info ---")
        cursor.execute("SELECT key, value FROM project_metadata")
        metadata = dict(cursor.fetchall())
        if metadata:
            for k, v in metadata.items():
                name = k.replace("_", " ").title()
                print(f"  {name:<15}: {v}")
        else:
            print("  No project metadata found.")

        # 2. File Statistics
        print("\n--- File Statistics ---")
        cursor.execute("""
            SELECT language, COUNT(*), SUM(size_bytes)
            FROM files
            GROUP BY language
        """)
        lang_stats = cursor.fetchall()
        print(f"  {'Language':<12} | {'File Count':<10} | {'Total Size (Bytes)':<18}")
        print("  " + "-" * 48)
        total_files = 0
        total_size = 0
        for lang, count, size in lang_stats:
            lang_str = lang if lang else "unknown"
            size_val = size if size else 0
            total_files += count
            total_size += size_val
            print(f"  {lang_str:<12} | {count:<10} | {size_val:<18,}")
        print("  " + "-" * 48)
        print(f"  {'Total':<12} | {total_files:<10} | {total_size:<18,}")

        # 3. Top 5 Files by Symbol Density
        print("\n--- Top 5 Files by Symbol Count ---")
        cursor.execute("""
            SELECT f.file_path, f.language, COUNT(n.id) as symbol_count
            FROM files f
            LEFT JOIN nodes n ON f.file_path = n.file_path
            GROUP BY f.file_path
            ORDER BY symbol_count DESC
            LIMIT 5
        """)
        top_files = cursor.fetchall()
        print(f"  {'File Path':<50} | {'Language':<10} | {'Symbols':<8}")
        print("  " + "-" * 74)
        for path, lang, syms in top_files:
            short_path = path if len(path) <= 50 else f"...{path[-47:]}"
            print(f"  {short_path:<50} | {lang:<10} | {syms:<8}")

        # 4. Symbol Count by Kind
        print("\n--- Symbol Breakdown by Kind ---")
        cursor.execute("""
            SELECT kind, COUNT(*)
            FROM nodes
            GROUP BY kind
            ORDER BY COUNT(*) DESC
        """)
        kinds = cursor.fetchall()
        for kind, count in kinds:
            print(f"  {kind.title():<15}: {count}")

        # 5. Top 5 Most Called Functions/Methods (In-degree)
        print("\n--- Top 5 Most Referenced Symbols (Hotspots) ---")
        cursor.execute("""
            SELECT n.id, COUNT(*) as incoming_calls
            FROM edges e
            JOIN nodes n ON e.target_nid = n.nid
            WHERE e.kind = 'calls'
            GROUP BY e.target_nid
            ORDER BY incoming_calls DESC
            LIMIT 5
        """)
        hotspots = cursor.fetchall()
        if hotspots:
            print(f"  {'Symbol ID (Target)':<55} | {'Calls':<8}")
            print("  " + "-" * 68)
            for target, count in hotspots:
                short_target = target if len(target) <= 55 else f"...{target[-52:]}"
                print(f"  {short_target:<55} | {count:<8}")
        else:
            print("  No call relationships found.")

        # 6. Call-Graph PageRank Centrality & Tarjan SCC Cycles
        cursor.execute("SELECT nid, id FROM nodes WHERE is_test = 0")
        prod_nodes = {int(nid): str(sym_id) for nid, sym_id in cursor.fetchall()}
        cursor.execute("SELECT source_nid, target_nid FROM edges WHERE kind = 'calls'")
        call_edges = [(int(s), int(t)) for s, t in cursor.fetchall()]

        print("\n--- Top 5 Symbols by PageRank Centrality ---")
        pagerank_top = compute_callgraph_pagerank(prod_nodes, call_edges)[:5]
        if pagerank_top:
            print(f"  {'Symbol ID':<55} | {'PageRank':<10}")
            print("  " + "-" * 68)
            for sym_id, score in pagerank_top:
                short_sym = sym_id if len(sym_id) <= 55 else f"...{sym_id[-52:]}"
                print(f"  {short_sym:<55} | {score:<10.4f}")
        else:
            print("  No production symbols indexed.")

        print("\n--- Call-Graph Cycles (Tarjan SCC) ---")
        cycles = find_recursive_sccs(prod_nodes, call_edges)
        if cycles:
            for idx, comp in enumerate(cycles[:5], start=1):
                print(f"  Cycle {idx} ({len(comp)} symbol(s)): {' <-> '.join(comp)}")
        else:
            print("  0 recursive call cycles detected (DAG call graph).")

        conn.close()

    except sqlite3.Error as e:
        print(f"Database error: {e}", file=sys.stderr)
        sys.exit(1)

    print("\n==================================================")


if __name__ == "__main__":
    main()
