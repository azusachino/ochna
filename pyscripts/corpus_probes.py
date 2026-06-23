#!/usr/bin/env python3
import sqlite3
from pathlib import Path

def check_probe(name, db_path, query_fn):
    print(f"=== Probe: {name} ===")
    path = Path(db_path)
    if not path.exists():
        print(f"Database not found at {path}. Skipping.\n")
        return
    conn = sqlite3.connect(path)
    conn.row_factory = sqlite3.Row
    try:
        query_fn(conn)
    except Exception as e:
        print(f"Error querying {name}: {e}")
    finally:
        conn.close()
    print()

def probe_kubernetes(conn):
    # Count total edges and nodes
    nodes_count = conn.execute("SELECT COUNT(*) FROM nodes").fetchone()[0]
    edges_count = conn.execute("SELECT COUNT(*) FROM edges").fetchone()[0]
    print(f"Nodes: {nodes_count}, Edges: {edges_count}")

    # Count how many targets GetList calls resolve to
    print("Go GetList Resolution Noise:")
    rows = conn.execute("""
        SELECT COUNT(DISTINCT e.target_nid) as unique_targets, COUNT(*) as total_edges
        FROM edges e
        JOIN nodes n ON e.target_nid = n.nid
        WHERE n.name = 'GetList'
    """).fetchone()
    print(f"  Edges targeting a 'GetList' symbol: {rows[1]}")
    print(f"  Unique definitions of 'GetList' resolved to: {rows[0]}")

    # Find the specific target for delegation test calls to GetList
    print("  Callers of GetList in cacher_whitebox_test.go:")
    callers = conn.execute("""
        SELECT src.id as caller_id, tgt.id as target_id
        FROM edges e
        JOIN nodes src ON e.source_nid = src.nid
        JOIN nodes tgt ON e.target_nid = tgt.nid
        WHERE src.file_path LIKE '%cacher_whitebox_test.go' AND tgt.name = 'GetList'
        LIMIT 5
    """).fetchall()
    for caller in callers:
        print(f"    {caller['caller_id']} -> {caller['target_id']}")

def probe_netty(conn):
    nodes_count = conn.execute("SELECT COUNT(*) FROM nodes").fetchone()[0]
    edges_count = conn.execute("SELECT COUNT(*) FROM edges").fetchone()[0]
    print(f"Nodes: {nodes_count}, Edges: {edges_count}")

    # Show releaseAndFailQueuedWrite callers
    print("releaseAndFailQueuedWrite callers (PR 16959):")
    callers = conn.execute("""
        SELECT src.id as caller_id, tgt.id as target_id
        FROM edges e
        JOIN nodes src ON e.source_nid = src.nid
        JOIN nodes tgt ON e.target_nid = tgt.nid
        WHERE tgt.name = 'releaseAndFailQueuedWrite'
    """).fetchall()
    for caller in callers:
        print(f"    {caller['caller_id']} -> {caller['target_id']}")

    # Common method 'release' noise
    print("Java 'release' method resolution count:")
    rows = conn.execute("""
        SELECT COUNT(*) FROM edges e
        JOIN nodes tgt ON e.target_nid = tgt.nid
        WHERE tgt.name = 'release'
    """).fetchone()
    print(f"  Total edges targeting a 'release' method: {rows[0]}")

def probe_linux(conn):
    nodes_count = conn.execute("SELECT COUNT(*) FROM nodes").fetchone()[0]
    edges_count = conn.execute("SELECT COUNT(*) FROM edges").fetchone()[0]
    print(f"Nodes: {nodes_count}, Edges: {edges_count}")

    # strncpy callers check
    print("strncpy callers:")
    rows = conn.execute("""
        SELECT COUNT(*) FROM edges e
        JOIN nodes tgt ON e.target_nid = tgt.nid
        WHERE tgt.name = 'strncpy'
    """).fetchone()
    print(f"  Edges targeting 'strncpy' (core API): {rows[0]}")

def main():
    repo_root = Path(__file__).resolve().parent.parent
    check_probe("Go/Kubernetes", repo_root / "clones/kubernetes/.ochna/ochna.db", probe_kubernetes)
    check_probe("Java/Netty", repo_root / "clones/netty/.ochna/ochna.db", probe_netty)
    check_probe("C/Linux", repo_root / "clones/linux/.ochna/ochna.db", probe_linux)

if __name__ == "__main__":
    main()
