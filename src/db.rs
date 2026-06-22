use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub qualified_name: Option<String>,
    pub file_path: String,
    pub start_line: i64,
    pub end_line: i64,
    pub start_column: i64,
    pub end_column: i64,
    pub signature: Option<String>,
    pub doc_comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Edge {
    pub source_id: String,
    pub target_id: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileMetadata {
    pub file_path: String,
    pub content_hash: String,
    pub language: Option<String>,
    pub size_bytes: Option<i64>,
    pub last_modified: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectMetadata {
    pub key: String,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnresolvedRef {
    pub id: Option<i64>,
    pub source_id: String,
    pub specifier: String,
    pub kind: String,
    pub line: i64,
    pub column: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawCall {
    pub caller_id: String,
    pub callee_name: String,
    pub callee_simple: String,
    pub callee_scope: Option<String>,
    pub line: i64,
    pub column: i64,
}

impl RawCall {
    pub fn new(caller_id: String, callee_name: String, line: i64, column: i64) -> Self {
        let (callee_scope, callee_simple) = split_callee_name(&callee_name);
        Self {
            caller_id,
            callee_name,
            callee_simple,
            callee_scope,
            line,
            column,
        }
    }
}

fn split_callee_name(callee_name: &str) -> (Option<String>, String) {
    if let Some((scope, simple)) = callee_name.rsplit_once("::") {
        (Some(scope.to_string()), simple.to_string())
    } else {
        (None, callee_name.to_string())
    }
}

/// Initialize the SQLite database schema.
/// Enforces foreign key constraints and creates all necessary tables,
/// indexes, the FTS5 virtual table, and the update/delete/insert triggers.
pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    // Enable foreign keys
    conn.execute("PRAGMA foreign_keys = ON;", [])?;

    // Create tables
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_versions (
            version INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
         )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS nodes (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            qualified_name TEXT,
            file_path TEXT NOT NULL,
            start_line INTEGER NOT NULL,
            end_line INTEGER NOT NULL,
            start_column INTEGER NOT NULL,
            end_column INTEGER NOT NULL,
            signature TEXT,
            doc_comment TEXT
         )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS edges (
            source_id TEXT NOT NULL,
            target_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            PRIMARY KEY (source_id, target_id, kind),
            FOREIGN KEY (source_id) REFERENCES nodes(id) ON DELETE CASCADE,
            FOREIGN KEY (target_id) REFERENCES nodes(id) ON DELETE CASCADE
         )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS files (
            file_path TEXT PRIMARY KEY,
            content_hash TEXT NOT NULL,
            language TEXT,
            size_bytes INTEGER,
            last_modified INTEGER
         )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS unresolved_refs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_id TEXT NOT NULL,
            specifier TEXT NOT NULL,
            kind TEXT NOT NULL,
            line INTEGER NOT NULL,
            column INTEGER NOT NULL,
            FOREIGN KEY (source_id) REFERENCES nodes(id) ON DELETE CASCADE
         )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS raw_calls (
            caller_id TEXT NOT NULL,
            callee_name TEXT NOT NULL,
            callee_simple TEXT,
            callee_scope TEXT,
            line INTEGER NOT NULL,
            column INTEGER NOT NULL,
            FOREIGN KEY (caller_id) REFERENCES nodes(id) ON DELETE CASCADE
         )",
        [],
    )?;
    migrate_raw_calls_schema(conn)?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_raw_calls_caller_id ON raw_calls(caller_id)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_raw_calls_callee_simple ON raw_calls(callee_simple)",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS project_metadata (
            key TEXT PRIMARY KEY,
            value TEXT
         )",
        [],
    )?;

    // FTS5 Virtual Table for nodes
    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
            name,
            qualified_name,
            signature,
            doc_comment,
            content='nodes',
            content_rowid='rowid'
         )",
        [],
    )?;

    create_node_fts_triggers(conn)?;

    // Indexes for query performance optimization
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_nodes_file_path ON nodes(file_path);",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_nodes_name ON nodes(name);",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(kind);",
        [],
    )?;
    // No index on edges(source_id): the PRIMARY KEY (source_id, target_id, kind)
    // already serves source_id-prefixed lookups (find_callees, delete-by-source),
    // so a dedicated index is pure duplication of the text IDs. Drop it from any
    // pre-existing DB on the next init/sync.
    conn.execute("DROP INDEX IF EXISTS idx_edges_source_id;", [])?;
    // find_callers filters on target_id, which the PK can't serve — keep this one.
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_edges_target_id ON edges(target_id);",
        [],
    )?;

    // Record schema version (Version 1)
    conn.execute(
        "INSERT OR IGNORE INTO schema_versions (version) VALUES (1)",
        [],
    )?;

    Ok(())
}

/// Create triggers that keep the external-content FTS table synchronized with nodes.
pub fn create_node_fts_triggers(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS nodes_ai AFTER INSERT ON nodes BEGIN
            INSERT INTO nodes_fts(rowid, name, qualified_name, signature, doc_comment)
            VALUES (new.rowid, new.name, new.qualified_name, new.signature, new.doc_comment);
         END;",
        [],
    )?;

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS nodes_ad AFTER DELETE ON nodes BEGIN
            INSERT INTO nodes_fts(nodes_fts, rowid, name, qualified_name, signature, doc_comment)
            VALUES ('delete', old.rowid, old.name, old.qualified_name, old.signature, old.doc_comment);
         END;",
        [],
    )?;

    conn.execute(
        "CREATE TRIGGER IF NOT EXISTS nodes_au AFTER UPDATE ON nodes BEGIN
            INSERT INTO nodes_fts(nodes_fts, rowid, name, qualified_name, signature, doc_comment)
            VALUES ('delete', old.rowid, old.name, old.qualified_name, old.signature, old.doc_comment);
            INSERT INTO nodes_fts(rowid, name, qualified_name, signature, doc_comment)
            VALUES (new.rowid, new.name, new.qualified_name, new.signature, new.doc_comment);
         END;",
        [],
    )?;

    Ok(())
}

/// Drop node FTS maintenance triggers so fresh builds can bulk-load nodes cheaply.
pub fn drop_node_fts_triggers(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("DROP TRIGGER IF EXISTS nodes_ai", [])?;
    conn.execute("DROP TRIGGER IF EXISTS nodes_ad", [])?;
    conn.execute("DROP TRIGGER IF EXISTS nodes_au", [])?;
    Ok(())
}

/// Rebuild the external-content FTS index from the current nodes table.
pub fn rebuild_node_fts(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO nodes_fts(nodes_fts) VALUES('rebuild')", [])?;
    Ok(())
}

fn migrate_raw_calls_schema(conn: &Connection) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(raw_calls)")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    if !columns.iter().any(|column| column == "callee_simple") {
        conn.execute("ALTER TABLE raw_calls ADD COLUMN callee_simple TEXT", [])?;
    }
    if !columns.iter().any(|column| column == "callee_scope") {
        conn.execute("ALTER TABLE raw_calls ADD COLUMN callee_scope TEXT", [])?;
    }
    // caller_file was a derived, never-queried column (and a redundant index on
    // it). Shed both from any pre-existing DB. DROP COLUMN needs SQLite 3.35+;
    // ignore failure so older engines simply keep an unused column.
    conn.execute("DROP INDEX IF EXISTS idx_raw_calls_caller_file", [])?;
    if columns.iter().any(|column| column == "caller_file") {
        let _ = conn.execute("ALTER TABLE raw_calls DROP COLUMN caller_file", []);
    }

    let mut stmt = conn.prepare(
        "SELECT rowid, caller_id, callee_name FROM raw_calls
         WHERE callee_simple IS NULL",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    for (rowid, caller_id, callee_name) in rows {
        let call = RawCall::new(caller_id, callee_name, 0, 0);
        conn.execute(
            "UPDATE raw_calls
             SET callee_simple = ?, callee_scope = ?
             WHERE rowid = ?",
            (&call.callee_simple, &call.callee_scope, rowid),
        )?;
    }

    Ok(())
}

/// Helper mapping database Row back to a Node structure
fn map_row_to_node(row: &rusqlite::Row) -> rusqlite::Result<Node> {
    Ok(Node {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: row.get(2)?,
        qualified_name: row.get(3)?,
        file_path: row.get(4)?,
        start_line: row.get(5)?,
        end_line: row.get(6)?,
        start_column: row.get(7)?,
        end_column: row.get(8)?,
        signature: row.get(9)?,
        doc_comment: row.get(10)?,
    })
}

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

/// Upsert an edge into the database (INSERT OR REPLACE)
pub fn upsert_edge(conn: &Connection, edge: &Edge) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT OR REPLACE INTO edges (source_id, target_id, kind) VALUES (?, ?, ?)",
    )?;
    stmt.execute((&edge.source_id, &edge.target_id, &edge.kind))?;
    Ok(())
}

/// Upsert file metadata into the database (INSERT OR REPLACE)
pub fn upsert_file_metadata(conn: &Connection, file: &FileMetadata) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO files (file_path, content_hash, language, size_bytes, last_modified)
         VALUES (?, ?, ?, ?, ?)",
        (
            &file.file_path,
            &file.content_hash,
            &file.language,
            file.size_bytes,
            file.last_modified,
        ),
    )?;
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

pub fn get_raw_call_source_ids_by_callee_simple(
    conn: &Connection,
    callee_simple: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT DISTINCT caller_id FROM raw_calls WHERE callee_simple = ?")?;
    let rows = stmt.query_map([callee_simple], |row| row.get(0))?;
    rows.collect()
}

pub fn get_unresolved_source_ids_by_specifier_simple(
    conn: &Connection,
    specifier_simple: &str,
) -> rusqlite::Result<Vec<String>> {
    let like_pattern = format!("%::{}", specifier_simple);
    let mut stmt = conn.prepare(
        "SELECT DISTINCT source_id, specifier FROM unresolved_refs
         WHERE specifier = ? OR specifier LIKE ?",
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

pub fn get_raw_calls_for_source_id(
    conn: &Connection,
    source_id: &str,
) -> rusqlite::Result<Vec<RawCall>> {
    let mut stmt = conn.prepare(
        "SELECT caller_id, callee_name, callee_simple, callee_scope, line, column
         FROM raw_calls
         WHERE caller_id = ?",
    )?;
    let rows = stmt.query_map([source_id], map_row_to_raw_call)?;
    rows.collect()
}

pub fn delete_edges_for_source_id(conn: &Connection, source_id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM edges WHERE source_id = ?", [source_id])?;
    Ok(())
}

pub fn delete_unresolved_refs_for_source_id(
    conn: &Connection,
    source_id: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM unresolved_refs WHERE source_id = ?",
        [source_id],
    )?;
    Ok(())
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

/// Get file metadata by file path
pub fn get_file_metadata(
    conn: &Connection,
    file_path: &str,
) -> rusqlite::Result<Option<FileMetadata>> {
    let mut stmt = conn.prepare(
        "SELECT file_path, content_hash, language, size_bytes, last_modified FROM files WHERE file_path = ?"
    )?;
    let mut rows = stmt.query([file_path])?;
    if let Some(row) = rows.next()? {
        Ok(Some(FileMetadata {
            file_path: row.get(0)?,
            content_hash: row.get(1)?,
            language: row.get(2)?,
            size_bytes: row.get(3)?,
            last_modified: row.get(4)?,
        }))
    } else {
        Ok(None)
    }
}

/// Upsert project metadata into the database
pub fn upsert_project_metadata(
    conn: &Connection,
    key: &str,
    value: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO project_metadata (key, value) VALUES (?, ?)",
        (key, value),
    )?;
    Ok(())
}

/// Retrieve project metadata value by key
pub fn get_project_metadata(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM project_metadata WHERE key = ?")?;
    let mut rows = stmt.query([key])?;
    if let Some(row) = rows.next()? {
        let val: Option<String> = row.get(0)?;
        Ok(val)
    } else {
        Ok(None)
    }
}

/// Insert an unresolved reference (a call whose target symbol is not indexed).
pub fn insert_unresolved_ref(conn: &Connection, r: &UnresolvedRef) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO unresolved_refs (source_id, specifier, kind, line, column)
         VALUES (?, ?, ?, ?, ?)",
        (&r.source_id, &r.specifier, &r.kind, r.line, r.column),
    )?;
    Ok(())
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
             JOIN edges e ON n.id = e.source_id \
             WHERE e.target_id = ? AND e.kind = ?",
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
             JOIN edges e ON n.id = e.source_id \
             WHERE e.target_id = ?",
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
             JOIN edges e ON n.id = e.target_id \
             WHERE e.source_id = ? AND e.kind = ?",
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
             JOIN edges e ON n.id = e.target_id \
             WHERE e.source_id = ?",
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

/// Insert a raw call into the raw_calls table.
pub fn insert_raw_call(conn: &Connection, r: &RawCall) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO raw_calls (
            caller_id, callee_name, callee_simple, callee_scope, line, column
         ) VALUES (?, ?, ?, ?, ?, ?)",
    )?;
    stmt.execute((
        &r.caller_id,
        &r.callee_name,
        &r.callee_simple,
        &r.callee_scope,
        r.line,
        r.column,
    ))?;
    Ok(())
}

/// Retrieve all raw calls from the database.
pub fn get_all_raw_calls(conn: &Connection) -> rusqlite::Result<Vec<RawCall>> {
    let mut stmt = conn.prepare(
        "SELECT caller_id, callee_name, callee_simple, callee_scope, line, column
         FROM raw_calls",
    )?;
    let rows = stmt.query_map([], map_row_to_raw_call)?;
    let mut calls = Vec::new();
    for r in rows {
        calls.push(r?);
    }
    Ok(calls)
}

fn map_row_to_raw_call(row: &rusqlite::Row) -> rusqlite::Result<RawCall> {
    let caller_id: String = row.get(0)?;
    let callee_name: String = row.get(1)?;
    let line: i64 = row.get(4)?;
    let column: i64 = row.get(5)?;
    let mut call = RawCall::new(caller_id, callee_name, line, column);
    if let Some(callee_simple) = row.get::<_, Option<String>>(2)? {
        call.callee_simple = callee_simple;
    }
    call.callee_scope = row.get(3)?;
    Ok(call)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn test_db_workflow() {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();

        // 1. Insert file metadata
        let file_meta = FileMetadata {
            file_path: "src/main.rs".to_string(),
            content_hash: "abcdef123456".to_string(),
            language: Some("rust".to_string()),
            size_bytes: Some(1024),
            last_modified: Some(1670000000),
        };
        upsert_file_metadata(&conn, &file_meta).unwrap();

        // Verify file metadata insertion
        let retrieved_file = get_file_metadata(&conn, "src/main.rs").unwrap().unwrap();
        assert_eq!(retrieved_file, file_meta);

        // 2. Insert nodes
        let node_main = Node {
            id: "src/main.rs::main".to_string(),
            name: "main".to_string(),
            kind: "function".to_string(),
            qualified_name: Some("main".to_string()),
            file_path: "src/main.rs".to_string(),
            start_line: 11,
            end_line: 15,
            start_column: 9,
            end_column: 1,
            signature: Some("fn main()".to_string()),
            doc_comment: Some("Main entrypoint".to_string()),
        };

        let node_helper = Node {
            id: "src/main.rs::helper".to_string(),
            name: "helper".to_string(),
            kind: "function".to_string(),
            qualified_name: Some("helper".to_string()),
            file_path: "src/main.rs".to_string(),
            start_line: 20,
            end_line: 25,
            start_column: 0,
            end_column: 0,
            signature: Some("fn helper()".to_string()),
            doc_comment: Some("Helper function that does magic".to_string()),
        };

        upsert_node(&conn, &node_main).unwrap();
        upsert_node(&conn, &node_helper).unwrap();

        // Verify querying nodes
        let nodes_by_name = query_nodes(&conn, Some("main"), None, None).unwrap();
        assert_eq!(nodes_by_name.len(), 1);
        assert_eq!(nodes_by_name[0], node_main);

        let nodes_by_path = query_nodes(&conn, None, None, Some("src/main.rs")).unwrap();
        assert_eq!(nodes_by_path.len(), 2);

        // 3. Insert edges
        let edge_call = Edge {
            source_id: "src/main.rs::main".to_string(),
            target_id: "src/main.rs::helper".to_string(),
            kind: "calls".to_string(),
        };
        upsert_edge(&conn, &edge_call).unwrap();

        // Verify callers/callees
        let callers = find_callers(&conn, "src/main.rs::helper", Some("calls")).unwrap();
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].id, "src/main.rs::main");

        let callees = find_callees(&conn, "src/main.rs::main", Some("calls")).unwrap();
        assert_eq!(callees.len(), 1);
        assert_eq!(callees[0].id, "src/main.rs::helper");

        // 4. Test FTS (full-text search)
        let fts_results = search_nodes_fts(&conn, "magic").unwrap();
        assert_eq!(fts_results.len(), 1);
        assert_eq!(fts_results[0].id, "src/main.rs::helper");

        let fts_results_doc = search_nodes_fts(&conn, "entrypoint").unwrap();
        assert_eq!(fts_results_doc.len(), 1);
        assert_eq!(fts_results_doc[0].id, "src/main.rs::main");

        // 5. Test project metadata
        upsert_project_metadata(&conn, "version", Some("0.1.0")).unwrap();
        let version_val = get_project_metadata(&conn, "version").unwrap();
        assert_eq!(version_val, Some("0.1.0".to_string()));

        // 6. Test deletion
        delete_file_data(&conn, "src/main.rs").unwrap();

        // Verify nodes are deleted
        let nodes_after_delete = query_nodes(&conn, None, None, Some("src/main.rs")).unwrap();
        assert!(nodes_after_delete.is_empty());

        // Verify edges are cascade-deleted
        let callers_after_delete =
            find_callers(&conn, "src/main.rs::helper", Some("calls")).unwrap();
        assert!(callers_after_delete.is_empty());

        // Verify file metadata is deleted
        let file_meta_after_delete = get_file_metadata(&conn, "src/main.rs").unwrap();
        assert!(file_meta_after_delete.is_none());

        // Verify FTS is updated after delete
        let fts_results_after_delete = search_nodes_fts(&conn, "magic").unwrap();
        assert!(fts_results_after_delete.is_empty());
    }
}
