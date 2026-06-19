#!/usr/bin/env python3
"""
ochna Python Companion Tool (Requires Python 3.14+)
Directly queries the ochna SQLite relational index to extract advanced codebase statistics.
"""

import os
import sqlite3
import sys

def main():
    db_path = os.path.join(".codegraph", "codegraph.db")
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
                # Clean up formatting
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
            # Truncate path if too long
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
            SELECT target_id, COUNT(*) as incoming_calls
            FROM edges
            WHERE kind = 'calls'
            GROUP BY target_id
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

        conn.close()

    except sqlite3.Error as e:
        print(f"Database error: {e}", file=sys.stderr)
        sys.exit(1)

    print("\n==================================================")

if __name__ == "__main__":
    main()
