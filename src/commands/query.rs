//! Query commands: `search`, `callers`, `node`, and `explore`, plus the shared
//! node-lookup and emission helpers they build on.

use crate::db;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::json;
use std::error::Error;
use std::fs;
use std::path::Path;

const HOWTO_TEXT: &str = r#"ochna usage flow

1. Run `ochna status` first. If the index is stale, run `ochna sync` (or `ochna init` to build it).
2. Search for an entry point with `ochna search <name>`.
3. Trace incoming references with `ochna callers <name>`.
4. Inspect a definition with `ochna node --symbol <name> --include-code`.
5. Use `ochna explore <query>` when you want search results, snippets, and graph context together.

Use ochna BEFORE recursive grep/read: prefer `search`/`callers`/`node` over `rg` for symbol lookups.

Inspecting nodes

- `ochna node --file <path> --symbols-only` — list the symbols defined in a file.
- `ochna node --file <path> --offset <line> --limit <n>` — slice source by line range.
- `ochna node --symbol <name> --include-code [--line <n>]` — definition plus source; `--line` disambiguates overloads.

Call-edge confidence

- Add `--show-resolution` to any query to print each edge's resolution kind and confidence.
- Add `--min-confidence <N>` to `callers` to drop weak edges. Cascade: exact 100, receiver_type 90, package/namespace 80, same_file 60, name_only 30. Use `--min-confidence 80` to cut noise on common method names.

Operational facts

- The database is resolved from the current working directory at `.ochna/ochna.db`.
- There is no `--workspace` flag; `cd` into the project or submodule before querying.
- Add global `--json` for machine-readable stdout; add global `--no-tests` to hide symbols classified from test paths.
- `init`/`sync` skip library/generated dirs (target, node_modules, .venv, vendor, build, dist) by default; pass `--include-library` to index them.
- Diagnostics and progress go to stderr; JSON stdout is kept parseable.
"#;

#[derive(Serialize)]
struct HowtoDescriptor<'a> {
    flow: [&'a str; 5],
    rule: &'a str,
    commands: HowtoCommands<'a>,
    node_modes: [&'a str; 3],
    flags: HowtoFlags<'a>,
    confidence_cascade: [&'a str; 5],
    globals: [&'a str; 2],
    notes: [&'a str; 3],
}

#[derive(Serialize)]
struct HowtoCommands<'a> {
    status: &'a str,
    search: &'a str,
    callers: &'a str,
    node: &'a str,
    explore: &'a str,
    files: &'a str,
    init: &'a str,
    sync: &'a str,
}

#[derive(Serialize)]
struct HowtoFlags<'a> {
    show_resolution: &'a str,
    min_confidence: &'a str,
    include_library: &'a str,
    no_tests: &'a str,
    json: &'a str,
}

pub fn run_howto(json: bool) -> Result<(), Box<dyn Error>> {
    if json {
        let descriptor = HowtoDescriptor {
            flow: ["status", "search", "callers", "node", "explore"],
            rule: "use ochna before recursive grep/read; prefer search/callers/node over rg for symbol lookups",
            commands: HowtoCommands {
                status: "check whether the local index exists, matches the binary schema, and is fresh enough to trust",
                search: "fuzzy and full-text symbol lookup",
                callers: "reverse call-edge lookup for a symbol",
                node: "inspect a file, symbol metadata, and optionally source code",
                explore: "combined search, snippets, callers, and callees view",
                files: "list indexed files and per-file symbol counts",
                init: "create .ochna/ochna.db and build the initial index",
                sync: "incrementally update the existing index after source changes",
            },
            node_modes: [
                "node --file <path> --symbols-only: list the symbols defined in a file",
                "node --file <path> --offset <line> --limit <n>: slice source by line range",
                "node --symbol <name> --include-code [--line <n>]: definition plus source; --line disambiguates overloads",
            ],
            flags: HowtoFlags {
                show_resolution: "print each edge's resolution kind and confidence on any query",
                min_confidence: "drop weak callers edges below <N> (e.g. 80 to keep only typed/qualified matches)",
                include_library: "index library/generated dirs (target, node_modules, .venv, vendor, build, dist) on init/sync",
                no_tests: "hide symbols classified from test paths",
                json: "emit machine-readable JSON on stdout",
            },
            confidence_cascade: [
                "exact=100",
                "receiver_type=90",
                "package_or_namespace=80",
                "same_file=60",
                "name_only=30",
            ],
            globals: ["--json", "--no-tests"],
            notes: [
                "the database resolves from the current working directory (.ochna/ochna.db)",
                "there is no `--workspace` flag; cd into the project or submodule before querying",
                "diagnostics and progress go to stderr; JSON stdout stays parseable",
            ],
        };
        println!("{}", serde_json::to_string_pretty(&descriptor)?);
    } else {
        println!("{}", HOWTO_TEXT.trim_end());
    }
    Ok(())
}

fn query_nodes_by_like(conn: &Connection, pattern: &str) -> rusqlite::Result<Vec<db::Node>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, kind, qualified_name, file_path, start_line, end_line, start_column, end_column, signature, doc_comment, is_test \
         FROM nodes \
         WHERE name LIKE ? OR qualified_name LIKE ? OR id LIKE ?"
    )?;
    let mut rows = stmt.query([pattern, pattern, pattern])?;
    let mut nodes = Vec::new();
    while let Some(row) = rows.next()? {
        nodes.push(db::map_row_to_node(row)?);
    }
    Ok(nodes)
}

fn query_nodes_by_id_or_qual(conn: &Connection, symbol: &str) -> rusqlite::Result<Vec<db::Node>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, kind, qualified_name, file_path, start_line, end_line, start_column, end_column, signature, doc_comment, is_test \
         FROM nodes \
         WHERE id = ? OR qualified_name = ?"
    )?;
    let mut rows = stmt.query([symbol, symbol])?;
    let mut nodes = Vec::new();
    while let Some(row) = rows.next()? {
        nodes.push(db::map_row_to_node(row)?);
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
fn emit_nodes(
    nodes: &[db::Node],
    json: bool,
    empty_msg: &str,
    show_resolution: bool,
) -> Result<(), Box<dyn Error>> {
    if json {
        println!("{}", serde_json::to_string_pretty(nodes)?);
    } else if nodes.is_empty() {
        println!("{}", empty_msg);
    } else {
        for n in nodes {
            let res_suffix = if show_resolution {
                if let (Some(ref res_kind), Some(conf)) = (&n.resolution_kind, n.confidence) {
                    format!(" [resolution: {}, confidence: {}]", res_kind, conf)
                } else {
                    "".to_string()
                }
            } else {
                "".to_string()
            };
            // Prefer the qualified name (e.g. `Type::method`) so symbols that
            // share a bare name across packages are distinguishable at a glance.
            let display_name = n.qualified_name.as_deref().unwrap_or(&n.name);
            println!(
                "- {} ({}) - {}:{}{}",
                display_name, n.kind, n.file_path, n.start_line, res_suffix
            );
        }
    }
    Ok(())
}

fn process_related_nodes(nodes: &mut Vec<db::Node>, no_tests: bool) {
    retain_non_tests(nodes, no_tests);
    // Sort by id, then by confidence descending so dedup keeps highest confidence
    nodes.sort_by(|a, b| {
        a.id.cmp(&b.id).then_with(|| {
            let a_conf = a.confidence.unwrap_or(-1);
            let b_conf = b.confidence.unwrap_or(-1);
            b_conf.cmp(&a_conf)
        })
    });
    nodes.dedup_by(|a, b| a.id == b.id);
    // Rank by confidence descending
    nodes.sort_by(|a, b| {
        let a_conf = a.confidence.unwrap_or(-1);
        let b_conf = b.confidence.unwrap_or(-1);
        b_conf.cmp(&a_conf).then_with(|| a.id.cmp(&b.id))
    });
}

fn retain_non_tests(nodes: &mut Vec<db::Node>, no_tests: bool) {
    if no_tests {
        nodes.retain(|node| !node.is_test);
    }
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

pub fn run_search(
    workspace: &Path,
    query: &str,
    json: bool,
    no_tests: bool,
    limit: usize,
) -> Result<(), Box<dyn Error>> {
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

    retain_non_tests(&mut nodes, no_tests);

    // Rank by relevance to query
    let query_lower = query.to_lowercase();
    nodes.sort_by_key(|n| {
        let name_lower = n.name.to_lowercase();
        let rank = if name_lower == query_lower {
            0
        } else if name_lower.starts_with(&query_lower) {
            1
        } else if name_lower.contains(&query_lower) {
            2
        } else {
            3
        };
        (rank, n.name.len(), n.name.clone())
    });

    let total = nodes.len();
    if total > limit {
        nodes.truncate(limit);
    }

    emit_nodes(&nodes, json, "No matching nodes found.", false)?;

    if !json && total > limit {
        println!("... and {} more (use --limit to see more)", total - limit);
    }

    Ok(())
}

pub fn run_callers(
    workspace: &Path,
    symbol: &str,
    json: bool,
    no_tests: bool,
    min_confidence: Option<i64>,
    show_resolution: bool,
    in_path: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let conn = open_db(workspace)?;

    // Find nodes matching the symbol name or symbol ID
    let mut target_nodes = db::query_nodes(&conn, Some(symbol), None, None).unwrap_or_default();

    // If empty, try to find by qualified name or ID
    if target_nodes.is_empty() {
        if let Ok(res) = query_nodes_by_id_or_qual(&conn, symbol) {
            target_nodes = res;
        }
    }
    retain_non_tests(&mut target_nodes, no_tests);

    // Filter by path prefix if --in was specified
    if let Some(prefix) = in_path {
        target_nodes.retain(|node| node.file_path.starts_with(prefix));
    }

    if target_nodes.is_empty() {
        return emit_nodes(
            &[],
            json,
            &format!("Symbol '{}' not found in database.", symbol),
            show_resolution,
        );
    }

    let mut callers = Vec::new();
    for node in target_nodes {
        if let Ok(node_callers) = db::find_callers(&conn, &node.id, None) {
            callers.extend(node_callers);
        }
    }

    if let Some(min) = min_confidence {
        callers.retain(|c| c.confidence.unwrap_or(0) >= min);
    }

    process_related_nodes(&mut callers, no_tests);

    emit_nodes(&callers, json, "No callers found.", show_resolution)
}

pub fn run_callees(
    workspace: &Path,
    symbol: &str,
    json: bool,
    no_tests: bool,
    min_confidence: Option<i64>,
    show_resolution: bool,
    in_path: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let conn = open_db(workspace)?;

    // Find nodes matching the symbol name or symbol ID
    let mut target_nodes = db::query_nodes(&conn, Some(symbol), None, None).unwrap_or_default();

    // If empty, try to find by qualified name or ID
    if target_nodes.is_empty() {
        if let Ok(res) = query_nodes_by_id_or_qual(&conn, symbol) {
            target_nodes = res;
        }
    }
    retain_non_tests(&mut target_nodes, no_tests);

    // Filter by path prefix if --in was specified
    if let Some(prefix) = in_path {
        target_nodes.retain(|node| node.file_path.starts_with(prefix));
    }

    if target_nodes.is_empty() {
        return emit_nodes(
            &[],
            json,
            &format!("Symbol '{}' not found in database.", symbol),
            show_resolution,
        );
    }

    let mut callees = Vec::new();
    for node in target_nodes {
        if let Ok(node_callees) = db::find_callees(&conn, &node.id, None) {
            callees.extend(node_callees);
        }
    }

    if let Some(min) = min_confidence {
        callees.retain(|c| c.confidence.unwrap_or(0) >= min);
    }

    process_related_nodes(&mut callees, no_tests);

    emit_nodes(&callees, json, "No callees found.", show_resolution)
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
    no_tests: bool,
    show_resolution: bool,
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
            let mut file_nodes = file_nodes;
            retain_non_tests(&mut file_nodes, no_tests);

            if symbols_only {
                if json {
                    return emit_nodes(&file_nodes, true, "", show_resolution);
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
            process_related_nodes(&mut dependents, no_tests);

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
                    let res_suffix = if show_resolution {
                        if let (Some(ref res_kind), Some(conf)) =
                            (&dep.resolution_kind, dep.confidence)
                        {
                            format!(" [resolution: {}, confidence: {}]", res_kind, conf)
                        } else {
                            "".to_string()
                        }
                    } else {
                        "".to_string()
                    };
                    println!(
                        "- {} ({}) - {}:{}{}",
                        dep.name, dep.kind, dep.file_path, dep.start_line, res_suffix
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
            retain_non_tests(&mut target_nodes, no_tests);

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
                    process_related_nodes(&mut callers, no_tests);
                    let mut callees = db::find_callees(&conn, &node.id, None).unwrap_or_default();
                    process_related_nodes(&mut callees, no_tests);
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
                process_related_nodes(&mut callers, no_tests);

                let mut callees = db::find_callees(&conn, &node.id, None).unwrap_or_default();
                process_related_nodes(&mut callees, no_tests);

                section.push("\nCallers:".to_string());
                if callers.is_empty() {
                    section.push("None".to_string());
                } else {
                    for caller in callers {
                        let res_suffix = if show_resolution {
                            if let (Some(ref res_kind), Some(conf)) =
                                (&caller.resolution_kind, caller.confidence)
                            {
                                format!(" [resolution: {}, confidence: {}]", res_kind, conf)
                            } else {
                                "".to_string()
                            }
                        } else {
                            "".to_string()
                        };
                        section.push(format!(
                            "- {} ({}) - {}:{}{}",
                            caller.name,
                            caller.kind,
                            caller.file_path,
                            caller.start_line,
                            res_suffix
                        ));
                    }
                }

                section.push("\nCallees:".to_string());
                if callees.is_empty() {
                    section.push("None".to_string());
                } else {
                    for callee in callees {
                        let res_suffix = if show_resolution {
                            if let (Some(ref res_kind), Some(conf)) =
                                (&callee.resolution_kind, callee.confidence)
                            {
                                format!(" [resolution: {}, confidence: {}]", res_kind, conf)
                            } else {
                                "".to_string()
                            }
                        } else {
                            "".to_string()
                        };
                        section.push(format!(
                            "- {} ({}) - {}:{}{}",
                            callee.name,
                            callee.kind,
                            callee.file_path,
                            callee.start_line,
                            res_suffix
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

pub fn run_explore(
    workspace: &Path,
    query: &str,
    json: bool,
    no_tests: bool,
    show_resolution: bool,
) -> Result<(), Box<dyn Error>> {
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
    retain_non_tests(&mut nodes, no_tests);

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
                process_related_nodes(&mut callers, no_tests);
                let mut callees = db::find_callees(&conn, &node.id, None).unwrap_or_default();
                process_related_nodes(&mut callees, no_tests);
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
            process_related_nodes(&mut callers, no_tests);

            let mut callees = db::find_callees(&conn, &node.id, None).unwrap_or_default();
            process_related_nodes(&mut callees, no_tests);

            output.push("  Relationships:".to_string());
            if callers.is_empty() {
                output.push("    Callers: None".to_string());
            } else {
                output.push("    Callers:".to_string());
                for c in callers {
                    let res_suffix = if show_resolution {
                        if let (Some(ref res_kind), Some(conf)) = (&c.resolution_kind, c.confidence)
                        {
                            format!(" [resolution: {}, confidence: {}]", res_kind, conf)
                        } else {
                            "".to_string()
                        }
                    } else {
                        "".to_string()
                    };
                    output.push(format!(
                        "      - {} ({}) - {}:{}{}",
                        c.name, c.kind, c.file_path, c.start_line, res_suffix
                    ));
                }
            }

            if callees.is_empty() {
                output.push("    Callees: None".to_string());
            } else {
                output.push("    Callees:".to_string());
                for c in callees {
                    let res_suffix = if show_resolution {
                        if let (Some(ref res_kind), Some(conf)) = (&c.resolution_kind, c.confidence)
                        {
                            format!(" [resolution: {}, confidence: {}]", res_kind, conf)
                        } else {
                            "".to_string()
                        }
                    } else {
                        "".to_string()
                    };
                    output.push(format!(
                        "      - {} ({}) - {}:{}{}",
                        c.name, c.kind, c.file_path, c.start_line, res_suffix
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

    fn node(id: &str, is_test: bool) -> db::Node {
        db::Node {
            id: id.to_string(),
            name: id.to_string(),
            kind: "function".to_string(),
            qualified_name: Some(id.to_string()),
            file_path: "src/main.rs".to_string(),
            start_line: 1,
            end_line: 1,
            start_column: 0,
            end_column: 0,
            signature: None,
            doc_comment: None,
            is_test,
            resolution_kind: None,
            confidence: None,
        }
    }

    #[test]
    fn retain_non_tests_filters_only_when_requested() {
        let mut nodes = vec![node("prod", false), node("test", true)];
        retain_non_tests(&mut nodes, false);
        assert_eq!(nodes.len(), 2);

        retain_non_tests(&mut nodes, true);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "prod");
    }
}
