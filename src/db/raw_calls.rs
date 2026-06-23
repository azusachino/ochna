//! Raw (unresolved) call-site storage and lookups used by edge resolution.

use super::{map_row_to_raw_call, RawCall};
use rusqlite::Connection;

/// Insert a raw call into the raw_calls table.
pub fn insert_raw_call(conn: &Connection, r: &RawCall) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO raw_calls (
            caller_nid, callee_name, callee_simple, callee_scope,
            call_kind, receiver_expr, receiver_type, package_or_namespace, import_hint,
            line, column
         ) SELECT nid, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11 FROM nodes WHERE id = ?1",
    )?;
    stmt.execute((
        &r.caller_id,
        &r.callee_name,
        &r.callee_simple,
        &r.callee_scope,
        &r.call_kind,
        &r.receiver_expr,
        &r.receiver_type,
        &r.package_or_namespace,
        &r.import_hint,
        r.line,
        r.column,
    ))?;
    Ok(())
}

pub fn get_raw_call_source_ids_by_callee_simple(
    conn: &Connection,
    callee_simple: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT n.id FROM raw_calls r
         JOIN nodes n ON n.nid = r.caller_nid
         WHERE r.callee_simple = ?",
    )?;
    let rows = stmt.query_map([callee_simple], |row| row.get(0))?;
    rows.collect()
}

pub fn get_raw_calls_for_source_id(
    conn: &Connection,
    source_id: &str,
) -> rusqlite::Result<Vec<RawCall>> {
    let mut stmt = conn.prepare(
        "SELECT n.id, r.callee_name, r.callee_simple, r.callee_scope,
                r.call_kind, r.receiver_expr, r.receiver_type, r.package_or_namespace, r.import_hint,
                r.line, r.column
         FROM raw_calls r
         JOIN nodes n ON n.nid = r.caller_nid
         WHERE n.id = ?",
    )?;
    let rows = stmt.query_map([source_id], map_row_to_raw_call)?;
    rows.collect()
}

/// Retrieve all raw calls from the database.
pub fn get_all_raw_calls(conn: &Connection) -> rusqlite::Result<Vec<RawCall>> {
    let mut stmt = conn.prepare(
        "SELECT n.id, r.callee_name, r.callee_simple, r.callee_scope,
                r.call_kind, r.receiver_expr, r.receiver_type, r.package_or_namespace, r.import_hint,
                r.line, r.column
         FROM raw_calls r
         JOIN nodes n ON n.nid = r.caller_nid",
    )?;
    let rows = stmt.query_map([], map_row_to_raw_call)?;
    let mut calls = Vec::new();
    for r in rows {
        calls.push(r?);
    }
    Ok(calls)
}
