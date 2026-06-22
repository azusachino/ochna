//! Node CRUD, dynamic queries, full-text search, and call-graph traversal.

use super::{map_row_to_node, Node};
use rusqlite::Connection;

/// Upsert a node into the database (INSERT OR REPLACE)
pub fn upsert_node(conn: &Connection, node: &Node) -> rusqlite::Result<()> {
    // prepare_cached so the statement is parsed once and reused across the
    // millions of inserts a large repo produces, not re-parsed per row.
    let mut stmt = conn.prepare_cached(
        "INSERT OR REPLACE INTO nodes (
            id, name, kind, qualified_name, file_path,
            start_line, end_line, start_column, end_column,
            signature, doc_comment
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )?;
    stmt.execute((
        &node.id,
        &node.name,
        &node.kind,
        &node.qualified_name,
        &node.file_path,
        node.start_line,
        node.end_line,
        node.start_column,
        node.end_column,
        &node.signature,
        &node.doc_comment,
    ))?;
    Ok(())
}

/// Deletes file metadata, associated nodes, edges and unresolved references.
/// Leverages ON DELETE CASCADE for associated tables linked to the nodes.
pub fn delete_file_data(conn: &Connection, file_path: &str) -> rusqlite::Result<()> {
    // Ensure foreign key constraints are active
    conn.execute("PRAGMA foreign_keys = ON;", [])?;

    // Deleting nodes under this file_path automatically cascades deletion
    // to their associated edges and unresolved_refs tables.
    conn.execute("DELETE FROM nodes WHERE file_path = ?", [file_path])?;

    // Also delete the file metadata entry
    conn.execute("DELETE FROM files WHERE file_path = ?", [file_path])?;

    Ok(())
}

pub fn get_node_ids_and_names_for_file(
    conn: &Connection,
    file_path: &str,
) -> rusqlite::Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT id, name FROM nodes WHERE file_path = ?")?;
    let rows = stmt.query_map([file_path], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

/// Query nodes dynamically by matching name, kind, and/or file_path
pub fn query_nodes(
    conn: &Connection,
    name: Option<&str>,
    kind: Option<&str>,
    file_path: Option<&str>,
) -> rusqlite::Result<Vec<Node>> {
    let mut query = "SELECT id, name, kind, qualified_name, file_path, start_line, end_line, start_column, end_column, signature, doc_comment FROM nodes WHERE 1=1".to_string();

    let mut name_bind = None;
    let mut kind_bind = None;
    let mut file_path_bind = None;

    if let Some(n) = name {
        query.push_str(" AND name = :name");
        name_bind = Some(n);
    }
    if let Some(k) = kind {
        query.push_str(" AND kind = :kind");
        kind_bind = Some(k);
    }
    if let Some(f) = file_path {
        query.push_str(" AND file_path = :file_path");
        file_path_bind = Some(f);
    }

    let mut stmt = conn.prepare(&query)?;

    let mut params = Vec::new();
    if let Some(ref n) = name_bind {
        params.push((":name", n as &dyn rusqlite::ToSql));
    }
    if let Some(ref k) = kind_bind {
        params.push((":kind", k as &dyn rusqlite::ToSql));
    }
    if let Some(ref f) = file_path_bind {
        params.push((":file_path", f as &dyn rusqlite::ToSql));
    }

    let mut rows = stmt.query(params.as_slice())?;
    let mut nodes = Vec::new();
    while let Some(row) = rows.next()? {
        nodes.push(map_row_to_node(row)?);
    }
    Ok(nodes)
}

/// Find callers (symbols referencing/calling a target node)
pub fn find_callers(
    conn: &Connection,
    target_id: &str,
    edge_kind: Option<&str>,
) -> rusqlite::Result<Vec<Node>> {
    if let Some(kind) = edge_kind {
        let mut stmt = conn.prepare(
            "SELECT n.id, n.name, n.kind, n.qualified_name, n.file_path, \
                    n.start_line, n.end_line, n.start_column, n.end_column, \
                    n.signature, n.doc_comment \
             FROM nodes n \
             JOIN edges e ON n.nid = e.source_nid \
             WHERE e.target_nid = (SELECT nid FROM nodes WHERE id = ?) AND e.kind = ?",
        )?;
        let mut rows = stmt.query([target_id, kind])?;
        let mut nodes = Vec::new();
        while let Some(row) = rows.next()? {
            nodes.push(map_row_to_node(row)?);
        }
        Ok(nodes)
    } else {
        let mut stmt = conn.prepare(
            "SELECT n.id, n.name, n.kind, n.qualified_name, n.file_path, \
                    n.start_line, n.end_line, n.start_column, n.end_column, \
                    n.signature, n.doc_comment \
             FROM nodes n \
             JOIN edges e ON n.nid = e.source_nid \
             WHERE e.target_nid = (SELECT nid FROM nodes WHERE id = ?)",
        )?;
        let mut rows = stmt.query([target_id])?;
        let mut nodes = Vec::new();
        while let Some(row) = rows.next()? {
            nodes.push(map_row_to_node(row)?);
        }
        Ok(nodes)
    }
}

/// Find callees (symbols called by a source node)
pub fn find_callees(
    conn: &Connection,
    source_id: &str,
    edge_kind: Option<&str>,
) -> rusqlite::Result<Vec<Node>> {
    if let Some(kind) = edge_kind {
        let mut stmt = conn.prepare(
            "SELECT n.id, n.name, n.kind, n.qualified_name, n.file_path, \
                    n.start_line, n.end_line, n.start_column, n.end_column, \
                    n.signature, n.doc_comment \
             FROM nodes n \
             JOIN edges e ON n.nid = e.target_nid \
             WHERE e.source_nid = (SELECT nid FROM nodes WHERE id = ?) AND e.kind = ?",
        )?;
        let mut rows = stmt.query([source_id, kind])?;
        let mut nodes = Vec::new();
        while let Some(row) = rows.next()? {
            nodes.push(map_row_to_node(row)?);
        }
        Ok(nodes)
    } else {
        let mut stmt = conn.prepare(
            "SELECT n.id, n.name, n.kind, n.qualified_name, n.file_path, \
                    n.start_line, n.end_line, n.start_column, n.end_column, \
                    n.signature, n.doc_comment \
             FROM nodes n \
             JOIN edges e ON n.nid = e.target_nid \
             WHERE e.source_nid = (SELECT nid FROM nodes WHERE id = ?)",
        )?;
        let mut rows = stmt.query([source_id])?;
        let mut nodes = Vec::new();
        while let Some(row) = rows.next()? {
            nodes.push(map_row_to_node(row)?);
        }
        Ok(nodes)
    }
}

/// Full-text search on nodes via nodes_fts
pub fn search_nodes_fts(conn: &Connection, query_str: &str) -> rusqlite::Result<Vec<Node>> {
    let mut stmt = conn.prepare(
        "SELECT n.id, n.name, n.kind, n.qualified_name, n.file_path, \
                n.start_line, n.end_line, n.start_column, n.end_column, \
                n.signature, n.doc_comment \
         FROM nodes n \
         JOIN nodes_fts f ON n.rowid = f.rowid \
         WHERE nodes_fts MATCH ? \
         ORDER BY rank",
    )?;
    let mut rows = stmt.query([query_str])?;
    let mut nodes = Vec::new();
    while let Some(row) = rows.next()? {
        nodes.push(map_row_to_node(row)?);
    }
    Ok(nodes)
}
