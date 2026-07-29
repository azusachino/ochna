//! Read-only Git-to-index changed-symbol attribution for structural review.

use crate::db;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const MAX_CHANGED_FILES: usize = 500;

#[derive(Clone, Debug)]
struct ChangedFile {
    path: String,
    status: &'static str,
}

#[derive(Clone, Debug)]
struct Hunk {
    path: String,
    old_start: i64,
    new_start: i64,
    new_count: i64,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct EdgeKey {
    source_id: String,
    target_id: String,
    relationship: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EdgeSnapshot {
    key: EdgeKey,
    resolution_kind: String,
    confidence: i64,
}

#[derive(Clone, Default)]
struct GraphSnapshot {
    nodes: BTreeMap<String, db::Node>,
    edges: BTreeMap<EdgeKey, EdgeSnapshot>,
    unresolved: BTreeSet<(String, String, String)>,
}

fn nodes_for_path(graph: &GraphSnapshot, path: &str) -> Vec<db::Node> {
    graph
        .nodes
        .values()
        .filter(|node| node.file_path == path)
        .cloned()
        .collect()
}

struct HistoricalSnapshot {
    graph: GraphSnapshot,
    temporary_bytes: u64,
    elapsed_ms: u128,
}

/// A private, throw-away archive extraction. It never creates a Git worktree
/// or writes into the user's workspace; Drop removes the entire historical
/// source/index once the comparison has been rendered.
struct TemporaryWorkspace(PathBuf);

impl TemporaryWorkspace {
    fn new() -> Result<Self, Box<dyn Error>> {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ochna-historical-{}-{}-{}",
            std::process::id(),
            nonce,
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TemporaryWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git(workspace: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?
        .trim_end_matches('\n')
        .to_string())
}

fn resolve_revision(workspace: &Path, revision: &str) -> Result<String, Box<dyn Error>> {
    git(
        workspace,
        &["rev-parse", "--verify", "--end-of-options", revision],
    )
}

fn git_range(base: &str, head: Option<&str>) -> Vec<String> {
    match head {
        Some(head) => vec![base.to_string(), head.to_string()],
        None => vec![base.to_string()],
    }
}

fn changed_files(
    workspace: &Path,
    base: &str,
    head: Option<&str>,
) -> Result<Vec<ChangedFile>, Box<dyn Error>> {
    let mut args = vec!["diff", "--name-status", "-M"];
    let range = git_range(base, head);
    args.push("--end-of-options");
    args.extend(range.iter().map(String::as_str));
    let output = git(workspace, &args)?;
    let mut files = Vec::new();
    for line in output.lines() {
        let mut fields = line.split('\t');
        let code = fields.next().unwrap_or_default();
        let first = fields.next().unwrap_or_default();
        let path = if code.starts_with('R') {
            fields.next().unwrap_or(first)
        } else {
            first
        };
        let status = match code.chars().next() {
            Some('A') => "added",
            Some('D') => "deleted",
            Some('R') => "renamed",
            _ => "modified",
        };
        files.push(ChangedFile {
            path: path.to_string(),
            status,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn patch(workspace: &Path, base: &str, head: Option<&str>) -> Result<String, Box<dyn Error>> {
    let mut args = vec!["diff", "--unified=0", "--no-ext-diff", "-M"];
    let range = git_range(base, head);
    args.push("--end-of-options");
    args.extend(range.iter().map(String::as_str));
    git(workspace, &args)
}

fn parse_hunks(patch: &str) -> Vec<Hunk> {
    let mut old_path: Option<String> = None;
    let mut new_path: Option<String> = None;
    let mut current: Option<Hunk> = None;
    let mut result = Vec::new();
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("--- a/") {
            old_path = Some(rest.to_string());
            continue;
        }
        if line == "--- /dev/null" {
            old_path = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("+++ b/") {
            new_path = Some(rest.to_string());
            continue;
        }
        if line == "+++ /dev/null" {
            new_path = None;
            continue;
        }
        if line.starts_with("@@ ") {
            if let Some(hunk) = current.take() {
                result.push(hunk);
            }
            let parts: Vec<_> = line.split_whitespace().collect();
            let parse_range = |value: &str| -> (i64, i64) {
                let value = value.trim_start_matches(['-', '+']);
                let (start, count) = value.split_once(',').unwrap_or((value, "1"));
                (start.parse().unwrap_or(0), count.parse().unwrap_or(1))
            };
            if let (Some(old), Some(new), Some(path)) = (
                parts.get(1),
                parts.get(2),
                new_path.as_ref().or(old_path.as_ref()),
            ) {
                let (old_start, _) = parse_range(old);
                let (new_start, new_count) = parse_range(new);
                current = Some(Hunk {
                    path: path.clone(),
                    old_start,
                    new_start,
                    new_count,
                });
            }
        }
    }
    if let Some(hunk) = current {
        result.push(hunk);
    }
    result
}

fn cap_changed_files(files: &mut Vec<ChangedFile>) -> bool {
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files.dedup_by(|left, right| left.path == right.path);
    let truncated = files.len() > MAX_CHANGED_FILES;
    files.truncate(MAX_CHANGED_FILES);
    truncated
}

fn unresolved_for_new_lines(
    conn: &Connection,
    source_ids: &BTreeSet<String>,
    new_lines: &BTreeSet<(String, i64)>,
) -> rusqlite::Result<Vec<Value>> {
    let mut stmt = conn.prepare("SELECT n.id, n.file_path, u.specifier, u.kind, u.line FROM unresolved_refs u JOIN nodes n ON n.nid = u.source_nid ORDER BY n.id, u.line, u.column, u.specifier")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut result = Vec::new();
    for row in rows {
        let (source, path, specifier, kind, line) = row?;
        if source_ids.contains(&source) && new_lines.contains(&(path, line)) {
            let simple = specifier.rsplit("::").next().unwrap_or(&specifier);
            let candidates: i64 = conn.query_row(
                "SELECT COUNT(*) FROM nodes WHERE name = ?",
                [simple],
                |row| row.get(0),
            )?;
            let reason = if matches!(kind.as_str(), "macro_or_function" | "indirect_call") {
                kind
            } else if candidates > 0 {
                "ambiguous_target".to_string()
            } else {
                "missing_target".to_string()
            };
            result.push(json!({"source": source, "specifier": specifier, "reason": reason}));
        }
    }
    Ok(result)
}

fn unresolved_snapshot(conn: &Connection) -> rusqlite::Result<BTreeSet<(String, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT n.id, u.specifier, u.kind FROM unresolved_refs u JOIN nodes n ON n.nid = u.source_nid ORDER BY n.id, u.specifier, u.kind",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut unresolved = BTreeSet::new();
    for row in rows {
        let (source, specifier, kind) = row?;
        let simple = specifier.rsplit("::").next().unwrap_or(&specifier);
        let candidates: i64 = conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE name = ?",
            [simple],
            |row| row.get(0),
        )?;
        let reason = if matches!(kind.as_str(), "macro_or_function" | "indirect_call") {
            kind
        } else if candidates > 0 {
            "ambiguous_target".to_string()
        } else {
            "missing_target".to_string()
        };
        unresolved.insert((source, specifier, reason));
    }
    Ok(unresolved)
}

fn graph_snapshot(conn: &Connection) -> rusqlite::Result<GraphSnapshot> {
    let nodes = db::query_nodes(conn, None, None, None)?
        .into_iter()
        .map(|node| (node.id.clone(), node))
        .collect();
    let mut stmt = conn.prepare(
        "SELECT source.id, target.id, e.kind, e.resolution_kind
         FROM edges e
         JOIN nodes source ON source.nid = e.source_nid
         JOIN nodes target ON target.nid = e.target_nid
         ORDER BY source.id, target.id, e.kind",
    )?;
    let rows = stmt.query_map([], |row| {
        let resolution_kind: i64 = row.get(3)?;
        Ok(EdgeSnapshot {
            key: EdgeKey {
                source_id: row.get(0)?,
                target_id: row.get(1)?,
                relationship: row.get(2)?,
            },
            resolution_kind: db::label_for_kind(resolution_kind).to_string(),
            confidence: db::confidence_for_kind(resolution_kind),
        })
    })?;
    let mut edges = BTreeMap::new();
    for edge in rows {
        let edge = edge?;
        edges.insert(edge.key.clone(), edge);
    }
    Ok(GraphSnapshot {
        nodes,
        edges,
        unresolved: unresolved_snapshot(conn)?,
    })
}

fn directory_size(path: &Path) -> std::io::Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total += directory_size(&entry.path())?;
        } else if metadata.is_file() {
            total += metadata.len();
        }
    }
    Ok(total)
}

fn current_index_is_fresh(workspace: &Path, conn: &Connection) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare("SELECT file_path, content_hash FROM files ORDER BY file_path")?;
    let indexed = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    let discovered = super::discover_source_files(workspace)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    if discovered.len() != indexed.len() {
        return Ok(false);
    }
    for path in discovered {
        let path_string = path.to_string_lossy().replace('\\', "/");
        let Some(expected) = indexed.get(&path_string) else {
            return Ok(false);
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        fs::read_to_string(workspace.join(path))
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?
            .hash(&mut hasher);
        if format!("{:x}", hasher.finish()) != *expected {
            return Ok(false);
        }
    }
    Ok(true)
}

fn archive_revision(
    workspace: &Path,
    revision: &str,
    destination: &Path,
) -> Result<(), Box<dyn Error>> {
    let mut archive = Command::new("git")
        .args(["archive", "--format=tar", "--", revision])
        .current_dir(workspace)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let archive_stdout = archive
        .stdout
        .take()
        .ok_or("could not read git archive output")?;
    let unpack = Command::new("tar")
        .args(["-x", "-C"])
        .arg(destination)
        .stdin(archive_stdout)
        .output()?;
    let archive_output = archive.wait_with_output()?;
    if !archive_output.status.success() || !unpack.status.success() {
        return Err(format!(
            "could not archive historical revision {revision}: {}{}",
            String::from_utf8_lossy(&archive_output.stderr).trim(),
            String::from_utf8_lossy(&unpack.stderr).trim()
        )
        .into());
    }
    Ok(())
}

fn historical_snapshot(
    workspace: &Path,
    revision: &str,
) -> Result<HistoricalSnapshot, Box<dyn Error>> {
    let started = Instant::now();
    let temporary = TemporaryWorkspace::new()?;
    archive_revision(workspace, revision, &temporary.0)?;
    super::index::run_init(&temporary.0, false)?;
    let conn = Connection::open(temporary.0.join(".ochna/ochna.db"))?;
    let graph = graph_snapshot(&conn)?;
    let temporary_bytes = directory_size(&temporary.0)?;
    // `temporary` is deliberately dropped here, after all source and DB reads
    // are complete and before this function returns any data to the caller.
    drop(conn);
    drop(temporary);
    Ok(HistoricalSnapshot {
        graph,
        temporary_bytes,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

fn edge_value(edge: &EdgeSnapshot, nodes: &BTreeMap<String, db::Node>, change: &str) -> Value {
    json!({
        "source": nodes.get(&edge.key.source_id),
        "target": nodes.get(&edge.key.target_id),
        "relationship": edge.key.relationship,
        "resolution_kind": edge.resolution_kind,
        "confidence": edge.confidence,
        "change": change,
    })
}

pub fn run_diff(
    workspace: &Path,
    base: Option<&str>,
    head: Option<&str>,
    explicit_files: &[String],
    limit: usize,
    json_mode: bool,
) -> Result<(), Box<dyn Error>> {
    for file in explicit_files {
        let path = Path::new(file);
        if path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(format!("--files path must be workspace-relative: {file}").into());
        }
    }
    let explicit_file_count = explicit_files.iter().collect::<BTreeSet<_>>().len();
    if explicit_file_count > MAX_CHANGED_FILES {
        return Err(format!(
            "--files accepts at most {MAX_CHANGED_FILES} distinct workspace-relative paths"
        )
        .into());
    }
    let db_path = workspace.join(".ochna/ochna.db");
    if !db_path.exists() {
        return Err("Database not initialized. Run the 'init' command first.".into());
    }
    let conn = Connection::open(db_path)?;
    let (base_out, head_out, files, mut hunks, files_truncated) = if let Some(base) = base {
        let base_out = match resolve_revision(workspace, base) {
            Ok(revision) => revision,
            Err(error) => {
                let message = format!(
                    "Base revision {base:?} is unavailable locally ({error}). Fetch it before requesting historical deltas; shallow clones may need git fetch --deepen."
                );
                if json_mode {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&json!({
                            "contract_version": "0.3",
                            "command": "diff",
                            "ok": false,
                            "data": {"base": Value::Null, "head": Value::Null, "files": [], "symbols": [], "edges": [], "newly_unresolved_callers": [], "unmapped_hunks": [], "historical_snapshot": Value::Null},
                            "warnings": [{"code": "base_revision_unavailable", "message": message}],
                            "truncated": false,
                            "next_action": "provide a base revision"
                        }))?
                    );
                    return Ok(());
                }
                return Err(message.into());
            }
        };
        let head_out = head
            .map(|revision| resolve_revision(workspace, revision))
            .transpose()?;
        let mut files = changed_files(workspace, base, head)?;
        let files_truncated = cap_changed_files(&mut files);
        (
            Some(base_out),
            head_out,
            files,
            parse_hunks(&patch(workspace, base, head)?),
            files_truncated,
        )
    } else {
        let mut files: Vec<_> = explicit_files
            .iter()
            .map(|path| ChangedFile {
                path: path.clone(),
                status: if workspace.join(path).is_file() {
                    "modified"
                } else {
                    "deleted"
                },
            })
            .collect();
        cap_changed_files(&mut files);
        (None, None, files, Vec::new(), false)
    };
    let mut warnings = Vec::new();
    let current_graph = graph_snapshot(&conn)?;
    let target_history = if let Some(head_out) = head_out.as_deref() {
        match historical_snapshot(workspace, head_out) {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                warnings.push(json!({
                    "code": "historical_snapshot_unavailable",
                    "message": format!("The requested --head snapshot is unavailable ({error}). Fetch the revision locally (for a shallow clone, git fetch --deepen) and retry.")
                }));
                None
            }
        }
    } else {
        None
    };
    let target_graph = target_history
        .as_ref()
        .map(|snapshot| snapshot.graph.clone())
        .unwrap_or_else(|| current_graph.clone());
    if base.is_some() && head.is_none() && !current_index_is_fresh(workspace, &conn)? {
        warnings.push(json!({
            "code": "current_index_stale",
            "message": "The worktree-side index is stale, so historical deltas may not describe the current source. Run ochna sync and retry."
        }));
    }
    let retained_paths: BTreeSet<_> = files.iter().map(|file| file.path.as_str()).collect();
    hunks.retain(|hunk| retained_paths.contains(hunk.path.as_str()));
    let status_by_path: BTreeMap<_, _> = files
        .iter()
        .map(|file| (file.path.as_str(), file.status))
        .collect();
    let mut mapped: BTreeMap<String, (db::Node, &'static str, BTreeSet<i64>)> = BTreeMap::new();
    let mut unmapped = BTreeSet::new();
    let mut removed_hunks = Vec::new();
    let mut new_lines = BTreeSet::new();
    for hunk in &hunks {
        if hunk.new_count > 0 {
            let range_end = hunk.new_start + hunk.new_count - 1;
            let mut matching = false;
            for node in nodes_for_path(&target_graph, &hunk.path) {
                if node.start_line <= range_end && hunk.new_start <= node.end_line {
                    matching = true;
                    let change = if status_by_path.get(hunk.path.as_str()) == Some(&"added") {
                        "added"
                    } else {
                        "modified"
                    };
                    mapped
                        .entry(node.id.clone())
                        .or_insert_with(|| (node, change, BTreeSet::new()))
                        .2
                        .insert(hunk.new_start);
                }
            }
            for line in hunk.new_start..=range_end {
                new_lines.insert((hunk.path.clone(), line));
            }
            if !matching {
                unmapped.insert((hunk.path.clone(), hunk.new_start));
            }
        }
        if hunk.new_count == 0 {
            removed_hunks.push(hunk);
        }
    }
    if base.is_none() {
        for file in &files {
            for node in nodes_for_path(&target_graph, &file.path) {
                let change = if file.status == "added" {
                    "added"
                } else {
                    "modified"
                };
                mapped
                    .entry(node.id.clone())
                    .or_insert_with(|| (node, change, BTreeSet::new()));
            }
        }
    } else {
        // A pure rename has no content hunks. Its current indexed symbols are
        // still changed-path evidence and must not disappear from the review.
        for file in files.iter().filter(|file| file.status == "renamed") {
            for node in nodes_for_path(&target_graph, &file.path) {
                mapped
                    .entry(node.id.clone())
                    .or_insert_with(|| (node, "modified", BTreeSet::new()));
            }
        }
    }
    let mut historical_symbols = Vec::<Value>::new();
    let mut edges = Vec::<Value>::new();
    let deleted_explicit = base.is_none() && files.iter().any(|file| file.status == "deleted");
    let historical = if head.is_some() && target_history.is_none() {
        None
    } else if let (Some(_), Some(base_out)) = (base, base_out.as_deref()) {
        match historical_snapshot(workspace, base_out) {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                warnings.push(json!({
                    "code": "historical_snapshot_unavailable",
                    "message": format!("Historical deltas are unavailable ({error}). The current-index mapping is preserved; fetch the base object locally (for a shallow clone, git fetch --deepen) and retry.")
                }));
                None
            }
        }
    } else {
        None
    };
    let mut historical_metrics = Value::Null;
    let unresolved = if let Some(historical) = historical.as_ref() {
        historical_metrics = json!({
            "base_index": "temporary",
            "head_index": if target_history.is_some() { "temporary" } else { "current" },
            "temporary_bytes": historical.temporary_bytes,
            "elapsed_ms": historical.elapsed_ms,
            "cleanup": "removed"
        });
        for (id, node) in &target_graph.nodes {
            match historical.graph.nodes.get(id) {
                None => {
                    mapped.insert(id.clone(), (node.clone(), "added", BTreeSet::new()));
                }
                Some(before) if before != node => {
                    mapped.insert(id.clone(), (node.clone(), "modified", BTreeSet::new()));
                }
                _ => {}
            }
        }
        for (id, node) in &historical.graph.nodes {
            if !target_graph.nodes.contains_key(id) {
                historical_symbols.push(json!({"symbol": node, "change": "removed", "hunks": []}));
            }
        }
        for (key, edge) in &target_graph.edges {
            match historical.graph.edges.get(key) {
                None => edges.push(edge_value(edge, &target_graph.nodes, "added")),
                Some(before) if before != edge => {
                    edges.push(edge_value(edge, &target_graph.nodes, "modified"))
                }
                _ => {}
            }
        }
        for (key, edge) in &historical.graph.edges {
            if !target_graph.edges.contains_key(key) {
                edges.push(edge_value(edge, &historical.graph.nodes, "removed"));
            }
        }
        target_graph
            .unresolved
            .difference(&historical.graph.unresolved)
            .map(|(source, specifier, reason)| {
                json!({"source": source, "specifier": specifier, "reason": reason})
            })
            .collect()
    } else {
        unresolved_for_new_lines(&conn, &mapped.keys().cloned().collect(), &new_lines)?
    };
    if historical.is_none() && (!removed_hunks.is_empty() || deleted_explicit) {
        warnings.push(json!({"code":"historical_index_required", "message":"Removed symbols and edges require a historical index; unavailable endpoints are null."}));
        for hunk in removed_hunks {
            historical_symbols
                .push(json!({"symbol": Value::Null, "change":"removed", "hunks":[hunk.old_start]}));
        }
        if deleted_explicit {
            for file in files.iter().filter(|file| file.status == "deleted") {
                historical_symbols.push(json!({"symbol": Value::Null, "change":"removed", "hunks":[], "path":file.path}));
            }
        }
    }
    let mut symbols: Vec<Value> = mapped
        .values()
        .map(|(node, change, lines)| json!({"symbol": node, "change": change, "hunks": lines}))
        .collect();
    symbols.append(&mut historical_symbols);
    symbols.sort_by(|left, right| {
        left["symbol"]["id"]
            .as_str()
            .unwrap_or("")
            .cmp(right["symbol"]["id"].as_str().unwrap_or(""))
    });
    edges.sort_by(|left, right| {
        (
            left["source"]["id"].as_str().unwrap_or(""),
            left["target"]["id"].as_str().unwrap_or(""),
            left["relationship"].as_str().unwrap_or(""),
        )
            .cmp(&(
                right["source"]["id"].as_str().unwrap_or(""),
                right["target"]["id"].as_str().unwrap_or(""),
                right["relationship"].as_str().unwrap_or(""),
            ))
    });
    let mut truncated = files_truncated || symbols.len() > limit || edges.len() > limit;
    if symbols.len() > limit {
        symbols.truncate(limit);
    }
    if edges.len() > limit {
        edges.truncate(limit);
    }
    let mut unmapped_hunks: Vec<_> = unmapped
        .into_iter()
        .map(|(path, line)| json!({"path":path,"line":line}))
        .collect();
    if unmapped_hunks.len() > MAX_CHANGED_FILES {
        unmapped_hunks.truncate(MAX_CHANGED_FILES);
        truncated = true;
    }
    let data = json!({"base":base_out,"head":head_out,"files":files.iter().map(|f| json!({"path":f.path,"status":f.status})).collect::<Vec<_>>(),"symbols":symbols,"edges":edges,"newly_unresolved_callers":unresolved,"unmapped_hunks":unmapped_hunks,"historical_snapshot":historical_metrics});
    if json_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"contract_version":"0.3","command":"diff","ok":true,"data":data,"warnings":warnings,"truncated":truncated,"next_action":if truncated {"narrow the query"} else {"none"}})
            )?
        );
    } else {
        println!("Changed files:");
        for file in &files {
            println!("{} {}", file.status, file.path);
        }
        println!("Changed symbols:");
        if symbols.is_empty() {
            println!("none");
        } else {
            for symbol in &symbols {
                println!(
                    "{} {}",
                    symbol["change"],
                    symbol["symbol"]["qualified_name"]
                        .as_str()
                        .or(symbol["symbol"]["id"].as_str())
                        .unwrap_or("unavailable")
                );
            }
        }
        println!("Changed edges:");
        if edges.is_empty() {
            println!("none");
        } else {
            for edge in &edges {
                println!("{} {}", edge["change"], edge["relationship"]);
            }
        }
        println!("Newly unresolved callers:");
        if unresolved.is_empty() {
            println!("none");
        } else {
            for item in &unresolved {
                println!(
                    "{} -> {} ({})",
                    item["source"], item["specifier"], item["reason"]
                );
            }
        }
        println!("Unmapped hunks:");
        if data["unmapped_hunks"].as_array().is_none_or(Vec::is_empty) {
            println!("none");
        } else {
            for item in data["unmapped_hunks"].as_array().unwrap() {
                println!("{}:{}", item["path"], item["line"]);
            }
        }
        if !warnings.is_empty() {
            println!("Warnings:");
            for warning in warnings {
                println!("{}: {}", warning["code"], warning["message"]);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{cap_changed_files, parse_hunks, ChangedFile, MAX_CHANGED_FILES};

    #[test]
    fn parses_new_ranges_and_preserves_deleted_file_paths() {
        let hunks = parse_hunks(
            "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,7 +1,3 @@\n-pub fn legacy_render(input: &str) -> String {\n-    format!(\"legacy:{input}\")\n-}\n-\n pub fn render(input: &str) -> String {\n-    legacy_render(input)\n+    format!(\"rendered:{input}\")\n }\n",
        );
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].path, "src/lib.rs");
        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].new_count, 3);

        let deleted = parse_hunks("diff --git a/src/old.rs b/src/old.rs\n--- a/src/old.rs\n+++ /dev/null\n@@ -2,2 +0,0 @@\n-fn old() {}\n-\n");
        assert_eq!(deleted[0].path, "src/old.rs");
        assert_eq!(deleted[0].new_count, 0);
    }

    #[test]
    fn caps_and_deduplicates_changed_files_in_path_order() {
        let mut files = (0..=MAX_CHANGED_FILES)
            .rev()
            .map(|number| ChangedFile {
                path: format!("src/{number:03}.rs"),
                status: "modified",
            })
            .collect::<Vec<_>>();
        files.push(ChangedFile {
            path: "src/000.rs".to_string(),
            status: "modified",
        });
        assert!(cap_changed_files(&mut files));
        assert_eq!(files.len(), MAX_CHANGED_FILES);
        assert_eq!(files.first().unwrap().path, "src/000.rs");
        assert_eq!(files.last().unwrap().path, "src/499.rs");
    }
}
