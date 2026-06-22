//! Unresolved-reference storage and specifier-based lookups for re-resolution.

use super::{split_callee_name, UnresolvedRef};
use rusqlite::Connection;

/// Insert an unresolved reference (a call whose target symbol is not indexed).
pub fn insert_unresolved_ref(conn: &Connection, r: &UnresolvedRef) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO unresolved_refs (source_nid, specifier, kind, line, column)
         SELECT nid, ?2, ?3, ?4, ?5 FROM nodes WHERE id = ?1",
        (&r.source_id, &r.specifier, &r.kind, r.line, r.column),
    )?;
    Ok(())
}

pub fn delete_unresolved_refs_for_source_id(
    conn: &Connection,
    source_id: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM unresolved_refs WHERE source_nid = (SELECT nid FROM nodes WHERE id = ?)",
        [source_id],
    )?;
    Ok(())
}

pub fn get_unresolved_source_ids_by_specifier_simple(
    conn: &Connection,
    specifier_simple: &str,
) -> rusqlite::Result<Vec<String>> {
    let like_pattern = format!("%::{}", specifier_simple);
    let mut stmt = conn.prepare(
        "SELECT DISTINCT n.id, u.specifier FROM unresolved_refs u
         JOIN nodes n ON n.nid = u.source_nid
         WHERE u.specifier = ? OR u.specifier LIKE ?",
    )?;
    let rows = stmt.query_map([specifier_simple, &like_pattern], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut source_ids = Vec::new();
    for row in rows {
        let (source_id, specifier) = row?;
        if split_callee_name(&specifier).1 == specifier_simple {
            source_ids.push(source_id);
        }
    }
    source_ids.sort();
    source_ids.dedup();
    Ok(source_ids)
}
