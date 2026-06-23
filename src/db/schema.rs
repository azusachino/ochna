//! Single source of truth for every piece of schema DDL plus the routines that
//! apply it: `init_schema`, the migration step, and the FTS trigger toggles all
//! reference the constants here so table/index/trigger definitions live in
//! exactly one place.

use rusqlite::Connection;

/// Current on-disk schema version. Bump when a change is not backward
/// compatible; `init_schema` then drops the data tables and the caller rebuilds.
const SCHEMA_VERSION: i64 = 5;

// `nid` aliases rowid (the compact surrogate referenced by every edge/call);
// `id` keeps the human-readable "file::symbol" key, UNIQUE so resolution and
// upserts can still look a node up by string.
const NODES: &str = "CREATE TABLE IF NOT EXISTS nodes (
        nid INTEGER PRIMARY KEY,
        id TEXT NOT NULL UNIQUE,
        name TEXT NOT NULL,
        kind TEXT NOT NULL,
        qualified_name TEXT,
        file_path TEXT NOT NULL,
        start_line INTEGER NOT NULL,
        end_line INTEGER NOT NULL,
        start_column INTEGER NOT NULL,
        end_column INTEGER NOT NULL,
        signature TEXT,
        doc_comment TEXT,
        is_test INTEGER NOT NULL DEFAULT 0
     )";

// WITHOUT ROWID folds the (source_nid, target_nid, kind) primary key into the
// table b-tree, so there is no separate PK autoindex duplicating the keys.
const EDGES: &str = "CREATE TABLE IF NOT EXISTS edges (
        source_nid INTEGER NOT NULL,
        target_nid INTEGER NOT NULL,
        kind TEXT NOT NULL,
        resolution_kind INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (source_nid, target_nid, kind),
        FOREIGN KEY (source_nid) REFERENCES nodes(nid) ON DELETE CASCADE,
        FOREIGN KEY (target_nid) REFERENCES nodes(nid) ON DELETE CASCADE
     ) WITHOUT ROWID";

const FILES: &str = "CREATE TABLE IF NOT EXISTS files (
        file_path TEXT PRIMARY KEY,
        content_hash TEXT NOT NULL,
        language TEXT,
        size_bytes INTEGER,
        last_modified INTEGER,
        is_test INTEGER NOT NULL DEFAULT 0
     )";

const UNRESOLVED_REFS: &str = "CREATE TABLE IF NOT EXISTS unresolved_refs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        source_nid INTEGER NOT NULL,
        specifier TEXT NOT NULL,
        kind TEXT NOT NULL,
        line INTEGER NOT NULL,
        column INTEGER NOT NULL,
        FOREIGN KEY (source_nid) REFERENCES nodes(nid) ON DELETE CASCADE
     )";

const RAW_CALLS: &str = "CREATE TABLE IF NOT EXISTS raw_calls (
        caller_nid INTEGER NOT NULL,
        callee_name TEXT NOT NULL,
        callee_simple TEXT,
        callee_scope TEXT,
        call_kind TEXT,
        receiver_expr TEXT,
        receiver_type TEXT,
        package_or_namespace TEXT,
        import_hint TEXT,
        line INTEGER NOT NULL,
        column INTEGER NOT NULL,
        FOREIGN KEY (caller_nid) REFERENCES nodes(nid) ON DELETE CASCADE
     )";

const PROJECT_METADATA: &str = "CREATE TABLE IF NOT EXISTS project_metadata (
        key TEXT PRIMARY KEY,
        value TEXT
     )";

/// Bootstrapped on its own ahead of the migration step, which reads it.
const SCHEMA_VERSIONS: &str = "CREATE TABLE IF NOT EXISTS schema_versions (
        version INTEGER PRIMARY KEY,
        applied_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
     )";

/// Data tables created after migration (`nodes` first; children reference it).
const TABLES: &[&str] = &[
    NODES,
    EDGES,
    FILES,
    UNRESOLVED_REFS,
    RAW_CALLS,
    PROJECT_METADATA,
];

const FTS_TABLE: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
        name,
        qualified_name,
        signature,
        doc_comment,
        content='nodes',
        content_rowid='rowid'
     )";

// No index on edges(source_nid): the PRIMARY KEY (source_nid, target_nid, kind)
// already serves source-prefixed lookups (find_callees, delete-by-source).
// find_callers filters on target_nid, which the PK can't serve, so that reverse
// index is the one we keep.
const INDEXES: &[&str] = &[
    "CREATE INDEX IF NOT EXISTS idx_raw_calls_caller_nid ON raw_calls(caller_nid)",
    "CREATE INDEX IF NOT EXISTS idx_raw_calls_callee_simple ON raw_calls(callee_simple)",
    "CREATE INDEX IF NOT EXISTS idx_nodes_file_path ON nodes(file_path)",
    "CREATE INDEX IF NOT EXISTS idx_nodes_name ON nodes(name)",
    "CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(kind)",
    "CREATE INDEX IF NOT EXISTS idx_edges_target_nid ON edges(target_nid)",
];

/// FTS sync triggers as (name, DDL) so create and drop share one definition.
const FTS_TRIGGERS: &[(&str, &str)] = &[
    (
        "nodes_ai",
        "CREATE TRIGGER IF NOT EXISTS nodes_ai AFTER INSERT ON nodes BEGIN
            INSERT INTO nodes_fts(rowid, name, qualified_name, signature, doc_comment)
            VALUES (new.rowid, new.name, new.qualified_name, new.signature, new.doc_comment);
         END;",
    ),
    (
        "nodes_ad",
        "CREATE TRIGGER IF NOT EXISTS nodes_ad AFTER DELETE ON nodes BEGIN
            INSERT INTO nodes_fts(nodes_fts, rowid, name, qualified_name, signature, doc_comment)
            VALUES ('delete', old.rowid, old.name, old.qualified_name, old.signature, old.doc_comment);
         END;",
    ),
    (
        "nodes_au",
        "CREATE TRIGGER IF NOT EXISTS nodes_au AFTER UPDATE ON nodes BEGIN
            INSERT INTO nodes_fts(nodes_fts, rowid, name, qualified_name, signature, doc_comment)
            VALUES ('delete', old.rowid, old.name, old.qualified_name, old.signature, old.doc_comment);
            INSERT INTO nodes_fts(rowid, name, qualified_name, signature, doc_comment)
            VALUES (new.rowid, new.name, new.qualified_name, new.signature, new.doc_comment);
         END;",
    ),
];

/// Data tables dropped (children first) on a non-backward-compatible migration.
const MIGRATION_DROP: &[&str] = &[
    "edges",
    "raw_calls",
    "unresolved_refs",
    "nodes_fts",
    "nodes",
    "files",
];

/// Initialize the SQLite database schema.
/// Enforces foreign key constraints and creates all necessary tables,
/// indexes, the FTS5 virtual table, and the update/delete/insert triggers.
pub fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("PRAGMA foreign_keys = ON;", [])?;

    // schema_versions must exist before the migration step can read it.
    conn.execute(SCHEMA_VERSIONS, [])?;
    run_migration(conn)?;

    for ddl in TABLES {
        conn.execute(ddl, [])?;
    }
    conn.execute(FTS_TABLE, [])?;
    create_node_fts_triggers(conn)?;
    for ddl in INDEXES {
        conn.execute(ddl, [])?;
    }

    conn.execute(
        "INSERT OR IGNORE INTO schema_versions (version) VALUES (?1)",
        [SCHEMA_VERSION],
    )?;

    Ok(())
}

/// Schema v2 interns node identity to an integer surrogate (`nid`) stored on
/// edges/raw_calls/unresolved_refs instead of the long text `id`. The graph is
/// re-derivable from source, so an older DB is migrated by dropping the data
/// tables and letting the caller rebuild — no fragile in-place column rewrite.
fn run_migration(conn: &Connection) -> rusqlite::Result<()> {
    let current_version: Option<i64> =
        conn.query_row("SELECT MAX(version) FROM schema_versions", [], |row| {
            row.get(0)
        })?;
    if matches!(current_version, Some(v) if v < SCHEMA_VERSION) {
        for table in MIGRATION_DROP {
            conn.execute(&format!("DROP TABLE IF EXISTS {table}"), [])?;
        }
        conn.execute("DELETE FROM schema_versions", [])?;
    }
    Ok(())
}

/// Create triggers that keep the external-content FTS table synchronized with nodes.
pub fn create_node_fts_triggers(conn: &Connection) -> rusqlite::Result<()> {
    for (_, ddl) in FTS_TRIGGERS {
        conn.execute(ddl, [])?;
    }
    Ok(())
}

/// Drop node FTS maintenance triggers so fresh builds can bulk-load nodes cheaply.
pub fn drop_node_fts_triggers(conn: &Connection) -> rusqlite::Result<()> {
    for (name, _) in FTS_TRIGGERS {
        conn.execute(&format!("DROP TRIGGER IF EXISTS {name}"), [])?;
    }
    Ok(())
}

/// Rebuild the external-content FTS index from the current nodes table.
pub fn rebuild_node_fts(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO nodes_fts(nodes_fts) VALUES('rebuild')", [])?;
    Ok(())
}
