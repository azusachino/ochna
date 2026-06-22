//! Read-only inspection commands: `status` (counts + git baseline) and `files`.

use crate::db;
use rusqlite::Connection;
use serde_json::json;
use std::error::Error;
use std::path::Path;

struct FileRow {
    path: String,
    language: Option<String>,
    size_bytes: Option<i64>,
    is_test: bool,
    symbol_count: i64,
}

/// The `status` command:
/// - Displays statistics: number of files, nodes, and edges currently indexed in the database.
pub fn run_status(workspace: &Path, json: bool) -> Result<(), Box<dyn Error>> {
    let db_path = workspace.join(".ochna").join("ochna.db");
    if !db_path.exists() {
        return Err("Database not initialized. Run the 'init' command first.".into());
    }

    let conn = Connection::open(&db_path)?;

    let files_count: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
    let nodes_count: i64 = conn.query_row("SELECT COUNT(*) FROM nodes", [], |row| row.get(0))?;
    let edges_count: i64 = conn.query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))?;

    let git_commit_sha =
        db::get_project_metadata(&conn, "git_commit_sha")?.unwrap_or_else(|| "N/A".to_string());
    let git_commit_subject =
        db::get_project_metadata(&conn, "git_commit_subject")?.unwrap_or_else(|| "N/A".to_string());
    let git_commit_date =
        db::get_project_metadata(&conn, "git_commit_date")?.unwrap_or_else(|| "N/A".to_string());
    let git_branch =
        db::get_project_metadata(&conn, "git_branch")?.unwrap_or_else(|| "N/A".to_string());
    let git_status =
        db::get_project_metadata(&conn, "git_status")?.unwrap_or_else(|| "N/A".to_string());
    let indexed_at =
        db::get_project_metadata(&conn, "indexed_at")?.unwrap_or_else(|| "N/A".to_string());

    if json {
        let out = json!({
            "files": files_count,
            "nodes": nodes_count,
            "edges": edges_count,
            "git": {
                "commit_sha": git_commit_sha,
                "commit_subject": git_commit_subject,
                "commit_date": git_commit_date,
                "branch": git_branch,
                "status": git_status,
            },
            "indexed_at": indexed_at,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("Database Status:");
    println!("  Files: {}", files_count);
    println!("  Nodes: {}", nodes_count);
    println!("  Edges: {}", edges_count);
    println!();
    println!("Project Baseline Info:");
    println!("  Commit SHA:  {}", git_commit_sha);
    println!("  Commit Msg:  {}", git_commit_subject);
    println!("  Commit Date: {}", git_commit_date);
    println!("  Branch:      {}", git_branch);
    println!("  Git Status:  {}", git_status);
    println!("  Indexed At:  {}", indexed_at);

    Ok(())
}

/// The `files` command:
/// - Prints a list of indexed files with symbol count, language, and size.
pub fn run_files(workspace: &Path, json: bool) -> Result<(), Box<dyn Error>> {
    let db_path = workspace.join(".ochna").join("ochna.db");
    if !db_path.exists() {
        return Err("Database not initialized. Run the 'init' command first.".into());
    }

    let conn = Connection::open(&db_path)?;

    let mut stmt = conn.prepare(
        "SELECT f.file_path, f.language, f.size_bytes, f.is_test, COUNT(n.id) \
         FROM files f \
         LEFT JOIN nodes n ON f.file_path = n.file_path \
         GROUP BY f.file_path \
         ORDER BY f.file_path",
    )?;

    let rows: Vec<FileRow> = stmt
        .query_map([], |row| {
            Ok(FileRow {
                path: row.get(0)?,
                language: row.get(1)?,
                size_bytes: row.get(2)?,
                is_test: row.get(3)?,
                symbol_count: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    if json {
        let files: Vec<_> = rows
            .iter()
            .map(|row| {
                json!({
                    "file_path": row.path,
                    "language": row.language,
                    "size_bytes": row.size_bytes,
                    "is_test": row.is_test,
                    "symbols": row.symbol_count,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&files)?);
        return Ok(());
    }

    println!(
        "{:<40} {:<10} {:<10} {:<8} {:<12}",
        "File Path", "Language", "Size (B)", "Test", "Symbols"
    );
    println!("{}", "-".repeat(85));

    for row in rows {
        let lang_str = row.language.unwrap_or_else(|| "unknown".to_string());
        let size_str = row
            .size_bytes
            .map(|s| s.to_string())
            .unwrap_or_else(|| "-".to_string());

        println!(
            "{:<40} {:<10} {:<10} {:<8} {:<12}",
            row.path, lang_str, size_str, row.is_test, row.symbol_count
        );
    }

    Ok(())
}
