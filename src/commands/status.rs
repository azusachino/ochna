//! Read-only inspection commands: `status` (counts + git baseline) and `files`.

use crate::{commands::discover_source_files, db};
use rusqlite::Connection;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
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

const LOW_QUALITY_MIN_CALLS: i64 = 5;
const LOW_QUALITY_RATE: f64 = 0.20;
const SUPPORTED_EXTENSIONS: &[&str] = &[
    "rs", "go", "java", "c", "h", "cc", "cpp", "cxx", "hh", "hpp", "hxx", "zig",
];
const UNSUPPORTED_SOURCE_EXTENSIONS: &[&str] = &[
    "py", "js", "jsx", "ts", "tsx", "rb", "php", "cs", "kt", "kts", "swift", "scala", "sh", "lua",
];

#[derive(Debug)]
struct UnresolvedRow {
    source: String,
    specifier: String,
    kind: String,
    file_path: String,
    line: i64,
    column: i64,
    reason: String,
}

#[derive(Default)]
struct LocationCounts {
    calls: i64,
    unresolved: i64,
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
        .args(["status", "--porcelain", "--", ".", ":(exclude).ochna"])
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
                "working_tree": working_tree,
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
            "working_tree": working_tree,
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

fn unresolved_rows(
    conn: &Connection,
    in_path: Option<&str>,
    limit: Option<usize>,
) -> rusqlite::Result<Vec<UnresolvedRow>> {
    let sql = if limit.is_some() {
        "SELECT n.id, u.specifier, u.kind, n.file_path, u.line, u.column \
         FROM unresolved_refs u JOIN nodes n ON n.nid = u.source_nid \
         WHERE (?1 IS NULL OR n.file_path LIKE ?1 || '%') \
         ORDER BY n.file_path, u.line, u.column, u.specifier LIMIT ?2"
    } else {
        "SELECT n.id, u.specifier, u.kind, n.file_path, u.line, u.column \
         FROM unresolved_refs u JOIN nodes n ON n.nid = u.source_nid \
         WHERE (?1 IS NULL OR n.file_path LIKE ?1 || '%') \
         ORDER BY n.file_path, u.line, u.column, u.specifier"
    };
    let mut stmt = conn.prepare(sql)?;
    let mut query = match limit {
        Some(limit) => stmt.query((in_path, limit as i64))?,
        None => stmt.query([in_path])?,
    };
    let mut rows = Vec::new();
    while let Some(row) = query.next()? {
        rows.push((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
        ));
    }
    let mut result = Vec::with_capacity(rows.len());
    for (source, specifier, kind, file_path, line, column) in rows {
        let simple = specifier.rsplit("::").next().unwrap_or(&specifier);
        let candidates: i64 = conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE name = ?1",
            [simple],
            |row| row.get(0),
        )?;
        let reason = if matches!(kind.as_str(), "macro_or_function" | "indirect_call") {
            kind.clone()
        } else if candidates > 0 {
            "ambiguous_target".to_string()
        } else {
            "missing_target".to_string()
        };
        result.push(UnresolvedRow {
            source,
            specifier,
            kind,
            file_path,
            line,
            column,
            reason,
        });
    }
    Ok(result)
}

fn source_extension_counts(workspace: &Path) -> BTreeMap<String, i64> {
    fn visit(dir: &Path, counts: &mut BTreeMap<String, i64>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if matches!(
                    name.as_ref(),
                    ".git"
                        | ".ochna"
                        | "clones"
                        | "target"
                        | "node_modules"
                        | ".venv"
                        | "vendor"
                        | "build"
                        | "dist"
                ) || name.starts_with('.')
                {
                    continue;
                }
                visit(&path, counts);
            } else if path.is_file() {
                let Some(ext) = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(str::to_ascii_lowercase)
                else {
                    continue;
                };
                if !SUPPORTED_EXTENSIONS.contains(&ext.as_str())
                    && UNSUPPORTED_SOURCE_EXTENSIONS.contains(&ext.as_str())
                {
                    *counts.entry(format!(".{ext}")).or_default() += 1;
                }
            }
        }
    }

    let mut counts = BTreeMap::new();
    visit(workspace, &mut counts);
    counts
}

fn calculate_content_hash(content: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Freshness is structural: unrelated dirty files warn but do not invalidate a
/// graph whose indexed source content still matches the workspace.
fn indexed_sources_are_fresh(conn: &Connection, workspace: &Path) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare("SELECT file_path, content_hash FROM files ORDER BY file_path")?;
    let indexed: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if indexed.is_empty() {
        return Ok(false);
    }
    let discovered = discover_source_files(workspace)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let discovered: BTreeSet<String> = discovered
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let indexed_paths: BTreeSet<String> = indexed.iter().map(|(path, _)| path.clone()).collect();
    if discovered != indexed_paths {
        return Ok(false);
    }
    Ok(indexed.into_iter().all(|(path, hash)| {
        fs::read_to_string(workspace.join(path))
            .map(|content| calculate_content_hash(&content) == hash)
            .unwrap_or(false)
    }))
}

fn parent_location(path: &str) -> String {
    PathBuf::from(path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| ".".to_string())
}

fn graph_quality(conn: &Connection, workspace: &Path) -> rusqlite::Result<serde_json::Value> {
    let mut tiers = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT resolution_kind, COUNT(*) FROM edges GROUP BY resolution_kind ORDER BY resolution_kind DESC",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
    for row in rows {
        let (kind, edges) = row?;
        tiers.push(json!({"kind": db::label_for_kind(kind), "confidence": db::confidence_for_kind(kind), "edges": edges}));
    }

    let unresolved = unresolved_rows(conn, None, None)?;
    let mut unresolved_counts = BTreeMap::<String, i64>::new();
    let mut files = BTreeMap::<String, LocationCounts>::new();
    let mut packages = BTreeMap::<String, LocationCounts>::new();
    for row in &unresolved {
        *unresolved_counts.entry(row.reason.clone()).or_default() += 1;
        files.entry(row.file_path.clone()).or_default().unresolved += 1;
        packages
            .entry(parent_location(&row.file_path))
            .or_default()
            .unresolved += 1;
    }

    let mut raw = conn.prepare(
        "SELECT n.file_path, COUNT(*) FROM raw_calls r JOIN nodes n ON n.nid = r.caller_nid GROUP BY n.file_path ORDER BY n.file_path",
    )?;
    for row in raw.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })? {
        let (path, calls) = row?;
        files.entry(path.clone()).or_default().calls = calls;
        packages.entry(parent_location(&path)).or_default().calls += calls;
    }
    let mut locations = Vec::new();
    for (path, counts) in files.into_iter().chain(packages) {
        if counts.calls >= LOW_QUALITY_MIN_CALLS {
            let rate = counts.unresolved as f64 / counts.calls as f64;
            if rate >= LOW_QUALITY_RATE {
                locations
                    .push(json!({"path": path, "reason": "high_unresolved_rate", "rate": rate}));
            }
        }
    }
    locations.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    locations.dedup_by(|a, b| a["path"] == b["path"]);

    let mut collision_names = BTreeSet::new();
    let mut definitions = BTreeMap::<String, i64>::new();
    let mut low_confidence_edges = BTreeMap::<String, i64>::new();
    let mut defs =
        conn.prepare("SELECT name, COUNT(*) FROM nodes GROUP BY name HAVING COUNT(*) > 1")?;
    for row in defs.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })? {
        let (name, count) = row?;
        collision_names.insert(name.clone());
        definitions.insert(name, count);
    }
    let mut weak = conn.prepare(
        "SELECT n.name, COUNT(*) FROM edges e JOIN nodes n ON n.nid = e.target_nid WHERE e.resolution_kind < 2 GROUP BY n.name",
    )?;
    for row in weak.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })? {
        let (name, count) = row?;
        collision_names.insert(name.clone());
        low_confidence_edges.insert(name, count);
    }
    let collision_prone_names: Vec<_> = collision_names
        .into_iter()
        .map(|name| json!({"name": name, "definitions": definitions.get(&name).copied().unwrap_or(1), "low_confidence_edges": low_confidence_edges.get(&name).copied().unwrap_or(0)}))
        .collect();
    let unsupported_extensions = source_extension_counts(workspace)
        .into_iter()
        .map(|(extension, files)| json!({"extension": extension, "files": files}))
        .collect::<Vec<_>>();

    Ok(json!({
        "resolution_tiers": tiers,
        "unresolved": {
            "missing_target": unresolved_counts.get("missing_target").copied().unwrap_or(0),
            "ambiguous_target": unresolved_counts.get("ambiguous_target").copied().unwrap_or(0),
            "total": unresolved.len(),
        },
        "collision_prone_names": collision_prone_names,
        "low_quality_locations": locations,
        "unsupported_extensions": unsupported_extensions,
    }))
}

/// Emit every unresolved call site in stable source order.
pub fn run_unresolved(
    workspace: &Path,
    in_path: Option<&str>,
    limit: usize,
    json_mode: bool,
) -> Result<(), Box<dyn Error>> {
    let db_path = workspace.join(".ochna").join("ochna.db");
    if !db_path.exists() {
        return Err("Database not initialized. Run the 'init' command first.".into());
    }
    let conn = Connection::open(db_path)?;
    let mut rows = unresolved_rows(&conn, in_path, Some(limit + 1))?;
    let truncated = rows.len() > limit;
    if truncated {
        rows.pop();
    }
    if json_mode {
        let references: Vec<_> = rows.iter().map(|row| json!({"source": row.source, "specifier": row.specifier, "kind": row.kind, "file_path": row.file_path, "line": row.line, "column": row.column, "reason": row.reason})).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"contract_version": "0.3", "command": "unresolved", "ok": true, "data": {"references": references}, "warnings": [], "truncated": truncated, "next_action": if truncated { "narrow the query" } else { "none" }})
            )?
        );
    } else {
        println!("Unresolved references ({}):", rows.len());
        for row in rows {
            println!(
                "{}:{}:{} {} ({})",
                row.file_path, row.line, row.column, row.specifier, row.reason
            );
        }
    }
    Ok(())
}

/// Emit the v0.3 graph trust preflight without modifying the workspace or index.
pub fn run_doctor(workspace: &Path, json_mode: bool) -> Result<(), Box<dyn Error>> {
    let db_path = workspace.join(".ochna").join("ochna.db");
    let head = live_git_head(workspace);
    let dirty = live_git_status(workspace).as_deref() == Some("dirty");
    if !db_path.exists() {
        let data = json!({"index": {"present": false, "path": ".ochna/ochna.db"}, "schema": {"expected": db::SCHEMA_VERSION, "actual": null, "match": false}, "git": {"head": head, "baseline": null, "dirty": dirty}, "freshness": "missing", "graph_quality": {"trust_verdict": "unusable", "resolution_tiers": [], "unresolved": {"missing_target": 0, "ambiguous_target": 0, "total": 0}, "collision_prone_names": [], "low_quality_locations": [], "unsupported_extensions": source_extension_counts(workspace).into_iter().map(|(extension, files)| json!({"extension": extension, "files": files})).collect::<Vec<_>>()}});
        if json_mode {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"contract_version": "0.3", "command": "doctor", "ok": false, "data": data, "warnings": [], "truncated": false, "next_action": "ochna init"})
                )?
            );
        } else {
            println!("Index: missing (.ochna/ochna.db)");
            println!(
                "Schema: expected {}, actual none, match false",
                db::SCHEMA_VERSION
            );
            println!("Git baseline: none");
            println!("Freshness: missing");
            println!("Graph trust: unusable");
            println!("Resolution tiers: []");
            println!("Unresolved: {{\"missing_target\":0,\"ambiguous_target\":0,\"total\":0}}");
            println!("Collision-prone names: []");
            println!("Low-quality locations: []");
            println!(
                "Unsupported extensions: {}",
                data["graph_quality"]["unsupported_extensions"]
            );
            println!("Action: ochna init");
        }
        return Err("Index is unavailable. Run 'ochna init'.".into());
    }
    let conn = Connection::open(db_path)?;
    let actual = read_schema_version(&conn);
    let schema_match = actual == Some(db::SCHEMA_VERSION);
    let baseline = db::get_project_metadata(&conn, "git_commit_sha")?;
    let freshness = if indexed_sources_are_fresh(&conn, workspace)? {
        Freshness::Fresh
    } else {
        Freshness::Stale
    };
    let freshness_name = if schema_match {
        freshness.as_str()
    } else {
        "incompatible"
    };
    let mut quality = graph_quality(&conn, workspace)?;
    let graph_degraded = quality["low_quality_locations"]
        .as_array()
        .is_some_and(|locations| !locations.is_empty())
        || quality["unsupported_extensions"]
            .as_array()
            .is_some_and(|extensions| !extensions.is_empty());
    let verdict = if !schema_match {
        "unusable"
    } else if freshness == Freshness::Fresh && !graph_degraded {
        "trusted"
    } else {
        "degraded"
    };
    quality["trust_verdict"] = json!(verdict);
    let ok = verdict == "trusted";
    let next_action = if !schema_match {
        "ochna init"
    } else if freshness == Freshness::Stale {
        "ochna sync"
    } else {
        "none"
    };
    let warnings = if dirty {
        vec![
            json!({"code": "dirty_worktree", "message": "Git worktree is dirty; diff may include uncommitted changes."}),
        ]
    } else {
        vec![]
    };
    let data = json!({"index": {"present": true, "path": ".ochna/ochna.db"}, "schema": {"expected": db::SCHEMA_VERSION, "actual": actual, "match": schema_match}, "git": {"head": head, "baseline": baseline, "dirty": dirty}, "freshness": freshness_name, "graph_quality": quality});
    if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"contract_version": "0.3", "command": "doctor", "ok": ok, "data": data, "warnings": warnings, "truncated": false, "next_action": next_action})
            )?
        );
    } else {
        println!("Index: present (.ochna/ochna.db)");
        println!(
            "Schema: expected {}, actual {:?}, match {}",
            db::SCHEMA_VERSION,
            actual,
            schema_match
        );
        println!("Git baseline: {}", baseline.as_deref().unwrap_or("none"));
        println!("Freshness: {freshness_name}");
        println!("Graph trust: {verdict}");
        println!("Resolution tiers: {}", quality["resolution_tiers"]);
        println!("Unresolved: {}", quality["unresolved"]);
        println!(
            "Collision-prone names: {}",
            quality["collision_prone_names"]
        );
        println!(
            "Low-quality locations: {}",
            quality["low_quality_locations"]
        );
        println!(
            "Unsupported extensions: {}",
            quality["unsupported_extensions"]
        );
        println!("Action: {next_action}");
    }
    if ok {
        Ok(())
    } else if graph_degraded && freshness == Freshness::Fresh {
        Err("Graph quality is degraded; review low-quality locations and unsupported source extensions.".into())
    } else {
        Err(format!("Graph trust is {verdict}. Run '{next_action}'.").into())
    }
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

#[cfg(test)]
mod diagnostics_tests {
    use super::*;
    use crate::commands::run_init;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn workspace() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ochna_diagnostics_{stamp}_{}",
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(path.join("src")).unwrap();
        path
    }

    #[test]
    fn diagnostics_report_tiers_collisions_unresolved_and_unsupported_sources() {
        let workspace = workspace();
        fs::write(
            workspace.join("src/lib.rs"),
            "fn shared() {}\nfn caller() { shared(); missing(); }\n",
        )
        .unwrap();
        fs::write(workspace.join("src/other.rs"), "fn shared() {}\n").unwrap();
        fs::write(workspace.join("tool.py"), "def ignored(): pass\n").unwrap();
        fs::create_dir_all(workspace.join("clones")).unwrap();
        fs::write(workspace.join("clones/ignored.py"), "def ignored(): pass\n").unwrap();
        run_init(&workspace, false).unwrap();

        let conn = Connection::open(workspace.join(".ochna/ochna.db")).unwrap();
        let quality = graph_quality(&conn, &workspace).unwrap();
        assert_eq!(quality["unresolved"]["missing_target"], 1);
        assert_eq!(quality["unresolved"]["total"], 1);
        assert_eq!(quality["collision_prone_names"][0]["name"], "shared");
        assert_eq!(quality["unsupported_extensions"][0]["extension"], ".py");
        assert_eq!(quality["unsupported_extensions"][0]["files"], 1);
        assert!(quality["resolution_tiers"].is_array());

        let unresolved = unresolved_rows(&conn, Some("src"), Some(50)).unwrap();
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].specifier, "missing");
        assert_eq!(unresolved[0].reason, "missing_target");
        assert!(indexed_sources_are_fresh(&conn, &workspace).unwrap());
        fs::write(workspace.join("README.md"), "dirty but non-structural\n").unwrap();
        assert!(indexed_sources_are_fresh(&conn, &workspace).unwrap());
        fs::write(workspace.join("src/new.rs"), "fn added() {}\n").unwrap();
        assert!(!indexed_sources_are_fresh(&conn, &workspace).unwrap());
        fs::remove_file(workspace.join("src/new.rs")).unwrap();
        fs::write(workspace.join("src/lib.rs"), "fn changed() {}\n").unwrap();
        assert!(!indexed_sources_are_fresh(&conn, &workspace).unwrap());
        fs::remove_dir_all(workspace).unwrap();
    }
}
