//! Read-only inspection commands: `status` (counts + git baseline) and `files`.

use crate::db;
use rusqlite::Connection;
use serde_json::json;
use std::error::Error;
use std::path::Path;
use std::process::Command;

struct FileRow {
    path: String,
    language: Option<String>,
    size_bytes: Option<i64>,
    is_test: bool,
    symbol_count: i64,
}

#[derive(Debug, PartialEq)]
enum Freshness {
    Fresh,
    Stale,
    Unknown,
}

impl Freshness {
    fn as_str(&self) -> &'static str {
        match self {
            Freshness::Fresh => "fresh",
            Freshness::Stale => "stale",
            Freshness::Unknown => "unknown",
        }
    }
}

fn live_git_head(workspace: &Path) -> Option<String> {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn live_git_status(workspace: &Path) -> Option<String> {
    Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            if String::from_utf8_lossy(&o.stdout).trim().is_empty() {
                "clean".to_string()
            } else {
                "dirty".to_string()
            }
        })
}

fn read_schema_version(conn: &Connection) -> Option<i64> {
    conn.query_row("SELECT MAX(version) FROM schema_versions", [], |row| {
        row.get(0)
    })
    .ok()
    .flatten()
}

fn classify_freshness(
    indexed_sha: &str,
    indexed_status: &str,
    head_sha: Option<&str>,
    working_tree: Option<&str>,
) -> Freshness {
    let Some(head_sha) = head_sha else {
        return Freshness::Unknown;
    };
    let Some(working_tree) = working_tree else {
        return Freshness::Unknown;
    };
    if indexed_sha == "N/A" || indexed_sha.is_empty() {
        return Freshness::Unknown;
    }
    if indexed_sha == head_sha && indexed_status == "clean" && working_tree == "clean" {
        Freshness::Fresh
    } else {
        Freshness::Stale
    }
}

/// The `status` command:
/// - Displays statistics: number of files, nodes, and edges currently indexed in the database.
pub fn run_status(workspace: &Path, json: bool) -> Result<(), Box<dyn Error>> {
    let db_path = workspace.join(".ochna").join("ochna.db");
    if !db_path.exists() {
        if json {
            let head_sha = live_git_head(workspace);
            let working_tree = live_git_status(workspace);
            let out = json!({
                "ok": false,
                "db_present": false,
                "schema": {
                    "expected": db::SCHEMA_VERSION,
                    "found": null,
                    "match": false,
                },
                "counts": {
                    "files": 0,
                    "nodes": 0,
                    "edges": 0,
                },
                "freshness": "unknown",
                "indexed_sha": null,
                "head_sha": head_sha,
                "working_tree": working_tree.unwrap_or_else(|| "unknown".to_string()),
                "action": "ochna init",
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
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
    let found_schema = read_schema_version(&conn);
    let schema_match = found_schema == Some(db::SCHEMA_VERSION);
    let head_sha = live_git_head(workspace);
    let working_tree = live_git_status(workspace);
    let freshness = classify_freshness(
        &git_commit_sha,
        &git_status,
        head_sha.as_deref(),
        working_tree.as_deref(),
    );
    let ok = schema_match && nodes_count > 0 && freshness != Freshness::Stale;
    let action = if !schema_match || nodes_count == 0 {
        "ochna init"
    } else if freshness == Freshness::Stale {
        "ochna sync"
    } else {
        "none"
    };

    if json {
        let out = json!({
            "ok": ok,
            "db_present": true,
            "schema": {
                "expected": db::SCHEMA_VERSION,
                "found": found_schema,
                "match": schema_match,
            },
            "counts": {
                "files": files_count,
                "nodes": nodes_count,
                "edges": edges_count,
            },
            "freshness": freshness.as_str(),
            "indexed_sha": git_commit_sha.clone(),
            "head_sha": head_sha.clone(),
            "working_tree": working_tree.unwrap_or_else(|| "unknown".to_string()),
            "action": action,
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
        if !ok {
            return Err(format!("Index is not ready. Run '{action}'.").into());
        }
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
