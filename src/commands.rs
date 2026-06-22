//! CLI subcommand implementations, split by concern:
//! - [`index`] — the `init`/`sync` indexing pipeline.
//! - [`status`] — read-only `status`/`files` inspection.
//! - [`query`] — `search`/`callers`/`node`/`explore` graph queries.

mod index;
mod query;
mod status;

pub use index::run_init;
pub use query::{run_callers, run_explore, run_node, run_search};
pub use status::{run_files, run_status};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use rusqlite::Connection;
    use std::fs;
    use std::path::PathBuf;

    fn create_temp_dir() -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let count = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("ochna_test_{}_{}", now, count));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn test_commands_workflow() {
        let temp_workspace = create_temp_dir();

        // 1. Create a sub-directory and some mock files
        let src_dir = temp_workspace.join("src");
        fs::create_dir_all(&src_dir).unwrap();

        let rust_file = src_dir.join("main.rs");
        let rust_code = r#"
            /// A main entry point.
            fn main() {
                helper();
            }

            fn helper() {
                println!("hello");
            }
        "#;
        fs::write(&rust_file, rust_code).unwrap();

        let go_file = temp_workspace.join("main.go");
        let go_code = r#"
            package main
            import "fmt"

            // GoHelper function
            func GoHelper() {
                fmt.Println("go helper")
            }
        "#;
        fs::write(&go_file, go_code).unwrap();

        let c_file = temp_workspace.join("main.c");
        let c_code = r#"
            int c_helper(void) {
                return 1;
            }
        "#;
        fs::write(&c_file, c_code).unwrap();

        let cpp_file = temp_workspace.join("main.cpp");
        let cpp_code = r#"
            int cpp_helper() {
                return 2;
            }
        "#;
        fs::write(&cpp_file, cpp_code).unwrap();

        let zig_file = temp_workspace.join("main.zig");
        let zig_code = r#"
            fn zigHelper() i32 {
                return 3;
            }
        "#;
        fs::write(&zig_file, zig_code).unwrap();

        // Let's create an ignored directory/file to ensure they are skipped
        let ignored_dir = temp_workspace.join(".git");
        fs::create_dir_all(&ignored_dir).unwrap();
        fs::write(ignored_dir.join("config"), "dummy content").unwrap();

        let target_dir = temp_workspace.join("target");
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(target_dir.join("binary.rs"), "dummy rust in target").unwrap();

        // 2. Run run_init
        run_init(&temp_workspace).unwrap();

        // Verify .ochna/ochna.db was created
        let db_path = temp_workspace.join(".ochna").join("ochna.db");
        assert!(db_path.exists());

        // Verify status fetches expected data
        let conn = Connection::open(&db_path).unwrap();
        let files_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap();
        let nodes_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))
            .unwrap();

        // files should only contain supported source files outside ignored directories.
        assert_eq!(files_count, 5);
        // nodes:
        // rust: "main" (function), "helper" (function) -> 2 nodes
        // go: "GoHelper" (function) -> 1 node
        // c: "c_helper" (function) -> 1 node
        // cpp: "cpp_helper" (function) -> 1 node
        // zig: "zigHelper" (function) -> 1 node
        // Total nodes: 6
        assert_eq!(nodes_count, 6);

        // Run status command and verify it succeeds (text + json)
        run_status(&temp_workspace, false).unwrap();
        run_status(&temp_workspace, true).unwrap();

        // Run files command and verify it succeeds (text + json)
        run_files(&temp_workspace, false).unwrap();
        run_files(&temp_workspace, true).unwrap();

        // Verify new query commands query the SQLite database successfully and print expected output formats
        run_search(&temp_workspace, "helper", false).unwrap();
        run_search(&temp_workspace, "helper", true).unwrap();
        run_callers(&temp_workspace, "helper", false).unwrap();
        run_callers(&temp_workspace, "helper", true).unwrap();

        // Test run_node with file (symbols_only = false)
        run_node(
            &temp_workspace,
            Some("src/main.rs".to_string()),
            Some(1),
            Some(10),
            false,
            None,
            false,
            None,
            false,
        )
        .unwrap();
        // Test run_node with file (symbols_only = true)
        run_node(
            &temp_workspace,
            Some("src/main.rs".to_string()),
            None,
            None,
            true,
            None,
            false,
            None,
            false,
        )
        .unwrap();
        // Test run_node with symbol (include_code = true)
        run_node(
            &temp_workspace,
            None,
            None,
            None,
            false,
            Some("helper".to_string()),
            true,
            None,
            false,
        )
        .unwrap();
        // Test run_node with symbol (include_code = true, JSON output)
        run_node(
            &temp_workspace,
            None,
            None,
            None,
            false,
            Some("helper".to_string()),
            true,
            None,
            true,
        )
        .unwrap();
        // Test run_node with symbol (include_code = true and line filtering)
        run_node(
            &temp_workspace,
            None,
            None,
            None,
            false,
            Some("helper".to_string()),
            true,
            Some(6),
            false,
        )
        .unwrap();

        // Test run_explore (text + json)
        run_explore(&temp_workspace, "helper", false).unwrap();
        run_explore(&temp_workspace, "helper", true).unwrap();

        // 3. Modify a file and check that re-indexing works
        let rust_code_modified = r#"
            /// Modified main entry point.
            fn main() {
                // calls deleted helper
            }
        "#;
        fs::write(&rust_file, rust_code_modified).unwrap();

        // FileMetadata updates
        run_init(&temp_workspace).unwrap();

        let nodes_count_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))
            .unwrap();
        // Now rust file has 1 node ("main"). Other source files retain one node each.
        assert_eq!(nodes_count_after, 5);

        // Clean up temporary workspace
        fs::remove_dir_all(&temp_workspace).unwrap();
    }

    #[test]
    fn test_cross_file_edges_and_unresolved() {
        let temp_workspace = create_temp_dir();
        let src_dir = temp_workspace.join("src");
        fs::create_dir_all(&src_dir).unwrap();

        // a.rs calls target() (defined in b.rs) and missing() (defined nowhere).
        fs::write(
            src_dir.join("a.rs"),
            "fn caller() {\n    target();\n    missing();\n}\n",
        )
        .unwrap();
        fs::write(src_dir.join("b.rs"), "fn target() {}\n").unwrap();

        run_init(&temp_workspace).unwrap();

        let db_path = temp_workspace.join(".ochna").join("ochna.db");
        let conn = Connection::open(&db_path).unwrap();

        // A call edge must cross the file boundary: src/a.rs::caller -> src/b.rs::target.
        let cross_file: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM edges e \
                 JOIN nodes s ON e.source_nid = s.nid \
                 JOIN nodes t ON e.target_nid = t.nid \
                 WHERE s.file_path <> t.file_path",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cross_file, 1, "expected one cross-file call edge");

        // The call to an unindexed symbol must be recorded as an unresolved reference.
        let unresolved: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM unresolved_refs WHERE specifier = 'missing'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unresolved, 1, "expected one unresolved reference");

        fs::remove_dir_all(&temp_workspace).unwrap();
    }

    #[test]
    fn test_fresh_init_rebuilds_fts_index() {
        let temp_workspace = create_temp_dir();
        let src_dir = temp_workspace.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(
            src_dir.join("main.rs"),
            "/// Performs a searchable calibration.\nfn calibrate() {}\n",
        )
        .unwrap();

        run_init(&temp_workspace).unwrap();

        let db_path = temp_workspace.join(".ochna").join("ochna.db");
        let conn = Connection::open(&db_path).unwrap();
        let fts_results = db::search_nodes_fts(&conn, "calibration").unwrap();
        assert_eq!(fts_results.len(), 1);
        assert_eq!(fts_results[0].name, "calibrate");

        fs::remove_dir_all(&temp_workspace).unwrap();
    }

    #[test]
    fn test_incremental_sync_keeps_fts_triggers_after_fresh_rebuild() {
        let temp_workspace = create_temp_dir();
        let src_dir = temp_workspace.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        let rust_file = src_dir.join("main.rs");
        fs::write(
            &rust_file,
            "/// Mentions the original marker.\nfn searchable() {}\n",
        )
        .unwrap();

        run_init(&temp_workspace).unwrap();

        let db_path = temp_workspace.join(".ochna").join("ochna.db");
        let conn = Connection::open(&db_path).unwrap();
        assert_eq!(db::search_nodes_fts(&conn, "original").unwrap().len(), 1);

        fs::write(
            &rust_file,
            "/// Mentions the replacement marker.\nfn searchable() {}\n",
        )
        .unwrap();
        run_init(&temp_workspace).unwrap();

        assert_eq!(db::search_nodes_fts(&conn, "replacement").unwrap().len(), 1);
        assert!(
            db::search_nodes_fts(&conn, "original").unwrap().is_empty(),
            "updated file should remove stale FTS content"
        );

        fs::remove_file(&rust_file).unwrap();
        run_init(&temp_workspace).unwrap();

        assert!(
            db::search_nodes_fts(&conn, "replacement")
                .unwrap()
                .is_empty(),
            "deleted file should remove FTS content"
        );

        fs::remove_dir_all(&temp_workspace).unwrap();
    }

    #[test]
    fn test_incremental_sync_re_resolves_unmodified_incoming_callers() {
        let temp_workspace = create_temp_dir();
        let src_dir = temp_workspace.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        let caller_file = src_dir.join("a.rs");
        let old_target_file = src_dir.join("b.rs");
        let new_target_file = src_dir.join("c.rs");
        fs::write(
            &caller_file,
            "fn caller() {\n    target();\n    local_keep();\n}\nfn local_keep() {}\n",
        )
        .unwrap();
        fs::write(&old_target_file, "fn target() {}\n").unwrap();

        run_init(&temp_workspace).unwrap();

        fs::write(&old_target_file, "fn other() {}\n").unwrap();
        fs::write(&new_target_file, "fn target() {}\n").unwrap();
        run_init(&temp_workspace).unwrap();

        let db_path = temp_workspace.join(".ochna").join("ochna.db");
        let conn = Connection::open(&db_path).unwrap();
        let moved_edge: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM edges
                 WHERE source_nid = (SELECT nid FROM nodes WHERE id = 'src/a.rs::caller')
                   AND target_nid = (SELECT nid FROM nodes WHERE id = 'src/c.rs::target')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            moved_edge, 1,
            "unmodified caller should point at new target"
        );

        let stale_edge: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM edges
                 WHERE source_nid = (SELECT nid FROM nodes WHERE id = 'src/a.rs::caller')
                   AND target_nid = (SELECT nid FROM nodes WHERE id = 'src/b.rs::target')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale_edge, 0, "stale target edge should be removed");

        let preserved_edge: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM edges
                 WHERE source_nid = (SELECT nid FROM nodes WHERE id = 'src/a.rs::caller')
                   AND target_nid = (SELECT nid FROM nodes WHERE id = 'src/a.rs::local_keep')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            preserved_edge, 1,
            "other edges from the source are reinserted"
        );

        fs::remove_dir_all(&temp_workspace).unwrap();
    }

    #[test]
    fn test_incremental_sync_re_resolves_matching_unresolved_refs() {
        let temp_workspace = create_temp_dir();
        let src_dir = temp_workspace.join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("a.rs"), "fn caller() {\n    missing();\n}\n").unwrap();

        run_init(&temp_workspace).unwrap();

        let db_path = temp_workspace.join(".ochna").join("ochna.db");
        let conn = Connection::open(&db_path).unwrap();
        let unresolved_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM unresolved_refs WHERE specifier = 'missing'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unresolved_before, 1);

        fs::write(src_dir.join("b.rs"), "fn missing() {}\n").unwrap();
        run_init(&temp_workspace).unwrap();

        let resolved_edge: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM edges
                 WHERE source_nid = (SELECT nid FROM nodes WHERE id = 'src/a.rs::caller')
                   AND target_nid = (SELECT nid FROM nodes WHERE id = 'src/b.rs::missing')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(resolved_edge, 1);

        let unresolved_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM unresolved_refs WHERE specifier = 'missing'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unresolved_after, 0);

        fs::remove_dir_all(&temp_workspace).unwrap();
    }
}
