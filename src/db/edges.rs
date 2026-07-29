//! Edge writes and source-scoped deletion.

use super::{confidence_for_kind, label_for_kind, Edge, EdgeRecord};
use rusqlite::Connection;

/// Upsert an edge into the database (INSERT OR REPLACE)
pub fn upsert_edge(conn: &Connection, edge: &Edge) -> rusqlite::Result<()> {
    // Translate the string endpoints to their interned `nid` in SQL so callers
    // keep working with "file::symbol" ids. If either endpoint is missing the
    // SELECT yields no row and nothing is inserted (the edge has no valid node).
    let mut stmt = conn.prepare_cached(
        "INSERT OR REPLACE INTO edges (source_nid, target_nid, kind, resolution_kind)
         SELECT s.nid, t.nid, ?3, ?4 FROM nodes s, nodes t WHERE s.id = ?1 AND t.id = ?2",
    )?;
    stmt.execute((
        &edge.source_id,
        &edge.target_id,
        &edge.kind,
        edge.resolution_kind,
    ))?;
    Ok(())
}

pub fn delete_edges_for_source_id(conn: &Connection, source_id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM edges WHERE source_nid = (SELECT nid FROM nodes WHERE id = ?)",
        [source_id],
    )?;
    Ok(())
}

/// Reverse framework relations are stored dependency/event -> consumer even
/// though their raw relationship is owned by the consumer. Replaying that raw
/// source must invalidate only those reverse edges, not unrelated incoming
/// calls to the same node.
pub fn delete_reverse_framework_edges_for_target_id(
    conn: &Connection,
    target_id: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM edges
         WHERE target_nid = (SELECT nid FROM nodes WHERE id = ?)
           AND kind IN ('injected_into', 'consumes_event')",
        [target_id],
    )?;
    Ok(())
}

/// Return the actual indexed edge metadata between two symbols. Query commands
/// use this rather than recreating edge confidence from a related node.
pub fn find_edges_between(
    conn: &Connection,
    source_id: &str,
    target_id: &str,
    edge_kind: Option<&str>,
) -> rusqlite::Result<Vec<EdgeRecord>> {
    let mut stmt = conn.prepare(
        "SELECT e.kind, e.resolution_kind
         FROM edges e
         JOIN nodes source ON source.nid = e.source_nid
         JOIN nodes target ON target.nid = e.target_nid
         WHERE source.id = ?1 AND target.id = ?2 AND (?3 IS NULL OR e.kind = ?3)
         ORDER BY e.kind, e.resolution_kind DESC",
    )?;
    let rows = stmt.query_map((source_id, target_id, edge_kind), |row| {
        let resolution_kind: i64 = row.get(1)?;
        Ok(EdgeRecord {
            kind: row.get(0)?,
            resolution_kind: label_for_kind(resolution_kind).to_string(),
            confidence: confidence_for_kind(resolution_kind),
        })
    })?;
    rows.collect()
}
