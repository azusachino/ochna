//! The `init`/`sync` indexing pipeline: scan, parse, and resolve call edges.

use crate::db;
use crate::parser;
use rayon::prelude::*;
use rusqlite::Connection;
use rustc_hash::{FxHashMap, FxHashSet};
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
