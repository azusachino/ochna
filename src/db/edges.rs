//! Edge writes and source-scoped deletion.

use super::Edge;
use rusqlite::Connection;

/// Upsert an edge into the database (INSERT OR REPLACE)
pub fn upsert_edge(conn: &Connection, edge: &Edge) -> rusqlite::Result<()> {
    // Translate the string endpoints to their interned `nid` in SQL so callers
    // keep working with "file::symbol" ids. If either endpoint is missing the
    // SELECT yields no row and nothing is inserted (the edge has no valid node).
    let mut stmt = conn.prepare_cached(
        "INSERT OR REPLACE INTO edges (source_nid, target_nid, kind)
         SELECT s.nid, t.nid, ?3 FROM nodes s, nodes t WHERE s.id = ?1 AND t.id = ?2",
    )?;
    stmt.execute((&edge.source_id, &edge.target_id, &edge.kind))?;
    Ok(())
}

pub fn delete_edges_for_source_id(conn: &Connection, source_id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM edges WHERE source_nid = (SELECT nid FROM nodes WHERE id = ?)",
        [source_id],
    )?;
    Ok(())
}
