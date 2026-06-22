//! Query commands: `search`, `callers`, `node`, and `explore`, plus the shared
//! node-lookup and emission helpers they build on.

use crate::db;
use rusqlite::Connection;
use serde_json::json;
use std::error::Error;
use std::fs;
use std::path::Path;

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
