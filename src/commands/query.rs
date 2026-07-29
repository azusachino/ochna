//! Query commands: `search`, `callers`, `node`, and `explore`, plus the shared
//! node-lookup and emission helpers they build on.

use crate::{db, ImpactDirection};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, VecDeque};
use std::error::Error;
use std::fs;
use std::path::Path;

const HOWTO_TEXT: &str = r#"ochna usage flow

1. Run `ochna status` first. If the index is stale, run `ochna sync` (or `ochna init` to build it).
2. Search for an entry point with `ochna search <name>` (results are ranked best-first and capped by `--limit`, default 30).
3. Trace incoming references with `ochna callers <name>`, or outgoing calls with `ochna callees <name>` for top-down walks.
4. Inspect a definition with `ochna node --symbol <name> --include-code`.
5. Use `ochna explore <query>` when you want search results, snippets, and graph context together.

ochna complements `rg`/`ast-grep`: use it for symbol and call-graph lookups (definitions, callers/callees) instead of reconstructing edges with `rg`; use `rg` for free-text/regex and `ast-grep` for structural AST patterns.

Inspecting nodes

- `ochna node --file <path> --symbols-only` — list the symbols defined in a file.
- `ochna node --file <path> --offset <line> --limit <n>` — slice source by line range.
- `ochna node --symbol <name> --include-code [--line <n>]` — definition plus source; `--line` disambiguates overloads.

Call-edge confidence

- Add `--show-resolution` to any query to print each edge's resolution kind and confidence.
- Add `--min-confidence <N>` to `callers` to drop weak edges. Cascade: exact 100, receiver_type 90, package/namespace 80, same_file 60, name_only 30. Use `--min-confidence 80` to cut noise on common method names.
- Disambiguate names that collide across packages with `--in <path-prefix>` on `callers`/`callees`, e.g. `ochna callees worker --in pkg/controller/podautoscaler`. Query output shows the receiver-qualified name (`Type::method`) when available.

Operational facts

- The database is resolved from the current working directory at `.ochna/ochna.db`.
- Target another workspace from any cwd with the global `--workspace <PATH>` (`-C <PATH>`) flag, e.g. `ochna -C clones/tokio search Runtime`.
- Add global `--json` for machine-readable stdout; add global `--no-tests` to hide symbols classified from test paths.
- `init`/`sync` skip library/generated dirs (target, node_modules, .venv, vendor, build, dist) by default; pass `--include-library` to index them.
- Diagnostics and progress go to stderr; JSON stdout is kept parseable.

Review workflow

1. Run `ochna doctor` before trusting graph evidence — reports index/schema/freshness, a `trust_verdict` (trusted/degraded/unusable), resolution tiers, unresolved/collision/low-quality diagnostics, and exits nonzero when degraded or unusable.
2. Run `ochna diff (--base <rev> [--head <rev>] | --files <path>...)` to map a Git range or explicit paths onto changed symbols and edges; `--base` alone diffs against the current worktree, `--base`+`--head` compares two revisions via disposable temporary indexes. Read-only; never mutates Git state.
3. Run `ochna impact <symbol> [--depth <n>] [--direction callers|callees|both] [--min-confidence <n>]` for a bounded, confidence-labelled blast radius (default depth 2, min-confidence 30); it reports `affected_tests` and `unresolved_boundaries`, and refuses to guess rather than exploding across an ambiguous or common name.
4. Run `ochna tests-for <symbol>` for test symbols with a structural path to it, each labelled `direct_call`, `same_module_candidate`, or `name_heuristic` — a candidate or heuristic result is never proof of coverage.
5. Run `ochna unresolved [--in <path-prefix>] [--limit <n>]` to list call sites that could not be resolved to a trusted edge, each with an explicit reason (`missing_target`, `ambiguous_target`, `macro_or_function`, `indirect_call`).

All review commands accept `--workspace`/`-C`, `--json`, and `--no-tests`; JSON mode emits the frozen `contract_version: "0.3"` envelope (`ok`, `data`, `warnings`, `truncated`, `next_action`) documented in `docs/v0.3-contract.md`.
"#;

#[derive(Serialize)]
struct HowtoDescriptor<'a> {
    flow: [&'a str; 6],
    review_flow: [&'a str; 5],
    rule: &'a str,
    commands: HowtoCommands<'a>,
    node_modes: [&'a str; 3],
    flags: HowtoFlags<'a>,
    confidence_cascade: [&'a str; 5],
    globals: [&'a str; 3],
    notes: [&'a str; 3],
}

#[derive(Serialize)]
struct HowtoCommands<'a> {
    status: &'a str,
    search: &'a str,
    callers: &'a str,
    callees: &'a str,
    node: &'a str,
    explore: &'a str,
    files: &'a str,
    init: &'a str,
    sync: &'a str,
    doctor: &'a str,
    unresolved: &'a str,
    #[serde(rename = "tests-for")]
    tests_for: &'a str,
    impact: &'a str,
    diff: &'a str,
}

#[derive(Serialize)]
struct HowtoFlags<'a> {
    show_resolution: &'a str,
    min_confidence: &'a str,
    #[serde(rename = "in")]
    in_scope: &'a str,
    limit: &'a str,
    include_library: &'a str,
    no_tests: &'a str,
    workspace: &'a str,
    json: &'a str,
}

pub fn run_howto(json: bool) -> Result<(), Box<dyn Error>> {
    if json {
        let descriptor = HowtoDescriptor {
            flow: ["status", "search", "callers", "callees", "node", "explore"],
            review_flow: ["doctor", "diff", "impact", "tests-for", "unresolved"],
            rule: "ochna complements rg/ast-grep: use it for symbol and call-graph lookups; use rg for free-text/regex and ast-grep for structural AST patterns",
            commands: HowtoCommands {
                status: "check whether the local index exists, matches the binary schema, and is fresh enough to trust",
                search: "fuzzy and full-text symbol lookup, ranked best-first and capped by --limit",
                callers: "reverse call-edge lookup for a symbol (who calls it)",
                callees: "forward call-edge lookup for a symbol (what it calls); use for top-down walks",
                node: "inspect a file, symbol metadata, and optionally source code",
                explore: "combined search, snippets, callers, and callees view",
                files: "list indexed files and per-file symbol counts",
                init: "create .ochna/ochna.db and build the initial index",
                sync: "incrementally update the existing index after source changes",
                doctor: "review preflight: index/schema/freshness, a trust_verdict (trusted/degraded/unusable), resolution tiers, and unresolved/collision/low-quality diagnostics; exits nonzero when degraded or unusable",
                unresolved: "list call sites that could not be resolved to a trusted edge, each with an explicit reason (missing_target, ambiguous_target, macro_or_function, indirect_call)",
                tests_for: "find test symbols with a structural path to a production symbol, labelled direct_call, same_module_candidate, or name_heuristic; never claims proof of runtime coverage",
                impact: "bounded confidence-labelled traversal from a resolved symbol (default depth 2, min-confidence 30); reports affected_tests and unresolved_boundaries, and refuses to guess across an ambiguous or common name rather than exploding unbounded",
                diff: "map a Git range (--base [--head]) or explicit --files onto changed symbols/edges; historical ranges use disposable temporary indexes and report base_revision_unavailable rather than guessing on a shallow clone",
            },
            node_modes: [
                "node --file <path> --symbols-only: list the symbols defined in a file",
                "node --file <path> --offset <line> --limit <n>: slice source by line range",
                "node --symbol <name> --include-code [--line <n>]: definition plus source; --line disambiguates overloads",
            ],
            flags: HowtoFlags {
                show_resolution: "print each edge's resolution kind and confidence on any query",
                min_confidence: "drop weak callers edges below <N> (e.g. 80 to keep only typed/qualified matches)",
                in_scope: "scope callers/callees target resolution to symbols under a file path prefix (disambiguates name collisions)",
                limit: "cap search results (default 30); results are ranked exact > prefix > substring > body match",
                include_library: "index library/generated dirs (target, node_modules, .venv, vendor, build, dist) on init/sync",
                no_tests: "hide symbols classified from test paths",
                workspace: "target a workspace's .ochna/ochna.db from any cwd (--workspace <PATH> / -C <PATH>)",
                json: "emit machine-readable JSON on stdout",
            },
            confidence_cascade: [
                "exact=100",
                "receiver_type=90",
                "package_or_namespace=80",
                "same_file=60",
                "name_only=30",
            ],
            globals: ["--json", "--no-tests", "--workspace"],
            notes: [
                "the database resolves from the current working directory (.ochna/ochna.db), or from --workspace <PATH> / -C <PATH>",
                "query output shows the receiver-qualified name (Type::method) when available",
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

#[derive(Clone, Serialize)]
struct RelationshipPath {
    relationship: String,
    source: db::Node,
    target: db::Node,
    resolution_kind: String,
    confidence: i64,
}

#[derive(Serialize)]
struct TestForItem {
    test: db::Node,
    path: Vec<RelationshipPath>,
    confidence: i64,
    evidence: &'static str,
}

struct TargetResolution {
    target: Option<db::Node>,
    warnings: Vec<serde_json::Value>,
}

fn resolve_one_node(
    conn: &Connection,
    symbol: &str,
    in_path: Option<&str>,
) -> rusqlite::Result<TargetResolution> {
    let mut exact = query_nodes_by_id_or_qual(conn, symbol)?;
    let mut nodes = if exact.is_empty() {
        db::query_nodes(conn, Some(symbol), None, None)?
    } else {
        std::mem::take(&mut exact)
    };
    if let Some(prefix) = in_path {
        nodes.retain(|node| node.file_path.starts_with(prefix));
    }
    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    if nodes.len() == 1 {
        Ok(TargetResolution {
            target: nodes.into_iter().next(),
            warnings: Vec::new(),
        })
    } else if nodes.len() > 1 {
        Ok(TargetResolution {
            target: None,
            warnings: vec![json!({
                "code": "ambiguous_symbol",
                "message": format!("symbol '{symbol}' matches {} indexed definitions; use --in or an ID/qualified name", nodes.len()),
            })],
        })
    } else {
        Ok(TargetResolution {
            target: None,
            warnings: Vec::new(),
        })
    }
}

fn edge_path(source: &db::Node, target: &db::Node, edge: &db::EdgeRecord) -> RelationshipPath {
    RelationshipPath {
        relationship: edge.kind.clone(),
        source: source.clone(),
        target: target.clone(),
        resolution_kind: edge.resolution_kind.clone(),
        confidence: edge.confidence,
    }
}

fn collect_tests_for(
    conn: &Connection,
    target: &db::Node,
    no_tests: bool,
) -> rusqlite::Result<Vec<TestForItem>> {
    if no_tests {
        return Ok(Vec::new());
    }

    let mut tests = db::query_nodes(conn, None, None, None)?;
    tests.retain(|node| node.is_test);
    tests.sort_by(|left, right| left.id.cmp(&right.id));

    let mut siblings = db::query_nodes(conn, None, None, Some(&target.file_path))?;
    siblings.retain(|node| !node.is_test && node.id != target.id);
    siblings.sort_by(|left, right| left.id.cmp(&right.id));

    let target_name = target.name.to_ascii_lowercase();
    let mut items = Vec::new();
    for test in tests {
        let mut callees = db::find_callees(conn, &test.id, None)?;
        callees.sort_by(|left, right| left.id.cmp(&right.id));

        let direct_edges = db::find_edges_between(conn, &test.id, &target.id, Some("calls"))?;
        if let Some(edge) = direct_edges.first() {
            let path = edge_path(&test, target, edge);
            items.push(TestForItem {
                test: test.clone(),
                confidence: path.confidence,
                path: vec![path],
                evidence: "direct_call",
            });
            continue;
        }

        if siblings
            .iter()
            .any(|sibling| callees.iter().any(|callee| callee.id == sibling.id))
        {
            items.push(TestForItem {
                test: test.clone(),
                confidence: 60,
                path: vec![RelationshipPath {
                    relationship: "references".to_string(),
                    source: test.clone(),
                    target: target.clone(),
                    resolution_kind: "same_module_candidate".to_string(),
                    confidence: 60,
                }],
                evidence: "same_module_candidate",
            });
            continue;
        }

        let test_name = test
            .qualified_name
            .as_deref()
            .unwrap_or(&test.name)
            .to_ascii_lowercase();
        if test_name.contains(&target_name) {
            items.push(TestForItem {
                test: test.clone(),
                confidence: 30,
                path: vec![RelationshipPath {
                    relationship: "tests".to_string(),
                    source: test.clone(),
                    target: target.clone(),
                    resolution_kind: "name_heuristic".to_string(),
                    confidence: 30,
                }],
                evidence: "name_heuristic",
            });
        }
    }

    items.sort_by(|left, right| {
        right
            .confidence
            .cmp(&left.confidence)
            .then_with(|| left.test.id.cmp(&right.test.id))
            .then_with(|| left.evidence.cmp(right.evidence))
    });
    Ok(items)
}

pub fn run_tests_for(
    workspace: &Path,
    symbol: &str,
    in_path: Option<&str>,
    limit: usize,
    json: bool,
    no_tests: bool,
) -> Result<(), Box<dyn Error>> {
    let conn = open_db(workspace)?;
    let resolution = resolve_one_node(&conn, symbol, in_path)?;
    let target = resolution.target;
    let mut tests = match &target {
        Some(target) => collect_tests_for(&conn, target, no_tests)?,
        None => Vec::new(),
    };
    let truncated = tests.len() > limit;
    tests.truncate(limit);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "contract_version": "0.3",
                "command": "tests-for",
                "ok": target.is_some(),
                "data": {"target": target, "tests": tests},
                "warnings": resolution.warnings,
                "truncated": truncated,
                "next_action": if truncated || !target.is_some() { "narrow the query" } else { "none" },
            }))?
        );
    } else {
        println!("Tests for {symbol}:");
        if target.is_none() {
            if resolution.warnings.is_empty() {
                println!("none (symbol not found)");
            } else {
                println!("none (symbol is ambiguous; use --in or an ID/qualified name)");
            }
        } else if tests.is_empty() {
            println!("none");
        } else {
            for item in tests {
                let path = item
                    .path
                    .iter()
                    .map(|edge| format!("{} -> {}", edge.source.id, edge.target.id))
                    .collect::<Vec<_>>()
                    .join("; ");
                println!(
                    "- {} [{}; confidence: {}]: {}",
                    item.test.id, item.evidence, item.confidence, path
                );
            }
        }
        if truncated {
            println!("Warnings:\n- result limit reached; narrow the query");
        }
    }
    Ok(())
}

const IMPACT_EDGE_LIMIT: usize = 400;

#[derive(Clone, Serialize)]
struct ImpactEdge {
    relationship: String,
    source: db::Node,
    target: db::Node,
    resolution_kind: String,
    confidence: i64,
}

#[derive(Serialize)]
struct ImpactPath {
    id: String,
    edges: Vec<ImpactEdge>,
}

#[derive(Serialize)]
struct AffectedTest {
    test: db::Node,
    path_ids: Vec<String>,
}

#[derive(Serialize)]
struct UnresolvedBoundary {
    source: db::Node,
    relationship: String,
    target: Option<db::Node>,
    specifier: String,
    reason: String,
    path_id: String,
}

struct TraversalState {
    node: db::Node,
    node_ids: Vec<String>,
    edges: Vec<ImpactEdge>,
    path_id: String,
}

fn map_node_at(row: &rusqlite::Row, start: usize) -> rusqlite::Result<db::Node> {
    Ok(db::Node {
        id: row.get(start)?,
        name: row.get(start + 1)?,
        kind: row.get(start + 2)?,
        qualified_name: row.get(start + 3)?,
        file_path: row.get(start + 4)?,
        start_line: row.get(start + 5)?,
        end_line: row.get(start + 6)?,
        start_column: row.get(start + 7)?,
        end_column: row.get(start + 8)?,
        signature: row.get(start + 9)?,
        doc_comment: row.get(start + 10)?,
        is_test: row.get(start + 11)?,
        resolution_kind: None,
        confidence: None,
    })
}

fn impact_edges_from(
    conn: &Connection,
    node_id: &str,
    direction: ImpactDirection,
) -> rusqlite::Result<Vec<ImpactEdge>> {
    let mut directions = Vec::new();
    if matches!(direction, ImpactDirection::Callees | ImpactDirection::Both) {
        directions.push((
            "e.source_nid = (SELECT nid FROM nodes WHERE id = ?1)",
            "outgoing",
        ));
    }
    if matches!(direction, ImpactDirection::Callers | ImpactDirection::Both) {
        directions.push((
            "e.target_nid = (SELECT nid FROM nodes WHERE id = ?1)",
            "incoming",
        ));
    }

    let mut result = Vec::new();
    for (filter, _) in directions {
        let sql = format!(
            "SELECT e.kind, e.resolution_kind, \
                    s.id, s.name, s.kind, s.qualified_name, s.file_path, s.start_line, s.end_line, s.start_column, s.end_column, s.signature, s.doc_comment, s.is_test, \
                    t.id, t.name, t.kind, t.qualified_name, t.file_path, t.start_line, t.end_line, t.start_column, t.end_column, t.signature, t.doc_comment, t.is_test \
             FROM edges e JOIN nodes s ON s.nid = e.source_nid JOIN nodes t ON t.nid = e.target_nid \
             WHERE {filter}"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([node_id], |row| {
            let resolution: i64 = row.get(1)?;
            Ok(ImpactEdge {
                relationship: match row.get::<_, String>(0)?.as_str() {
                    "calls" => "calls".to_string(),
                    "tests" => "tests".to_string(),
                    "contains" => "contains".to_string(),
                    other => other.to_string(),
                },
                source: map_node_at(row, 2)?,
                target: map_node_at(row, 14)?,
                resolution_kind: db::label_for_kind(resolution).to_string(),
                confidence: db::confidence_for_kind(resolution),
            })
        })?;
        result.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    result.sort_by(|left, right| {
        left.source
            .id
            .cmp(&right.source.id)
            .then_with(|| left.target.id.cmp(&right.target.id))
            .then_with(|| left.relationship.cmp(&right.relationship))
            .then_with(|| left.resolution_kind.cmp(&right.resolution_kind))
    });
    result.dedup_by(|left, right| {
        left.source.id == right.source.id
            && left.target.id == right.target.id
            && left.relationship == right.relationship
    });
    Ok(result)
}

fn impact_unresolved_for(
    conn: &Connection,
    source: &db::Node,
    path_id: &str,
) -> rusqlite::Result<Vec<UnresolvedBoundary>> {
    let mut stmt = conn.prepare(
        "SELECT specifier, kind FROM unresolved_refs \
         WHERE source_nid = (SELECT nid FROM nodes WHERE id = ?1) \
         ORDER BY line, column, specifier",
    )?;
    let rows = stmt.query_map([&source.id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut boundaries = Vec::new();
    for row in rows {
        let (specifier, kind) = row?;
        let reason = db::unresolved_reason(conn, &specifier, &kind)?;
        boundaries.push(UnresolvedBoundary {
            source: source.clone(),
            relationship: kind,
            target: None,
            specifier,
            reason,
            path_id: path_id.to_string(),
        });
    }
    Ok(boundaries)
}

fn impact_edge_key(edge: &ImpactEdge) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}",
        edge.source.id, edge.target.id, edge.relationship
    )
}

struct ImpactResult {
    nodes: BTreeMap<String, db::Node>,
    edges: BTreeMap<String, ImpactEdge>,
    paths: Vec<ImpactPath>,
    affected_tests: BTreeMap<String, AffectedTest>,
    boundaries: Vec<UnresolvedBoundary>,
    truncated: bool,
    truncated_frontiers: Vec<String>,
}

fn collect_impact(
    conn: &Connection,
    root: &db::Node,
    depth: usize,
    direction: ImpactDirection,
    min_confidence: i64,
    limit: usize,
    no_tests: bool,
) -> rusqlite::Result<ImpactResult> {
    let mut queue = VecDeque::from([TraversalState {
        node: root.clone(),
        node_ids: vec![root.id.clone()],
        edges: Vec::new(),
        path_id: "root".to_string(),
    }]);
    let mut nodes = BTreeMap::<String, db::Node>::new();
    let mut edges = BTreeMap::<String, ImpactEdge>::new();
    let mut paths = vec![ImpactPath {
        id: "root".to_string(),
        edges: Vec::new(),
    }];
    let mut next_path_number = 1usize;
    let mut affected_tests = BTreeMap::<String, AffectedTest>::new();
    let mut boundaries = Vec::<UnresolvedBoundary>::new();
    let mut truncated_frontiers = Vec::<String>::new();
    let mut truncated = false;

    while let Some(state) = queue.pop_front() {
        let unresolved = impact_unresolved_for(conn, &state.node, &state.path_id)?;
        if boundaries.len() + unresolved.len() > IMPACT_EDGE_LIMIT {
            truncated = true;
            truncated_frontiers.push(format!("{} (unresolved references)", state.node.id));
        } else {
            boundaries.extend(unresolved);
        }
        if state.edges.len() >= depth {
            continue;
        }
        for edge in impact_edges_from(conn, &state.node.id, direction)? {
            if edge.confidence < min_confidence
                || (no_tests && (edge.source.is_test || edge.target.is_test))
            {
                continue;
            }
            let next = if edge.source.id == state.node.id {
                edge.target.clone()
            } else {
                edge.source.clone()
            };
            if state.node_ids.iter().any(|id| id == &next.id) {
                continue;
            }
            let edge_key = impact_edge_key(&edge);
            if !edges.contains_key(&edge_key) && edges.len() == IMPACT_EDGE_LIMIT {
                truncated = true;
                truncated_frontiers.push(format!("{} -> {} (edge limit)", state.node.id, next.id));
                continue;
            }
            if !nodes.contains_key(&next.id) && nodes.len() == limit {
                truncated = true;
                truncated_frontiers.push(format!("{} -> {} (node limit)", state.node.id, next.id));
                continue;
            }
            if paths.len() == IMPACT_EDGE_LIMIT {
                truncated = true;
                truncated_frontiers.push(format!("{} -> {} (path limit)", state.node.id, next.id));
                continue;
            }
            let mut next_edges = state.edges.clone();
            next_edges.push(edge.clone());
            edges.entry(edge_key).or_insert(edge);
            nodes.entry(next.id.clone()).or_insert_with(|| next.clone());
            let next_path_id = format!("p{next_path_number}");
            next_path_number += 1;
            paths.push(ImpactPath {
                id: next_path_id.clone(),
                edges: next_edges.clone(),
            });
            if next.is_test {
                affected_tests
                    .entry(next.id.clone())
                    .and_modify(|item| item.path_ids.push(next_path_id.clone()))
                    .or_insert(AffectedTest {
                        test: next.clone(),
                        path_ids: vec![next_path_id.clone()],
                    });
            }
            let mut next_node_ids = state.node_ids.clone();
            next_node_ids.push(next.id.clone());
            queue.push_back(TraversalState {
                node: next,
                node_ids: next_node_ids,
                edges: next_edges,
                path_id: next_path_id,
            });
        }
    }
    Ok(ImpactResult {
        nodes,
        edges,
        paths,
        affected_tests,
        boundaries,
        truncated,
        truncated_frontiers,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_impact(
    workspace: &Path,
    symbol: &str,
    depth: usize,
    direction: ImpactDirection,
    min_confidence: i64,
    limit: usize,
    json: bool,
    no_tests: bool,
) -> Result<(), Box<dyn Error>> {
    let conn = open_db(workspace)?;
    let resolution = resolve_one_node(&conn, symbol, None)?;
    let Some(root) = resolution.target else {
        let data = json!({
            "root": serde_json::Value::Null,
            "direction": format!("{:?}", direction).to_ascii_lowercase(),
            "depth": depth,
            "min_confidence": min_confidence,
            "nodes": [], "edges": [], "paths": [], "affected_tests": [], "unresolved_boundaries": []
        });
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"contract_version":"0.3","command":"impact","ok":false,"data":data,"warnings":resolution.warnings,"truncated":false,"next_action":"narrow the query"})
                )?
            );
        } else {
            println!("Impact for {symbol}:");
            println!("Nodes:\nnone\nPaths:\nnone\nAffected tests:\nnone\nUnresolved boundaries:\nnone\nEdges:\nnone");
            if !resolution.warnings.is_empty() {
                println!("Warnings:");
                for warning in resolution.warnings {
                    println!(
                        "- {}",
                        warning["message"].as_str().unwrap_or("unknown warning")
                    );
                }
            }
        }
        return Ok(());
    };

    let result = collect_impact(
        &conn,
        &root,
        depth,
        direction,
        min_confidence,
        limit,
        no_tests,
    )?;
    let ImpactResult {
        nodes,
        edges,
        paths,
        affected_tests,
        boundaries,
        truncated,
        truncated_frontiers,
    } = result;

    let direction_name = match direction {
        ImpactDirection::Callers => "callers",
        ImpactDirection::Callees => "callees",
        ImpactDirection::Both => "both",
    };
    let mut warning_values = if truncated {
        vec![
            json!({"code":"truncated","message":format!("result budget reached; frontier not walked: {}", truncated_frontiers.first().map(String::as_str).unwrap_or("unknown"))}),
        ]
    } else {
        Vec::new()
    };
    if boundaries
        .iter()
        .any(|boundary| boundary.relationship != "calls")
    {
        warning_values.push(json!({
            "code":"unresolved_framework_endpoint",
            "message":"A framework relationship endpoint is not indexed; its target is null rather than inferred from a same-named symbol."
        }));
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "contract_version":"0.3", "command":"impact", "ok":true,
                "data":{"root":root,"direction":direction_name,"depth":depth,"min_confidence":min_confidence,"nodes":nodes.into_values().collect::<Vec<_>>(),"edges":edges.into_values().collect::<Vec<_>>(),"paths":paths,"affected_tests":affected_tests.into_values().collect::<Vec<_>>(),"unresolved_boundaries":boundaries},
                "warnings":warning_values, "truncated":truncated, "next_action":if truncated {"narrow the query"} else {"none"}
            }))?
        );
    } else {
        println!("Impact for {symbol}:");
        println!("Nodes:");
        for node in nodes.values() {
            println!("- {}", node.qualified_name.as_deref().unwrap_or(&node.id));
        }
        if nodes.is_empty() {
            println!("none");
        }
        println!("Paths:");
        for path in &paths {
            println!(
                "- {}: {}",
                path.id,
                path.edges
                    .iter()
                    .map(|edge| format!(
                        "{} -> {} [{}]",
                        edge.source.id, edge.target.id, edge.confidence
                    ))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
        if paths.is_empty() {
            println!("none");
        }
        println!("Affected tests:");
        for item in affected_tests.values() {
            println!("- {} ({})", item.test.id, item.path_ids.join(", "));
        }
        if affected_tests.is_empty() {
            println!("none");
        }
        println!("Unresolved boundaries:");
        for boundary in &boundaries {
            println!(
                "- {} -[{}]-> null ({}, {}, {})",
                boundary.source.id,
                boundary.relationship,
                boundary.specifier,
                boundary.reason,
                boundary.path_id
            );
        }
        if boundaries.is_empty() {
            println!("none");
        }
        println!("Edges:");
        for edge in edges.values() {
            println!(
                "- {} -> {} [{}; {}]",
                edge.source.id, edge.target.id, edge.resolution_kind, edge.confidence
            );
        }
        if edges.is_empty() {
            println!("none");
        }
        if truncated {
            println!(
                "Warnings:\n- result budget reached; frontier not walked: {}",
                truncated_frontiers
                    .first()
                    .map(String::as_str)
                    .unwrap_or("unknown")
            );
        }
    }
    Ok(())
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
    use rusqlite::Connection;

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

    #[test]
    fn impact_uses_indexed_edge_metadata_and_direction_without_name_guessing() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn).unwrap();
        let root = node("src/lib.rs::render", false);
        let caller = node("src/lib.rs::render_page", false);
        let test = node("tests/render_tests.rs::render_page_uses_render", true);
        let unrelated = node("src/other.rs::render", false);
        for node in [&root, &caller, &test, &unrelated] {
            db::upsert_node(&conn, node).unwrap();
        }
        db::upsert_edge(
            &conn,
            &db::Edge {
                source_id: caller.id.clone(),
                target_id: root.id.clone(),
                kind: "calls".to_string(),
                resolution_kind: 4,
            },
        )
        .unwrap();
        db::upsert_edge(
            &conn,
            &db::Edge {
                source_id: test.id.clone(),
                target_id: caller.id.clone(),
                kind: "calls".to_string(),
                resolution_kind: 5,
            },
        )
        .unwrap();

        let incoming = impact_edges_from(&conn, &root.id, ImpactDirection::Callers).unwrap();
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].source.id, caller.id);
        assert_eq!(incoming[0].target.id, root.id);
        assert_eq!(incoming[0].resolution_kind, "receiver_type");
        assert_eq!(incoming[0].confidence, 90);
        assert!(impact_edges_from(&conn, &root.id, ImpactDirection::Callees)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn impact_preserves_converging_paths_terminates_cycles_and_reports_frontiers() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn).unwrap();
        let root = node("root", false);
        let left = node("left", false);
        let right = node("right", false);
        let test = node("test", true);
        let cycle = node("cycle", false);
        for node in [&root, &left, &right, &test, &cycle] {
            db::upsert_node(&conn, node).unwrap();
        }
        for (source, target) in [
            (&left, &root),
            (&right, &root),
            (&test, &root),
            (&test, &left),
            (&test, &right),
            (&root, &cycle),
            (&cycle, &root),
        ] {
            db::upsert_edge(
                &conn,
                &db::Edge {
                    source_id: source.id.clone(),
                    target_id: target.id.clone(),
                    kind: "calls".to_string(),
                    resolution_kind: 5,
                },
            )
            .unwrap();
        }
        db::insert_unresolved_ref(
            &conn,
            &db::UnresolvedRef {
                id: None,
                source_id: left.id.clone(),
                specifier: "missing".to_string(),
                kind: "call".to_string(),
                line: 1,
                column: 1,
            },
        )
        .unwrap();

        let first = collect_impact(&conn, &root, 5, ImpactDirection::Both, 0, 50, false).unwrap();
        let second = collect_impact(&conn, &root, 5, ImpactDirection::Both, 0, 50, false).unwrap();
        let test_paths = &first.affected_tests["test"].path_ids;
        assert_eq!(
            test_paths.len(),
            3,
            "direct and both converging paths reach the test"
        );
        let emitted_ids = first
            .paths
            .iter()
            .map(|path| path.id.as_str())
            .collect::<Vec<_>>();
        assert!(test_paths
            .iter()
            .all(|id| emitted_ids.contains(&id.as_str())));
        let test_path_lengths = first
            .paths
            .iter()
            .filter(|path| test_paths.contains(&path.id))
            .map(|path| path.edges.len())
            .collect::<Vec<_>>();
        assert!(
            test_path_lengths.contains(&1),
            "the direct path is retained"
        );
        assert_eq!(
            test_path_lengths.iter().filter(|&&len| len == 2).count(),
            2,
            "both converging paths are retained"
        );
        assert!(!first.boundaries.is_empty());
        assert!(first
            .boundaries
            .iter()
            .all(|boundary| emitted_ids.contains(&boundary.path_id.as_str())));
        for path in &first.paths {
            let mut current = root.id.clone();
            let mut visited = std::collections::BTreeSet::from([current.clone()]);
            for edge in &path.edges {
                current = if edge.source.id == current {
                    edge.target.id.clone()
                } else if edge.target.id == current {
                    edge.source.id.clone()
                } else {
                    panic!("path {} contains a non-contiguous edge", path.id);
                };
                assert!(
                    visited.insert(current.clone()),
                    "path {} repeats node {current}",
                    path.id
                );
            }
        }
        assert_eq!(
            first
                .paths
                .iter()
                .map(|path| {
                    (
                        path.id.as_str(),
                        path.edges
                            .iter()
                            .map(|edge| {
                                (
                                    edge.source.id.as_str(),
                                    edge.target.id.as_str(),
                                    edge.relationship.as_str(),
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>(),
            second
                .paths
                .iter()
                .map(|path| {
                    (
                        path.id.as_str(),
                        path.edges
                            .iter()
                            .map(|edge| {
                                (
                                    edge.source.id.as_str(),
                                    edge.target.id.as_str(),
                                    edge.relationship.as_str(),
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>(),
            "path IDs and ordered edge sequences are deterministic across identical runs"
        );

        let limited =
            collect_impact(&conn, &root, 2, ImpactDirection::Callers, 0, 1, false).unwrap();
        assert!(limited.truncated);
        assert!(limited
            .truncated_frontiers
            .iter()
            .any(|frontier| frontier.contains("node limit")));
    }

    #[test]
    fn tests_for_labels_direct_and_candidate_evidence_without_coverage_claims() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn).unwrap();

        let target = db::Node {
            id: "src/lib.rs::render".to_string(),
            name: "render".to_string(),
            kind: "function".to_string(),
            qualified_name: Some("render".to_string()),
            file_path: "src/lib.rs".to_string(),
            start_line: 1,
            end_line: 3,
            start_column: 0,
            end_column: 1,
            signature: None,
            doc_comment: None,
            is_test: false,
            resolution_kind: None,
            confidence: None,
        };
        let sibling = db::Node {
            id: "src/lib.rs::render_page".to_string(),
            name: "render_page".to_string(),
            qualified_name: Some("render_page".to_string()),
            ..target.clone()
        };
        let test = db::Node {
            id: "tests/render_tests.rs::render_page_uses_render".to_string(),
            name: "render_page_uses_render".to_string(),
            qualified_name: Some("render_page_uses_render".to_string()),
            file_path: "tests/render_tests.rs".to_string(),
            is_test: true,
            ..target.clone()
        };
        let module_candidate = db::Node {
            id: "tests/render_tests.rs::module_probe".to_string(),
            name: "module_probe".to_string(),
            qualified_name: Some("module_probe".to_string()),
            file_path: "tests/render_tests.rs".to_string(),
            is_test: true,
            ..target.clone()
        };
        let name_candidate = db::Node {
            id: "tests/render_tests.rs::render_name_probe".to_string(),
            name: "render_name_probe".to_string(),
            qualified_name: Some("render_name_probe".to_string()),
            file_path: "tests/render_tests.rs".to_string(),
            is_test: true,
            ..target.clone()
        };
        for node in [&target, &sibling, &test, &module_candidate, &name_candidate] {
            db::upsert_node(&conn, node).unwrap();
        }
        for target_id in [&target.id, &sibling.id] {
            db::upsert_edge(
                &conn,
                &db::Edge {
                    source_id: test.id.clone(),
                    target_id: target_id.clone(),
                    kind: "calls".to_string(),
                    resolution_kind: 5,
                },
            )
            .unwrap();
        }
        db::upsert_edge(
            &conn,
            &db::Edge {
                source_id: module_candidate.id.clone(),
                target_id: sibling.id.clone(),
                kind: "calls".to_string(),
                resolution_kind: 5,
            },
        )
        .unwrap();
        // A future relationship kind between the same test/target must not be
        // relabelled as direct call evidence.
        db::upsert_edge(
            &conn,
            &db::Edge {
                source_id: name_candidate.id.clone(),
                target_id: target.id.clone(),
                kind: "tests".to_string(),
                resolution_kind: 5,
            },
        )
        .unwrap();

        let items = collect_tests_for(&conn, &target, false).unwrap();
        assert_eq!(
            items
                .iter()
                .map(|item| (item.test.id.as_str(), item.evidence))
                .collect::<Vec<_>>(),
            vec![
                (
                    "tests/render_tests.rs::render_page_uses_render",
                    "direct_call"
                ),
                (
                    "tests/render_tests.rs::module_probe",
                    "same_module_candidate"
                ),
                ("tests/render_tests.rs::render_name_probe", "name_heuristic"),
            ]
        );
        assert_eq!(items[0].confidence, 100);
        assert_eq!(items[1].confidence, 60);
        assert_eq!(items[2].confidence, 30);
        assert!(items[2].confidence <= 60);
        assert_eq!(items[1].path[0].relationship, "references");
        assert_eq!(items[1].path[0].resolution_kind, "same_module_candidate");
        assert_eq!(items[2].path[0].relationship, "tests");
        assert_eq!(items[2].evidence, "name_heuristic");

        assert!(collect_tests_for(&conn, &target, true).unwrap().is_empty());
    }

    #[test]
    fn tests_for_requires_scope_for_ambiguous_simple_names() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn).unwrap();
        let left = db::Node {
            id: "src/left.rs::run".to_string(),
            name: "run".to_string(),
            kind: "function".to_string(),
            qualified_name: Some("run".to_string()),
            file_path: "src/left.rs".to_string(),
            start_line: 1,
            end_line: 1,
            start_column: 0,
            end_column: 0,
            signature: None,
            doc_comment: None,
            is_test: false,
            resolution_kind: None,
            confidence: None,
        };
        let right = db::Node {
            id: "src/right.rs::run".to_string(),
            file_path: "src/right.rs".to_string(),
            ..left.clone()
        };
        db::upsert_node(&conn, &left).unwrap();
        db::upsert_node(&conn, &right).unwrap();

        let ambiguous = resolve_one_node(&conn, "run", None).unwrap();
        assert!(ambiguous.target.is_none());
        assert_eq!(ambiguous.warnings[0]["code"], "ambiguous_symbol");

        let scoped = resolve_one_node(&conn, "run", Some("src/right")).unwrap();
        assert_eq!(scoped.target.unwrap().id, right.id);

        let exact = resolve_one_node(&conn, &left.id, None).unwrap();
        assert_eq!(exact.target.unwrap().id, left.id);
    }
}
