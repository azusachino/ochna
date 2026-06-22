use serde::{Deserialize, Serialize};

mod edges;
mod files;
mod nodes;
mod raw_calls;
mod refs;
mod schema;

pub use edges::{delete_edges_for_source_id, upsert_edge};
pub use files::{
    get_file_metadata, get_project_metadata, upsert_file_metadata, upsert_project_metadata,
};
pub use nodes::{
    delete_file_data, find_callees, find_callers, get_node_ids_and_names_for_file, query_nodes,
    search_nodes_fts, upsert_node,
};
pub use raw_calls::{
    get_all_raw_calls, get_raw_call_source_ids_by_callee_simple, get_raw_calls_for_source_id,
    insert_raw_call,
};
pub use refs::{
    delete_unresolved_refs_for_source_id, get_unresolved_source_ids_by_specifier_simple,
    insert_unresolved_ref,
};
pub use schema::{create_node_fts_triggers, drop_node_fts_triggers, init_schema, rebuild_node_fts};

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

pub(crate) fn split_callee_name(callee_name: &str) -> (Option<String>, String) {
    if let Some((scope, simple)) = callee_name.rsplit_once("::") {
        (Some(scope.to_string()), simple.to_string())
    } else {
        (None, callee_name.to_string())
    }
}

/// Helper mapping database Row back to a Node structure
pub(crate) fn map_row_to_node(row: &rusqlite::Row) -> rusqlite::Result<Node> {
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

pub(crate) fn map_row_to_raw_call(row: &rusqlite::Row) -> rusqlite::Result<RawCall> {
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
