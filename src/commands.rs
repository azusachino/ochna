use crate::db;
use crate::parser;
use rayon::prelude::*;
use rusqlite::Connection;
use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::error::Error;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};

/// A source file that was read and parsed off the DB thread. Parsing is the
/// CPU-bound part of indexing and is independent per file, so it runs in
/// parallel; the resulting nodes/calls are written to the single SQLite
/// transaction serially afterwards.
struct ParsedFile {
    relative_path: String,
    language: &'static str,
    content_hash: String,
    size_bytes: i64,
    last_modified: i64,
    nodes: Vec<db::Node>,
    calls: Vec<db::RawCall>,
}

/// Recursively scans `current_dir` for supported source files.
/// Relative paths are calculated with respect to `base_dir`.
fn scan_dir(base_dir: &Path, current_dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if current_dir.is_dir() {
        for entry in fs::read_dir(current_dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_name = path.file_name().unwrap_or_default().to_string_lossy();

            if path.is_dir() {
                // Ignore dotfiles, target directory, node_modules, etc.
                if file_name.starts_with('.')
                    || file_name == "target"
                    || file_name == "node_modules"
                {
                    continue;
                }
                scan_dir(base_dir, &path, files)?;
            } else if path.is_file() && language_for_path(&path).is_some() {
                if let Ok(rel_path) = path.strip_prefix(base_dir) {
                    files.push(rel_path.to_path_buf());
                }
            }
        }
    }
    Ok(())
}

fn language_for_path(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    match ext.as_str() {
        "rs" => Some("rust"),
        "go" => Some("go"),
        "java" => Some("java"),
        // `.h` is ambiguous (C vs C++); we treat it as C. C++-only headers
        // should use `.hpp`/`.hh`/`.hxx` to parse with the C++ grammar.
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" => Some("cpp"),
        "zig" => Some("zig"),
        _ => None,
    }
}

/// Calculate a simple hash of the file contents.
fn calculate_hash(content: &str) -> String {
    let mut s = DefaultHasher::new();
    content.hash(&mut s);
    format!("{:x}", s.finish())
}

#[allow(clippy::type_complexity)]
fn get_git_info(
    workspace: &Path,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    use std::process::Command;

    let commit_info = Command::new("git")
        .args(["log", "-1", "--format=%H|%s|%cd", "--date=iso"])
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    let (commit_sha, commit_subject, commit_date) = match commit_info {
        Some(info) => {
            let parts: Vec<&str> = info.split('|').collect();
            let sha = parts.first().map(|s| s.to_string());
            let subject = parts.get(1).map(|s| s.to_string());
            let date = parts.get(2).map(|s| s.to_string());
            (sha, subject, date)
        }
        None => (None, None, None),
    };

    let branch = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(workspace)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    let status = Command::new("git")
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
        });

    (commit_sha, commit_subject, commit_date, branch, status)
}

/// The `init` command:
/// - Creates a `.ochna/` directory in the workspace.
/// - Opens/creates the SQLite database at `.ochna/ochna.db`.
/// - Initializes the schema.
/// - Recursively scans for supported source files.
/// - Computes hashes, and updates database for new/modified files.
/// - Resolves call edges across files and records unmatched calls as unresolved refs.
pub fn run_init(workspace: &Path) -> Result<(), Box<dyn Error>> {
    let ochna_dir = workspace.join(".ochna");
    if !ochna_dir.exists() {
        fs::create_dir_all(&ochna_dir)?;
    }

    let db_path = ochna_dir.join("ochna.db");
    // A brand-new DB is a full build that is entirely re-derivable from source,
    // so we can relax durability for a large I/O win — a crash mid-build just
    // means re-running. An existing DB means this is an incremental update
    // (`sync`, or a re-`init`) mutating data we don't want to lose to a crash,
    // so it stays crash-safe on WAL + synchronous=NORMAL (the safe-and-fast WAL
    // pairing). WAL persists in the DB header, so subsequent queries get it too.
    let is_fresh_build = !db_path.exists();
    let mut conn = Connection::open(&db_path)?;
    if is_fresh_build {
        conn.pragma_update(None, "journal_mode", "MEMORY")?;
        conn.pragma_update(None, "synchronous", "OFF")?;
    } else {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
    }
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "cache_size", -262_144)?; // ~256 MB page cache
    db::init_schema(&conn)?;

    let tx = conn.transaction()?;
    if is_fresh_build {
        db::drop_node_fts_triggers(&tx)?;
    }

    let mut files = Vec::new();
    scan_dir(workspace, workspace, &mut files)?;

    let mut affected_source_ids: FxHashSet<String> = FxHashSet::default();
    let mut changed_symbol_names: FxHashSet<String> = FxHashSet::default();

    // Prune files that are no longer on disk
    {
        let disk_files: FxHashSet<String> = files
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let mut stmt = tx.prepare("SELECT file_path FROM files")?;
        let db_files: Result<Vec<String>, _> = stmt.query_map([], |row| row.get(0))?.collect();
        let db_files = db_files?;
        for db_file in db_files {
            if !disk_files.contains(&db_file) {
                for (id, name) in db::get_node_ids_and_names_for_file(&tx, &db_file)? {
                    affected_source_ids.insert(id);
                    changed_symbol_names.insert(name);
                }
                info!("Pruning deleted file from index: {}", db_file);
                db::delete_file_data(&tx, &db_file)?;
            }
        }
    }

    info!("Found {} source files to index.", files.len());

    // Snapshot existing file metadata once so the modified-check is an in-memory
    // lookup, leaving the parallel phase free of any DB access.
    let existing_meta: FxHashMap<String, db::FileMetadata> = {
        let mut stmt = tx.prepare(
            "SELECT file_path, content_hash, language, size_bytes, last_modified FROM files",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                db::FileMetadata {
                    file_path: row.get(0)?,
                    content_hash: row.get(1)?,
                    language: row.get(2)?,
                    size_bytes: row.get(3)?,
                    last_modified: row.get(4)?,
                },
            ))
        })?;
        rows.collect::<Result<_, _>>()?
    };

    // Read + hash + parse modified files across all cores. Unchanged,
    // unreadable, or unsupported files are filtered out here. A dedicated pool
    // gives workers a large stack: AST traversal recurses with source nesting
    // depth and the default worker stack overflows on deeply-nested files.
    let pool = rayon::ThreadPoolBuilder::new()
        .stack_size(64 * 1024 * 1024)
        .build()?;
    let parsed: Vec<ParsedFile> = pool.install(|| {
        files
            .par_iter()
            .filter_map(|file_path| {
                let absolute_path = workspace.join(file_path);
                let metadata = fs::metadata(&absolute_path).ok()?;
                let size_bytes = metadata.len() as i64;
                let last_modified = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let relative_path_str = file_path.to_string_lossy().to_string();

                let content = match fs::read_to_string(&absolute_path) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("Could not read file {}: {}", file_path.display(), e);
                        return None;
                    }
                };
                let current_hash = calculate_hash(&content);

                let is_modified = match existing_meta.get(&relative_path_str) {
                    None => true,
                    Some(meta) => {
                        meta.content_hash != current_hash
                            || meta.last_modified != Some(last_modified)
                            || meta.size_bytes != Some(size_bytes)
                    }
                };
                if !is_modified {
                    debug!("Up to date: {}", relative_path_str);
                    return None;
                }

                let language = language_for_path(&absolute_path)?;
                match parser::parse_code(&relative_path_str, &content, language) {
                    Ok((nodes, calls)) => Some(ParsedFile {
                        relative_path: relative_path_str,
                        language,
                        content_hash: current_hash,
                        size_bytes,
                        last_modified,
                        nodes,
                        calls,
                    }),
                    Err(e) => {
                        error!("Error parsing {}: {}", relative_path_str, e);
                        None
                    }
                }
            })
            .collect()
    });

    for pf in &parsed {
        for (id, name) in db::get_node_ids_and_names_for_file(&tx, &pf.relative_path)? {
            affected_source_ids.insert(id);
            changed_symbol_names.insert(name);
        }
        for node in &pf.nodes {
            affected_source_ids.insert(node.id.clone());
            changed_symbol_names.insert(node.name.clone());
        }
    }

    // Write parsed results into the single transaction serially.
    for pf in &parsed {
        debug!("Indexing: {}", pf.relative_path);
        db::delete_file_data(&tx, &pf.relative_path)?;
        for node in &pf.nodes {
            db::upsert_node(&tx, node)?;
        }
        for call in &pf.calls {
            db::insert_raw_call(&tx, call)?;
        }
        db::upsert_file_metadata(
            &tx,
            &db::FileMetadata {
                file_path: pf.relative_path.clone(),
                content_hash: pf.content_hash.clone(),
                language: Some(pf.language.to_string()),
                size_bytes: Some(pf.size_bytes),
                last_modified: Some(pf.last_modified),
            },
        )?;
    }

    let mut symbol_index = parser::SymbolIndexBuilder::default();
    {
        let mut stmt = tx.prepare("SELECT id, name, file_path, qualified_name FROM nodes")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let file: String = row.get(2)?;
            let qualified_name: Option<String> = row.get(3)?;
            symbol_index.push(&id, &name, &file, qualified_name.as_deref());
        }
    }
    let symbol_index = symbol_index.finish();

    let calls_to_resolve = if is_fresh_build {
        tx.execute("DELETE FROM edges", [])?;
        tx.execute("DELETE FROM unresolved_refs", [])?;
        db::get_all_raw_calls(&tx)?
    } else {
        for name in &changed_symbol_names {
            for source_id in db::get_raw_call_source_ids_by_callee_simple(&tx, name)? {
                affected_source_ids.insert(source_id);
            }
            for source_id in db::get_unresolved_source_ids_by_specifier_simple(&tx, name)? {
                affected_source_ids.insert(source_id);
            }
        }

        let mut calls = Vec::new();
        let mut seen_calls = FxHashSet::default();
        for source_id in &affected_source_ids {
            db::delete_edges_for_source_id(&tx, source_id)?;
            db::delete_unresolved_refs_for_source_id(&tx, source_id)?;
            for call in db::get_raw_calls_for_source_id(&tx, source_id)? {
                let key = (
                    call.caller_id.clone(),
                    call.callee_name.clone(),
                    call.line,
                    call.column,
                );
                if seen_calls.insert(key) {
                    calls.push(call);
                }
            }
        }
        calls
    };

    let (edges, unresolved) = parser::resolve_calls_global(&calls_to_resolve, &symbol_index);
    let edge_count = edges.len();

    // Edges reference existing nodes by construction; verify against the
    // in-memory id set rather than a SQL point query per endpoint (millions of
    // lookups on large repos).
    for edge in edges {
        if symbol_index.by_id.contains_key(&edge.source_id)
            && symbol_index.by_id.contains_key(&edge.target_id)
        {
            db::upsert_edge(&tx, &edge)?;
        }
    }
    for uref in &unresolved {
        db::insert_unresolved_ref(&tx, uref)?;
    }
    info!(
        "Resolved {} call edges from {} raw calls; {} unresolved references recorded.",
        edge_count,
        calls_to_resolve.len(),
        unresolved.len()
    );

    // Save Git baseline info & indexing timestamp
    let (git_sha, git_subject, git_date, git_branch, git_status) = get_git_info(workspace);
    db::upsert_project_metadata(&tx, "git_commit_sha", git_sha.as_deref())?;
    db::upsert_project_metadata(&tx, "git_commit_subject", git_subject.as_deref())?;
    db::upsert_project_metadata(&tx, "git_commit_date", git_date.as_deref())?;
    db::upsert_project_metadata(&tx, "git_branch", git_branch.as_deref())?;
    db::upsert_project_metadata(&tx, "git_status", git_status.as_deref())?;

    let indexed_at = std::process::Command::new("date")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "Unknown".to_string());
    db::upsert_project_metadata(&tx, "indexed_at", Some(&indexed_at))?;

    if is_fresh_build {
        db::rebuild_node_fts(&tx)?;
        db::create_node_fts_triggers(&tx)?;
    }

    tx.commit()?;

    // A fresh build runs with an in-memory journal and synchronous=OFF, which
    // leaves free-page slack and fragmentation behind. Compact it once at the
    // end so the on-disk file matches the logical size and pages pack densely
    // for better page-cache locality on subsequent queries.
    if is_fresh_build {
        info!("Compacting database (VACUUM)...");
        conn.execute("VACUUM", [])?;
    }

    info!("Indexing completed successfully.");
    info!(
        "  Commit SHA:  {}",
        git_sha.unwrap_or_else(|| "N/A".to_string())
    );
    info!(
        "  Commit Msg:  {}",
        git_subject.unwrap_or_else(|| "N/A".to_string())
    );
    info!(
        "  Commit Date: {}",
        git_date.unwrap_or_else(|| "N/A".to_string())
    );
    info!(
        "  Branch:      {}",
        git_branch.unwrap_or_else(|| "N/A".to_string())
    );
    info!(
        "  Git Status:  {}",
        git_status.unwrap_or_else(|| "N/A".to_string())
    );
    info!("  Indexed At:  {}", indexed_at);

    Ok(())
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
        "SELECT f.file_path, f.language, f.size_bytes, COUNT(n.id) \
         FROM files f \
         LEFT JOIN nodes n ON f.file_path = n.file_path \
         GROUP BY f.file_path \
         ORDER BY f.file_path",
    )?;

    let rows: Vec<(String, Option<String>, Option<i64>, i64)> = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;

    if json {
        let files: Vec<_> = rows
            .iter()
            .map(|(path, lang, size, count)| {
                json!({
                    "file_path": path,
                    "language": lang,
                    "size_bytes": size,
                    "symbols": count,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&files)?);
        return Ok(());
    }

    println!(
        "{:<40} {:<10} {:<10} {:<12}",
        "File Path", "Language", "Size (B)", "Symbols"
    );
    println!("{}", "-".repeat(76));

    for (path, lang, size, symbol_count) in rows {
        let lang_str = lang.unwrap_or_else(|| "unknown".to_string());
        let size_str = size
            .map(|s| s.to_string())
            .unwrap_or_else(|| "-".to_string());

        println!(
            "{:<40} {:<10} {:<10} {:<12}",
            path, lang_str, size_str, symbol_count
        );
    }

    Ok(())
}

fn query_nodes_by_like(conn: &Connection, pattern: &str) -> rusqlite::Result<Vec<db::Node>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, kind, qualified_name, file_path, start_line, end_line, start_column, end_column, signature, doc_comment \
         FROM nodes \
         WHERE name LIKE ? OR qualified_name LIKE ? OR id LIKE ?"
    )?;
    let mut rows = stmt.query([pattern, pattern, pattern])?;
    let mut nodes = Vec::new();
    while let Some(row) = rows.next()? {
        nodes.push(db::Node {
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
        });
    }
    Ok(nodes)
}

fn query_nodes_by_id_or_qual(conn: &Connection, symbol: &str) -> rusqlite::Result<Vec<db::Node>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, kind, qualified_name, file_path, start_line, end_line, start_column, end_column, signature, doc_comment \
         FROM nodes \
         WHERE id = ? OR qualified_name = ?"
    )?;
    let mut rows = stmt.query([symbol, symbol])?;
    let mut nodes = Vec::new();
    while let Some(row) = rows.next()? {
        nodes.push(db::Node {
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
        });
    }
    Ok(nodes)
}

/// Open the workspace database, returning a clear error if it has not been indexed.
fn open_db(workspace: &Path) -> Result<Connection, Box<dyn Error>> {
    let db_path = workspace.join(".ochna").join("ochna.db");
    if !db_path.exists() {
        return Err("ochna database not found. Run 'ochna init' to index the workspace.".into());
    }
    Ok(Connection::open(&db_path)?)
}

/// Emit a list of nodes: pretty JSON when `json`, otherwise one human line each.
fn emit_nodes(nodes: &[db::Node], json: bool, empty_msg: &str) -> Result<(), Box<dyn Error>> {
    if json {
        println!("{}", serde_json::to_string_pretty(nodes)?);
    } else if nodes.is_empty() {
        println!("{}", empty_msg);
    } else {
        for n in nodes {
            println!(
                "- {} ({}) - {}:{}",
                n.name, n.kind, n.file_path, n.start_line
            );
        }
    }
    Ok(())
}

/// Read a node's source lines as `{line, text}` records, or `None` if unreadable.
fn read_code_lines(workspace: &Path, node: &db::Node) -> Option<Vec<serde_json::Value>> {
    let abs_path = workspace.join(&node.file_path);
    let content = fs::read_to_string(&abs_path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len() as i64;
    let start = node.start_line.max(1);
    let end = node.end_line.min(total);
    Some(
        (start..=end)
            .filter(|&idx| idx <= total)
            .map(|idx| json!({ "line": idx, "text": lines[(idx - 1) as usize] }))
            .collect(),
    )
}

pub fn run_search(workspace: &Path, query: &str, json: bool) -> Result<(), Box<dyn Error>> {
    let conn = open_db(workspace)?;

    // Try FTS first
    let mut nodes = db::search_nodes_fts(&conn, query).unwrap_or_default();

    // If empty, fall back to exact name search
    if nodes.is_empty() {
        if let Ok(res) = db::query_nodes(&conn, Some(query), None, None) {
            nodes = res;
        }
    }

    // If still empty, try partial LIKE search on name
    if nodes.is_empty() {
        let query_pattern = format!("%{}%", query);
        if let Ok(res) = query_nodes_by_like(&conn, &query_pattern) {
            nodes = res;
        }
    }

    emit_nodes(&nodes, json, "No matching nodes found.")
}

pub fn run_callers(workspace: &Path, symbol: &str, json: bool) -> Result<(), Box<dyn Error>> {
    let conn = open_db(workspace)?;

    // Find nodes matching the symbol name or symbol ID
    let mut target_nodes = db::query_nodes(&conn, Some(symbol), None, None).unwrap_or_default();

    // If empty, try to find by qualified name or ID
    if target_nodes.is_empty() {
        if let Ok(res) = query_nodes_by_id_or_qual(&conn, symbol) {
            target_nodes = res;
        }
    }

    if target_nodes.is_empty() {
        return emit_nodes(
            &[],
            json,
            &format!("Symbol '{}' not found in database.", symbol),
        );
    }

    let mut callers = Vec::new();
    for node in target_nodes {
        if let Ok(node_callers) = db::find_callers(&conn, &node.id, None) {
            callers.extend(node_callers);
        }
    }

    // Deduplicate callers
    callers.sort_by(|a, b| a.id.cmp(&b.id));
    callers.dedup_by(|a, b| a.id == b.id);

    emit_nodes(&callers, json, "No callers found.")
}

#[allow(clippy::too_many_arguments)]
pub fn run_node(
    workspace: &Path,
    file: Option<String>,
    offset: Option<i64>,
    limit: Option<i64>,
    symbols_only: bool,
    symbol: Option<String>,
    include_code: bool,
    line: Option<i64>,
    json: bool,
) -> Result<(), Box<dyn Error>> {
    let conn = open_db(workspace)?;

    match (file, symbol) {
        (Some(file_path), None) => {
            // File mode
            let offset_val = offset.unwrap_or(1);
            let file_nodes = match db::query_nodes(&conn, None, None, Some(&file_path)) {
                Ok(res) => res,
                Err(e) => return Err(format!("Failed to query nodes for file: {}", e).into()),
            };

            if symbols_only {
                if json {
                    return emit_nodes(&file_nodes, true, "");
                }
                if file_nodes.is_empty() {
                    println!("No symbols found for file '{}'.", file_path);
                    return Ok(());
                }
                println!("Symbols in {}:", file_path);
                for n in file_nodes {
                    println!(
                        "- {} ({}) - lines {}-{}",
                        n.name, n.kind, n.start_line, n.end_line
                    );
                }
                return Ok(());
            }

            // Otherwise return file content sliced + dependents
            let abs_path = workspace.join(&file_path);
            let file_content = match fs::read_to_string(&abs_path) {
                Ok(content) => content,
                Err(e) => return Err(format!("Could not read file '{}': {}", file_path, e).into()),
            };

            let file_lines: Vec<&str> = file_content.lines().collect();
            let total_lines = file_lines.len() as i64;

            // Slicing lines (1-based index)
            let start = offset_val.max(1);
            let end = match limit {
                Some(lim) => (start + lim).min(total_lines),
                None => total_lines,
            };

            // Query dependents (external callers calling nodes defined in this file)
            let mut dependents = Vec::new();
            for node in &file_nodes {
                if let Ok(callers) = db::find_callers(&conn, &node.id, None) {
                    for caller in callers {
                        if caller.file_path != file_path {
                            dependents.push(caller);
                        }
                    }
                }
            }
            dependents.sort_by(|a, b| a.id.cmp(&b.id));
            dependents.dedup_by(|a, b| a.id == b.id);

            if json {
                let lines: Vec<_> = (start..=end)
                    .filter(|&idx| idx <= total_lines)
                    .map(|idx| json!({ "line": idx, "text": file_lines[(idx - 1) as usize] }))
                    .collect();
                let out = json!({
                    "file_path": file_path,
                    "start_line": start,
                    "end_line": end,
                    "lines": lines,
                    "dependents": dependents,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
                return Ok(());
            }

            println!(
                "File content of {} (lines {} to {}):",
                file_path, start, end
            );
            for idx in start..=end {
                if idx <= total_lines {
                    println!("{}\t{}", idx, file_lines[(idx - 1) as usize]);
                }
            }

            println!("\nDependents:");
            if dependents.is_empty() {
                println!("No external dependents.");
            } else {
                for dep in dependents {
                    println!(
                        "- {} ({}) - {}:{}",
                        dep.name, dep.kind, dep.file_path, dep.start_line
                    );
                }
            }
        }
        (None, Some(symbol_name)) => {
            // Symbol mode
            let mut target_nodes =
                db::query_nodes(&conn, Some(&symbol_name), None, None).unwrap_or_default();
            if target_nodes.is_empty() {
                if let Ok(res) = query_nodes_by_id_or_qual(&conn, &symbol_name) {
                    target_nodes = res;
                }
            }

            if let Some(target_line) = line {
                target_nodes.retain(|n| target_line >= n.start_line && target_line <= n.end_line);
            }

            if target_nodes.is_empty() {
                if json {
                    println!("[]");
                    return Ok(());
                }
                println!("Symbol '{}' not found in database.", symbol_name);
                return Ok(());
            }

            if json {
                let mut out = Vec::new();
                for node in &target_nodes {
                    let code = if include_code {
                        read_code_lines(workspace, node)
                    } else {
                        None
                    };
                    let mut callers = db::find_callers(&conn, &node.id, None).unwrap_or_default();
                    callers.sort_by(|a, b| a.id.cmp(&b.id));
                    callers.dedup_by(|a, b| a.id == b.id);
                    let mut callees = db::find_callees(&conn, &node.id, None).unwrap_or_default();
                    callees.sort_by(|a, b| a.id.cmp(&b.id));
                    callees.dedup_by(|a, b| a.id == b.id);
                    out.push(json!({
                        "symbol": node,
                        "code": code,
                        "callers": callers,
                        "callees": callees,
                    }));
                }
                println!("{}", serde_json::to_string_pretty(&out)?);
                return Ok(());
            }

            let mut results = Vec::new();
            for node in target_nodes {
                let mut section = Vec::new();
                section.push(format!("Symbol: {} ({})", node.name, node.kind));
                section.push(format!(
                    "Defined in: {} (lines {}-{})",
                    node.file_path, node.start_line, node.end_line
                ));
                if let Some(sig) = &node.signature {
                    section.push(format!("Signature: {}", sig));
                }
                if let Some(doc) = &node.doc_comment {
                    section.push(format!("Documentation:\n{}", doc));
                }

                if include_code {
                    let abs_path = workspace.join(&node.file_path);
                    if let Ok(file_content) = fs::read_to_string(&abs_path) {
                        let file_lines: Vec<&str> = file_content.lines().collect();
                        let total_lines = file_lines.len() as i64;
                        let start = node.start_line.max(1);
                        let end = node.end_line.min(total_lines);
                        section.push("\nCode:".to_string());
                        for idx in start..=end {
                            if idx <= total_lines {
                                section.push(format!(
                                    "{}\t{}",
                                    idx,
                                    file_lines[(idx - 1) as usize]
                                ));
                            }
                        }
                    } else {
                        section.push(format!(
                            "\nCode: [Could not read file '{}']",
                            node.file_path
                        ));
                    }
                }

                // Callers & Callees
                let mut callers = db::find_callers(&conn, &node.id, None).unwrap_or_default();
                callers.sort_by(|a, b| a.id.cmp(&b.id));
                callers.dedup_by(|a, b| a.id == b.id);

                let mut callees = db::find_callees(&conn, &node.id, None).unwrap_or_default();
                callees.sort_by(|a, b| a.id.cmp(&b.id));
                callees.dedup_by(|a, b| a.id == b.id);

                section.push("\nCallers:".to_string());
                if callers.is_empty() {
                    section.push("None".to_string());
                } else {
                    for caller in callers {
                        section.push(format!(
                            "- {} ({}) - {}:{}",
                            caller.name, caller.kind, caller.file_path, caller.start_line
                        ));
                    }
                }

                section.push("\nCallees:".to_string());
                if callees.is_empty() {
                    section.push("None".to_string());
                } else {
                    for callee in callees {
                        section.push(format!(
                            "- {} ({}) - {}:{}",
                            callee.name, callee.kind, callee.file_path, callee.start_line
                        ));
                    }
                }

                results.push(section.join("\n"));
            }

            println!("{}", results.join("\n\n---\n\n"));
        }
        _ => {
            return Err("specify exactly one of '--file' or '--symbol'.".into());
        }
    }

    Ok(())
}

pub fn run_explore(workspace: &Path, query: &str, json: bool) -> Result<(), Box<dyn Error>> {
    let conn = open_db(workspace)?;

    // Find matching nodes (using search logic)
    let mut nodes = db::search_nodes_fts(&conn, query).unwrap_or_default();
    if nodes.is_empty() {
        if let Ok(res) = db::query_nodes(&conn, Some(query), None, None) {
            nodes = res;
        }
    }
    if nodes.is_empty() {
        let query_pattern = format!("%{}%", query);
        if let Ok(res) = query_nodes_by_like(&conn, &query_pattern) {
            nodes = res;
        }
    }

    if nodes.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No matching nodes found to explore.");
        }
        return Ok(());
    }

    // Group by file
    use std::collections::HashMap;
    let mut files_to_nodes: HashMap<String, Vec<db::Node>> = HashMap::new();
    for n in nodes {
        files_to_nodes
            .entry(n.file_path.clone())
            .or_default()
            .push(n);
    }

    if json {
        let mut out = Vec::new();
        for (file_path, file_nodes) in &files_to_nodes {
            let mut symbols = Vec::new();
            for node in file_nodes {
                let mut callers = db::find_callers(&conn, &node.id, None).unwrap_or_default();
                callers.sort_by(|a, b| a.id.cmp(&b.id));
                callers.dedup_by(|a, b| a.id == b.id);
                let mut callees = db::find_callees(&conn, &node.id, None).unwrap_or_default();
                callees.sort_by(|a, b| a.id.cmp(&b.id));
                callees.dedup_by(|a, b| a.id == b.id);
                symbols.push(json!({
                    "symbol": node,
                    "code": read_code_lines(workspace, node),
                    "callers": callers,
                    "callees": callees,
                }));
            }
            out.push(json!({ "file_path": file_path, "symbols": symbols }));
        }
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    let mut output = Vec::new();
    for (file_path, file_nodes) in files_to_nodes {
        output.push(format!("File: {}", file_path));

        let abs_path = workspace.join(&file_path);
        let file_lines = fs::read_to_string(&abs_path).ok().map(|content| {
            content
                .lines()
                .map(|s| s.to_string())
                .collect::<Vec<String>>()
        });

        for node in file_nodes {
            output.push(format!("  Symbol: {} ({})", node.name, node.kind));
            output.push(format!("  Lines: {} to {}", node.start_line, node.end_line));

            if let Some(ref lines) = file_lines {
                output.push("  Code:".to_string());
                let start = node.start_line.max(1);
                let end = node.end_line.min(lines.len() as i64);
                for idx in start..=end {
                    output.push(format!("    {}\t{}", idx, lines[(idx - 1) as usize]));
                }
            }

            // Query callers/callees
            let mut callers = db::find_callers(&conn, &node.id, None).unwrap_or_default();
            callers.sort_by(|a, b| a.id.cmp(&b.id));
            callers.dedup_by(|a, b| a.id == b.id);

            let mut callees = db::find_callees(&conn, &node.id, None).unwrap_or_default();
            callees.sort_by(|a, b| a.id.cmp(&b.id));
            callees.dedup_by(|a, b| a.id == b.id);

            output.push("  Relationships:".to_string());
            if callers.is_empty() {
                output.push("    Callers: None".to_string());
            } else {
                output.push("    Callers:".to_string());
                for c in callers {
                    output.push(format!(
                        "      - {} ({}) - {}:{}",
                        c.name, c.kind, c.file_path, c.start_line
                    ));
                }
            }

            if callees.is_empty() {
                output.push("    Callees: None".to_string());
            } else {
                output.push("    Callees:".to_string());
                for c in callees {
                    output.push(format!(
                        "      - {} ({}) - {}:{}",
                        c.name, c.kind, c.file_path, c.start_line
                    ));
                }
            }
        }
        output.push(String::new());
    }

    println!("{}", output.join("\n"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
