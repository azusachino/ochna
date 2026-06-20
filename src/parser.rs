use crate::db::{Edge, Node, RawCall, UnresolvedRef};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use std::error::Error;
use tree_sitter::Parser;

pub type SymbolIx = u32;
pub type CandidateList = SmallVec<[SymbolIx; 4]>;
pub type CandidateByFile = FxHashMap<String, CandidateList>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolCandidate {
    pub id: String,
    pub file_path: String,
    pub namespace: Option<String>,
}

impl SymbolCandidate {
    pub fn new(id: String, file_path: String, qualified_name: Option<String>) -> Self {
        let namespace = qualified_name
            .as_deref()
            .and_then(parent_namespace)
            .map(str::to_string);
        Self {
            id,
            file_path,
            namespace,
        }
    }
}

fn parent_namespace(qualified_name: &str) -> Option<&str> {
    let (parent, _) = qualified_name.rsplit_once("::")?;
    parent.rsplit("::").next()
}

/// Build a [`RawCall`] from a caller id, callee name, and the call-site AST node
/// (whose start position records where the call occurs).
fn raw_call(caller_id: &str, callee_name: String, call_node: tree_sitter::Node) -> RawCall {
    let pos = call_node.start_position();
    RawCall {
        caller_id: caller_id.to_string(),
        callee_name,
        line: (pos.row + 1) as i64,
        column: pos.column as i64,
    }
}

/// Parse supported source and extract nodes (symbols) plus the raw,
/// unresolved call sites within them. Call sites are returned unresolved so the
/// caller can resolve them against the whole-project symbol index (see
/// [`resolve_calls_global`]) rather than only the symbols in this one file.
pub fn parse_code(
    file_path: &str,
    content: &str,
    language: &str,
) -> Result<(Vec<Node>, Vec<RawCall>), Box<dyn Error>> {
    let mut parser = Parser::new();
    let lang = language.to_lowercase();

    match lang.as_str() {
        "rust" | "rs" => parser.set_language(&tree_sitter_rust::LANGUAGE.into())?,
        "go" => parser.set_language(&tree_sitter_go::LANGUAGE.into())?,
        "java" => parser.set_language(&tree_sitter_java::LANGUAGE.into())?,
        "c" => parser.set_language(&tree_sitter_c::LANGUAGE.into())?,
        "cpp" | "c++" | "cc" | "cxx" => parser.set_language(&tree_sitter_cpp::LANGUAGE.into())?,
        "zig" => parser.set_language(&tree_sitter_zig::LANGUAGE.into())?,
        _ => return Err(format!("Unsupported language: {}", language).into()),
    }

    let tree = parser
        .parse(content, None)
        .ok_or("Failed to parse code content")?;

    let mut nodes = Vec::new();
    let mut raw_calls = Vec::new();

    if lang == "rust" || lang == "rs" {
        traverse_rust(
            tree.root_node(),
            content,
            file_path,
            None,
            &mut nodes,
            &mut raw_calls,
            None,
        );
    } else if lang == "go" {
        traverse_go(
            tree.root_node(),
            content,
            file_path,
            &mut nodes,
            &mut raw_calls,
            None,
        );
    } else if lang == "java" {
        traverse_java(
            tree.root_node(),
            content,
            file_path,
            &mut nodes,
            &mut raw_calls,
            None,
            None,
        );
    } else if lang == "c" || lang == "cpp" || lang == "c++" || lang == "cc" || lang == "cxx" {
        traverse_c_like(
            tree.root_node(),
            content,
            file_path,
            &mut nodes,
            &mut raw_calls,
            CLikeContext {
                parent_qualified_name: None,
                parent_is_type: false,
            },
            None,
        );
    } else if lang == "zig" {
        traverse_zig(
            tree.root_node(),
            content,
            file_path,
            &mut nodes,
            &mut raw_calls,
            None,
            None,
        );
    }

    Ok((nodes, raw_calls))
}

/// Resolve raw call sites into concrete edges against a project-wide symbol
/// index, preferring a target in the caller's own file before falling back to a
/// unique match anywhere in the project. Calls that match no known symbol are
/// returned separately as [`UnresolvedRef`]s.
///
/// - `symbols` stores the candidate metadata indexed by `SymbolIx`.
/// - `by_name` maps a symbol's simple name to every candidate index that bears it.
/// - `by_name_file` maps simple name, then file, to same-file candidate indices.
/// - `by_id` maps a node id to its candidate index.
pub fn resolve_calls_global(
    calls: &[RawCall],
    symbols: &[SymbolCandidate],
    by_name: &FxHashMap<String, CandidateList>,
    by_name_file: &FxHashMap<String, CandidateByFile>,
    by_id: &FxHashMap<String, SymbolIx>,
) -> (Vec<Edge>, Vec<UnresolvedRef>) {
    let mut edges = Vec::new();
    let mut unresolved = Vec::new();

    for call in calls {
        let (call_namespace, simple_name) = call_namespace_and_simple_name(&call.callee_name);

        let candidates = match by_name.get(simple_name) {
            Some(indices) if !indices.is_empty() => indices,
            _ => {
                unresolved.push(UnresolvedRef {
                    id: None,
                    source_id: call.caller_id.clone(),
                    specifier: call.callee_name.clone(),
                    kind: "calls".to_string(),
                    line: call.line,
                    column: call.column,
                });
                continue;
            }
        };

        // Prefer a target defined in the caller's own file, else consider all matches.
        let caller = by_id
            .get(&call.caller_id)
            .and_then(|ix| symbols.get(*ix as usize));
        let same_file = caller.and_then(|caller| {
            by_name_file
                .get(simple_name)
                .and_then(|by_file| by_file.get(&caller.file_path))
        });
        let working_set = if let Some(indices) = same_file.filter(|indices| !indices.is_empty()) {
            indices.as_slice()
        } else {
            candidates.as_slice()
        };

        let target_id = if working_set.len() == 1 {
            symbols[working_set[0] as usize].id.clone()
        } else {
            let caller_namespace = caller.and_then(|symbol| symbol.namespace.as_deref());
            let target_ix = disambiguate(working_set, symbols, call_namespace, caller_namespace)
                .unwrap_or(working_set[0]);
            symbols[target_ix as usize].id.clone()
        };

        edges.push(Edge {
            source_id: call.caller_id.clone(),
            target_id,
            kind: "calls".to_string(),
        });
    }

    edges.sort_by(|a, b| {
        (&a.source_id, &a.target_id, &a.kind).cmp(&(&b.source_id, &b.target_id, &b.kind))
    });
    edges.dedup_by(|a, b| {
        a.source_id == b.source_id && a.target_id == b.target_id && a.kind == b.kind
    });

    (edges, unresolved)
}

/// Pick a target among several same-named candidates using namespace and
/// receiver context. Case A: the call carries an explicit namespace
/// (e.g. `Point::new`). Case B: fall back to the caller's own struct/class.
fn disambiguate(
    candidates: &[SymbolIx],
    symbols: &[SymbolCandidate],
    call_namespace: Option<&str>,
    caller_namespace: Option<&str>,
) -> Option<SymbolIx> {
    // Case A: explicit namespace on the call target.
    if let Some(namespace) = call_namespace {
        if let Some(candidate) = candidates.iter().copied().find(|ix| {
            symbols
                .get(*ix as usize)
                .and_then(|symbol| symbol.namespace.as_deref())
                == Some(namespace)
        }) {
            return Some(candidate);
        }
    }

    // Case B: caller is a method/constructor — prefer a target on the same receiver.
    if let Some(namespace) = caller_namespace {
        if let Some(candidate) = candidates.iter().copied().find(|ix| {
            symbols
                .get(*ix as usize)
                .and_then(|symbol| symbol.namespace.as_deref())
                == Some(namespace)
        }) {
            return Some(candidate);
        }
    }

    None
}

fn call_namespace_and_simple_name(callee_name: &str) -> (Option<&str>, &str) {
    if let Some((namespace, simple_name)) = callee_name.rsplit_once("::") {
        (namespace.rsplit("::").next(), simple_name)
    } else {
        (None, callee_name)
    }
}

/// Resolve call sites against only the given `nodes` (single-file scope). A thin
/// wrapper over [`resolve_calls_global`] used in tests and isolated parsing.
pub fn resolve_calls_local(nodes: &[Node], calls: &[RawCall]) -> Vec<Edge> {
    let mut symbols = Vec::with_capacity(nodes.len());
    let mut by_name: FxHashMap<String, CandidateList> = FxHashMap::default();
    let mut by_name_file: FxHashMap<String, CandidateByFile> = FxHashMap::default();
    let mut by_id: FxHashMap<String, SymbolIx> = FxHashMap::default();
    for n in nodes {
        let ix = symbols.len() as SymbolIx;
        symbols.push(SymbolCandidate::new(
            n.id.clone(),
            n.file_path.clone(),
            n.qualified_name.clone(),
        ));
        by_name.entry(n.name.clone()).or_default().push(ix);
        by_name_file
            .entry(n.name.clone())
            .or_default()
            .entry(n.file_path.clone())
            .or_default()
            .push(ix);
        by_id.insert(n.id.clone(), ix);
    }
    resolve_calls_global(calls, &symbols, &by_name, &by_name_file, &by_id).0
}

/// Recursively traverses a Rust AST.
fn traverse_rust<'a>(
    node: tree_sitter::Node<'a>,
    content: &'a str,
    file_path: &str,
    current_impl_struct: Option<&str>,
    nodes: &mut Vec<Node>,
    calls: &mut Vec<RawCall>,
    current_caller_id: Option<&str>,
) {
    let mut next_impl_struct = current_impl_struct;
    let mut next_caller_id = current_caller_id;

    #[allow(unused_assignments)]
    let mut struct_name_holder = String::new();
    #[allow(unused_assignments)]
    let mut id_holder = String::new();

    let kind = node.kind();
    match kind {
        "struct_item" => {
            let name_node = node
                .child_by_field_name("name")
                .or_else(|| find_child_by_kind(node, "type_identifier"));
            if let Some(name_node) = name_node {
                let name = name_node
                    .utf8_text(content.as_bytes())
                    .unwrap_or("")
                    .to_string();
                let id = format!("{}::{}", file_path, name);
                let doc = get_doc_comment(node, content);
                let sig = get_signature(node, content);
                let start_point = node.start_position();
                let end_point = node.end_position();
                nodes.push(Node {
                    id: id.clone(),
                    name: name.clone(),
                    kind: "struct".to_string(),
                    qualified_name: Some(name),
                    file_path: file_path.to_string(),
                    start_line: (start_point.row + 1) as i64,
                    end_line: (end_point.row + 1) as i64,
                    start_column: start_point.column as i64,
                    end_column: end_point.column as i64,
                    signature: Some(sig),
                    doc_comment: doc,
                });
            }
        }
        "enum_item" => {
            let name_node = node
                .child_by_field_name("name")
                .or_else(|| find_child_by_kind(node, "type_identifier"));
            if let Some(name_node) = name_node {
                let name = name_node
                    .utf8_text(content.as_bytes())
                    .unwrap_or("")
                    .to_string();
                let id = format!("{}::{}", file_path, name);
                let doc = get_doc_comment(node, content);
                let sig = get_signature(node, content);
                let start_point = node.start_position();
                let end_point = node.end_position();
                nodes.push(Node {
                    id: id.clone(),
                    name: name.clone(),
                    kind: "enum".to_string(),
                    qualified_name: Some(name),
                    file_path: file_path.to_string(),
                    start_line: (start_point.row + 1) as i64,
                    end_line: (end_point.row + 1) as i64,
                    start_column: start_point.column as i64,
                    end_column: end_point.column as i64,
                    signature: Some(sig),
                    doc_comment: doc,
                });
            }
        }
        "trait_item" => {
            let name_node = node
                .child_by_field_name("name")
                .or_else(|| find_child_by_kind(node, "type_identifier"));
            if let Some(name_node) = name_node {
                let name = name_node
                    .utf8_text(content.as_bytes())
                    .unwrap_or("")
                    .to_string();
                let id = format!("{}::{}", file_path, name);
                let doc = get_doc_comment(node, content);
                let sig = get_signature(node, content);
                let start_point = node.start_position();
                let end_point = node.end_position();
                nodes.push(Node {
                    id: id.clone(),
                    name: name.clone(),
                    kind: "trait".to_string(),
                    qualified_name: Some(name),
                    file_path: file_path.to_string(),
                    start_line: (start_point.row + 1) as i64,
                    end_line: (end_point.row + 1) as i64,
                    start_column: start_point.column as i64,
                    end_column: end_point.column as i64,
                    signature: Some(sig),
                    doc_comment: doc,
                });
            }
        }
        "impl_item" => {
            let type_node = node
                .child_by_field_name("type")
                .or_else(|| find_child_by_kind(node, "type_identifier"));
            if let Some(type_node) = type_node {
                let struct_name = type_node
                    .utf8_text(content.as_bytes())
                    .unwrap_or("")
                    .to_string();
                struct_name_holder = struct_name;
                next_impl_struct = Some(&struct_name_holder);
            }
        }
        "function_item" => {
            let name_node = node
                .child_by_field_name("name")
                .or_else(|| find_child_by_kind(node, "identifier"));
            if let Some(name_node) = name_node {
                let name = name_node
                    .utf8_text(content.as_bytes())
                    .unwrap_or("")
                    .to_string();
                let doc = get_doc_comment(node, content);
                let sig = get_signature(node, content);
                let start_point = node.start_position();
                let end_point = node.end_position();

                let (id, qualified_name, kind) = if let Some(sname) = current_impl_struct {
                    (
                        format!("{}::{}::{}", file_path, sname, name),
                        Some(format!("{}::{}", sname, name)),
                        "method".to_string(),
                    )
                } else {
                    (
                        format!("{}::{}", file_path, name),
                        Some(name.clone()),
                        "function".to_string(),
                    )
                };

                nodes.push(Node {
                    id: id.clone(),
                    name: name.clone(),
                    kind,
                    qualified_name,
                    file_path: file_path.to_string(),
                    start_line: (start_point.row + 1) as i64,
                    end_line: (end_point.row + 1) as i64,
                    start_column: start_point.column as i64,
                    end_column: end_point.column as i64,
                    signature: Some(sig),
                    doc_comment: doc,
                });

                id_holder = id;
                next_caller_id = Some(&id_holder);
            }
        }
        "call_expression" => {
            if let Some(caller) = current_caller_id {
                if let Some(func_node) = node.child_by_field_name("function") {
                    let func_name = extract_rust_call_target(func_node, content);
                    calls.push(raw_call(caller, func_name, node));
                }
            }
        }
        "method_call_expression" => {
            if let Some(caller) = current_caller_id {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let method_name = name_node
                        .utf8_text(content.as_bytes())
                        .unwrap_or("")
                        .to_string();
                    calls.push(raw_call(caller, method_name, node));
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        traverse_rust(
            child,
            content,
            file_path,
            next_impl_struct,
            nodes,
            calls,
            next_caller_id,
        );
    }
}

/// Extracts call target simple name for Rust functions.
fn extract_rust_call_target(node: tree_sitter::Node, content: &str) -> String {
    let kind = node.kind();
    match kind {
        "identifier" => node.utf8_text(content.as_bytes()).unwrap_or("").to_string(),
        "path_expression" => {
            let text = node.utf8_text(content.as_bytes()).unwrap_or("");
            text.split("::").last().unwrap_or("").to_string()
        }
        "field_expression" => {
            if let Some(field_node) = node.child_by_field_name("field") {
                field_node
                    .utf8_text(content.as_bytes())
                    .unwrap_or("")
                    .to_string()
            } else {
                let text = node.utf8_text(content.as_bytes()).unwrap_or("");
                text.split('.').next_back().unwrap_or("").to_string()
            }
        }
        _ => node.utf8_text(content.as_bytes()).unwrap_or("").to_string(),
    }
}

/// Recursively traverses a Go AST.
fn traverse_go<'a>(
    node: tree_sitter::Node<'a>,
    content: &'a str,
    file_path: &str,
    nodes: &mut Vec<Node>,
    calls: &mut Vec<RawCall>,
    current_caller_id: Option<&str>,
) {
    let mut next_caller_id = current_caller_id;
    #[allow(unused_assignments)]
    let mut id_holder = String::new();

    let kind = node.kind();
    match kind {
        "type_declaration" => {
            if let Some(type_spec) = find_child_by_kind(node, "type_spec") {
                let name_node = type_spec
                    .child_by_field_name("name")
                    .or_else(|| find_child_by_kind(type_spec, "type_identifier"));
                if let Some(name_node) = name_node {
                    let name = name_node
                        .utf8_text(content.as_bytes())
                        .unwrap_or("")
                        .to_string();
                    let doc = get_doc_comment(node, content);
                    let sig = format!("type {}", get_signature(type_spec, content));
                    let start_point = node.start_position();
                    let end_point = node.end_position();

                    let mut node_kind = None;
                    if find_child_by_kind(type_spec, "struct_type").is_some() {
                        node_kind = Some("struct");
                    } else if find_child_by_kind(type_spec, "interface_type").is_some() {
                        node_kind = Some("interface");
                    }

                    if let Some(k) = node_kind {
                        let id = format!("{}::{}", file_path, name);
                        nodes.push(Node {
                            id: id.clone(),
                            name: name.clone(),
                            kind: k.to_string(),
                            qualified_name: Some(name),
                            file_path: file_path.to_string(),
                            start_line: (start_point.row + 1) as i64,
                            end_line: (end_point.row + 1) as i64,
                            start_column: start_point.column as i64,
                            end_column: end_point.column as i64,
                            signature: Some(sig),
                            doc_comment: doc,
                        });
                    }
                }
            }
        }
        "function_declaration" => {
            let name_node = node
                .child_by_field_name("name")
                .or_else(|| find_child_by_kind(node, "identifier"));
            if let Some(name_node) = name_node {
                let name = name_node
                    .utf8_text(content.as_bytes())
                    .unwrap_or("")
                    .to_string();
                let doc = get_doc_comment(node, content);
                let sig = get_signature(node, content);
                let start_point = node.start_position();
                let end_point = node.end_position();
                let id = format!("{}::{}", file_path, name);

                nodes.push(Node {
                    id: id.clone(),
                    name: name.clone(),
                    kind: "function".to_string(),
                    qualified_name: Some(name),
                    file_path: file_path.to_string(),
                    start_line: (start_point.row + 1) as i64,
                    end_line: (end_point.row + 1) as i64,
                    start_column: start_point.column as i64,
                    end_column: end_point.column as i64,
                    signature: Some(sig),
                    doc_comment: doc,
                });

                id_holder = id;
                next_caller_id = Some(&id_holder);
            }
        }
        "method_declaration" => {
            let name_node = node
                .child_by_field_name("name")
                .or_else(|| find_child_by_kind(node, "field_identifier"));
            if let Some(name_node) = name_node {
                let name = name_node
                    .utf8_text(content.as_bytes())
                    .unwrap_or("")
                    .to_string();
                let doc = get_doc_comment(node, content);
                let sig = get_signature(node, content);
                let start_point = node.start_position();
                let end_point = node.end_position();

                let receiver_node = node
                    .child_by_field_name("receiver")
                    .or_else(|| find_child_by_kind(node, "parameter_list"));

                let receiver_struct = if let Some(rec) = receiver_node {
                    get_go_receiver_type(rec, content)
                } else {
                    None
                };

                let (id, qualified_name) = if let Some(ref rstruct) = receiver_struct {
                    (
                        format!("{}::{}::{}", file_path, rstruct, name),
                        Some(format!("{}::{}", rstruct, name)),
                    )
                } else {
                    (format!("{}::{}", file_path, name), Some(name.clone()))
                };

                nodes.push(Node {
                    id: id.clone(),
                    name: name.clone(),
                    kind: "method".to_string(),
                    qualified_name,
                    file_path: file_path.to_string(),
                    start_line: (start_point.row + 1) as i64,
                    end_line: (end_point.row + 1) as i64,
                    start_column: start_point.column as i64,
                    end_column: end_point.column as i64,
                    signature: Some(sig),
                    doc_comment: doc,
                });

                id_holder = id;
                next_caller_id = Some(&id_holder);
            }
        }
        "call_expression" => {
            if let Some(caller) = current_caller_id {
                if let Some(func_node) = node.child_by_field_name("function") {
                    let func_name = extract_go_call_target(func_node, content);
                    calls.push(raw_call(caller, func_name, node));
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        traverse_go(child, content, file_path, nodes, calls, next_caller_id);
    }
}

/// Helper to find receiver type inside Go receiver parameter list.
fn get_go_receiver_type(receiver_node: tree_sitter::Node, content: &str) -> Option<String> {
    fn find_type_id(node: tree_sitter::Node, content: &str) -> Option<String> {
        if node.kind() == "type_identifier" {
            return Some(node.utf8_text(content.as_bytes()).unwrap_or("").to_string());
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(res) = find_type_id(child, content) {
                return Some(res);
            }
        }
        None
    }
    find_type_id(receiver_node, content)
}

/// Extracts call target simple name for Go functions/methods.
fn extract_go_call_target(node: tree_sitter::Node, content: &str) -> String {
    let kind = node.kind();
    match kind {
        "identifier" => node.utf8_text(content.as_bytes()).unwrap_or("").to_string(),
        "selector_expression" => {
            if let Some(field_node) = node.child_by_field_name("field") {
                field_node
                    .utf8_text(content.as_bytes())
                    .unwrap_or("")
                    .to_string()
            } else {
                let text = node.utf8_text(content.as_bytes()).unwrap_or("");
                text.split('.').next_back().unwrap_or("").to_string()
            }
        }
        _ => node.utf8_text(content.as_bytes()).unwrap_or("").to_string(),
    }
}

/// Helper to locate child of specific tree-sitter kind.
fn find_child_by_kind<'a>(
    node: tree_sitter::Node<'a>,
    kind: &str,
) -> Option<tree_sitter::Node<'a>> {
    node.children(&mut node.walk())
        .find(|child| child.kind() == kind)
}

/// Extracts clean signature from node.
fn get_signature(node: tree_sitter::Node, content: &str) -> String {
    let mut body_node = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if kind == "block"
            || kind == "field_declaration_list"
            || kind == "declaration_list"
            || kind == "interface_type"
            || kind == "struct_type"
            || kind == "compound_statement"
            || kind == "class_body"
            || kind == "interface_body"
            || kind == "constructor_body"
        {
            body_node = Some(child);
            break;
        }
    }

    let end_byte = if let Some(body) = body_node {
        body.start_byte()
    } else {
        node.end_byte()
    };

    let text = &content[node.start_byte()..end_byte];
    let cleaned = text.trim().trim_end_matches('{').trim().to_string();
    if cleaned.is_empty() {
        let full_text = node.utf8_text(content.as_bytes()).unwrap_or("");
        full_text.lines().next().unwrap_or("").trim().to_string()
    } else {
        cleaned
    }
}

/// Extracts doc comments preceding the symbol node.
fn get_doc_comment(node: tree_sitter::Node, content: &str) -> Option<String> {
    let mut comments = Vec::new();

    fn collect_preceding_comments(n: tree_sitter::Node, content: &str, comments: &mut Vec<String>) {
        let mut curr = n.prev_sibling();
        while let Some(prev) = curr {
            let kind = prev.kind();
            if kind == "line_comment" || kind == "block_comment" || kind == "comment" {
                let text = prev.utf8_text(content.as_bytes()).unwrap_or("");
                comments.push(text.to_string());
                curr = prev.prev_sibling();
            } else if kind == "attribute_item" || kind == "inner_attribute_item" {
                curr = prev.prev_sibling();
            } else {
                break;
            }
        }
    }

    collect_preceding_comments(node, content, &mut comments);

    if comments.is_empty() {
        if let Some(parent) = node.parent() {
            if parent.kind() == "type_declaration" {
                collect_preceding_comments(parent, content, &mut comments);
            }
        }
    }

    if comments.is_empty() {
        None
    } else {
        comments.reverse();
        let cleaned = comments
            .iter()
            .map(|c| c.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        Some(cleaned)
    }
}

fn text_for_node(node: tree_sitter::Node, content: &str) -> String {
    node.utf8_text(content.as_bytes())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn last_descendant_by_kind<'a>(
    node: tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut found = if kinds.contains(&node.kind()) {
        Some(node)
    } else {
        None
    };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(candidate) = last_descendant_by_kind(child, kinds) {
            found = Some(candidate);
        }
    }

    found
}

fn normalize_qualified_name(name: &str) -> String {
    name.split_whitespace().collect::<String>()
}

fn push_symbol(
    nodes: &mut Vec<Node>,
    file_path: &str,
    name: String,
    kind: &str,
    qualified_name: String,
    node: tree_sitter::Node,
    content: &str,
) {
    let start_point = node.start_position();
    let end_point = node.end_position();
    nodes.push(Node {
        id: format!("{}::{}", file_path, qualified_name),
        name,
        kind: kind.to_string(),
        qualified_name: Some(qualified_name),
        file_path: file_path.to_string(),
        start_line: (start_point.row + 1) as i64,
        end_line: (end_point.row + 1) as i64,
        start_column: start_point.column as i64,
        end_column: end_point.column as i64,
        signature: Some(get_signature(node, content)),
        doc_comment: get_doc_comment(node, content),
    });
}

fn qualified_name(parent: Option<&str>, name: &str) -> String {
    if let Some(parent) = parent {
        format!("{}::{}", parent, name)
    } else {
        name.to_string()
    }
}

fn extract_c_declarator_name(node: tree_sitter::Node, content: &str) -> Option<String> {
    if let Some(declarator) = node.child_by_field_name("declarator") {
        return extract_c_declarator_name(declarator, content);
    }

    match node.kind() {
        "identifier"
        | "field_identifier"
        | "type_identifier"
        | "qualified_identifier"
        | "destructor_name" => Some(normalize_qualified_name(&text_for_node(node, content))),
        _ => last_descendant_by_kind(
            node,
            &[
                "qualified_identifier",
                "field_identifier",
                "type_identifier",
                "identifier",
                "destructor_name",
            ],
        )
        .map(|name_node| normalize_qualified_name(&text_for_node(name_node, content))),
    }
}

fn extract_c_call_target(node: tree_sitter::Node, content: &str) -> String {
    match node.kind() {
        "identifier" | "field_identifier" | "type_identifier" | "qualified_identifier" => {
            normalize_qualified_name(&text_for_node(node, content))
        }
        "field_expression" => node
            .child_by_field_name("field")
            .map(|field| text_for_node(field, content))
            .unwrap_or_else(|| {
                text_for_node(node, content)
                    .split('.')
                    .next_back()
                    .unwrap_or("")
                    .to_string()
            }),
        _ => extract_c_declarator_name(node, content)
            .unwrap_or_else(|| normalize_qualified_name(&text_for_node(node, content))),
    }
}

#[derive(Clone, Copy)]
struct CLikeContext<'a> {
    parent_qualified_name: Option<&'a str>,
    parent_is_type: bool,
}

fn traverse_c_like<'a>(
    node: tree_sitter::Node<'a>,
    content: &'a str,
    file_path: &str,
    nodes: &mut Vec<Node>,
    calls: &mut Vec<RawCall>,
    context: CLikeContext<'a>,
    current_caller_id: Option<&str>,
) {
    let mut next_context = context;
    let mut next_caller_id = current_caller_id;

    #[allow(unused_assignments)]
    let mut qname_holder = String::new();
    #[allow(unused_assignments)]
    let mut id_holder = String::new();

    match node.kind() {
        "namespace_definition" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = normalize_qualified_name(&text_for_node(name_node, content));
                if !name.is_empty() {
                    let qname = qualified_name(context.parent_qualified_name, &name);
                    push_symbol(
                        nodes,
                        file_path,
                        name,
                        "namespace",
                        qname.clone(),
                        node,
                        content,
                    );
                    qname_holder = qname;
                    next_context = CLikeContext {
                        parent_qualified_name: Some(&qname_holder),
                        parent_is_type: false,
                    };
                    next_caller_id = None;
                }
            }
        }
        "class_specifier" | "struct_specifier" => {
            let name_node = node
                .child_by_field_name("name")
                .or_else(|| find_child_by_kind(node, "type_identifier"))
                .or_else(|| find_child_by_kind(node, "identifier"));
            if let Some(name_node) = name_node {
                let name = normalize_qualified_name(&text_for_node(name_node, content));
                if !name.is_empty() {
                    let qname = qualified_name(context.parent_qualified_name, &name);
                    let kind = if node.kind() == "class_specifier" {
                        "class"
                    } else {
                        "struct"
                    };
                    push_symbol(nodes, file_path, name, kind, qname.clone(), node, content);
                    qname_holder = qname;
                    next_context = CLikeContext {
                        parent_qualified_name: Some(&qname_holder),
                        parent_is_type: true,
                    };
                    next_caller_id = None;
                }
            }
        }
        "function_definition" => {
            if let Some(declarator) = node.child_by_field_name("declarator") {
                if let Some(raw_name) = extract_c_declarator_name(declarator, content) {
                    if !raw_name.is_empty() {
                        let simple_name = raw_name
                            .rsplit("::")
                            .next()
                            .unwrap_or(&raw_name)
                            .trim_start_matches('~')
                            .to_string();
                        // Out-of-line definitions (`Point::sum`) carry their own
                        // path; still prefix any enclosing namespace so the qname
                        // matches the in-class symbol (e.g. `demo::Point::sum`).
                        let name_for_qual = if raw_name.contains("::") {
                            &raw_name
                        } else {
                            &simple_name
                        };
                        let qname = qualified_name(context.parent_qualified_name, name_for_qual);
                        let kind = if context.parent_is_type || raw_name.contains("::") {
                            "method"
                        } else {
                            "function"
                        };
                        push_symbol(
                            nodes,
                            file_path,
                            simple_name,
                            kind,
                            qname.clone(),
                            node,
                            content,
                        );
                        id_holder = format!("{}::{}", file_path, qname);
                        next_caller_id = Some(&id_holder);
                    }
                }
            }
        }
        "call_expression" => {
            if let Some(caller) = current_caller_id {
                if let Some(func_node) = node.child_by_field_name("function") {
                    calls.push(raw_call(
                        caller,
                        extract_c_call_target(func_node, content),
                        node,
                    ));
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        traverse_c_like(
            child,
            content,
            file_path,
            nodes,
            calls,
            next_context,
            next_caller_id,
        );
    }
}

fn zig_variable_name(node: tree_sitter::Node, content: &str) -> Option<String> {
    find_child_by_kind(node, "identifier").map(|name_node| text_for_node(name_node, content))
}

fn zig_variable_kind(node: tree_sitter::Node, content: &str) -> Option<&'static str> {
    // Only inspect the initializer (right of the first `=`) and require the
    // container keyword to lead it, so a keyword inside the variable name, a
    // string literal, or a function argument doesn't trigger a false positive.
    let text = text_for_node(node, content);
    let init = text.split_once('=').map(|(_, rhs)| rhs).unwrap_or("");
    zig_container_kind(init)
}

fn zig_container_kind(init: &str) -> Option<&'static str> {
    // Skip an optional layout qualifier before the container keyword.
    let init = init.trim_start();
    let init = init
        .strip_prefix("packed ")
        .or_else(|| init.strip_prefix("extern "))
        .unwrap_or(init)
        .trim_start();
    for (keyword, kind) in [("struct", "struct"), ("enum", "enum"), ("union", "union")] {
        if let Some(rest) = init.strip_prefix(keyword) {
            // A standalone keyword is followed by a body/tag, not more of an
            // identifier (rules out names like `structFoo`).
            if rest
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric() && c != '_')
            {
                return Some(kind);
            }
        }
    }
    None
}

fn extract_zig_call_target(node: tree_sitter::Node, content: &str) -> String {
    match node.kind() {
        "identifier" => text_for_node(node, content),
        "field_expression" => node
            .child_by_field_name("field")
            .map(|field| text_for_node(field, content))
            .unwrap_or_else(|| {
                text_for_node(node, content)
                    .split('.')
                    .next_back()
                    .unwrap_or("")
                    .to_string()
            }),
        _ => last_descendant_by_kind(node, &["identifier"])
            .map(|name_node| text_for_node(name_node, content))
            .unwrap_or_else(|| text_for_node(node, content)),
    }
}

fn traverse_zig<'a>(
    node: tree_sitter::Node<'a>,
    content: &'a str,
    file_path: &str,
    nodes: &mut Vec<Node>,
    calls: &mut Vec<RawCall>,
    current_parent_qualified_name: Option<&str>,
    current_caller_id: Option<&str>,
) {
    let mut next_parent_qualified_name = current_parent_qualified_name;
    let mut next_caller_id = current_caller_id;

    #[allow(unused_assignments)]
    let mut qname_holder = String::new();
    #[allow(unused_assignments)]
    let mut id_holder = String::new();

    match node.kind() {
        "variable_declaration" => {
            if let (Some(name), Some(kind)) = (
                zig_variable_name(node, content),
                zig_variable_kind(node, content),
            ) {
                let qname = qualified_name(current_parent_qualified_name, &name);
                push_symbol(nodes, file_path, name, kind, qname.clone(), node, content);
                qname_holder = qname;
                next_parent_qualified_name = Some(&qname_holder);
                next_caller_id = None;
            }
        }
        "function_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = text_for_node(name_node, content);
                if !name.is_empty() {
                    let qname = qualified_name(current_parent_qualified_name, &name);
                    let kind = if current_parent_qualified_name.is_some() {
                        "method"
                    } else {
                        "function"
                    };
                    push_symbol(nodes, file_path, name, kind, qname.clone(), node, content);
                    id_holder = format!("{}::{}", file_path, qname);
                    next_caller_id = Some(&id_holder);
                }
            }
        }
        "call_expression" => {
            if let Some(caller) = current_caller_id {
                if let Some(func_node) = node.child_by_field_name("function") {
                    calls.push(raw_call(
                        caller,
                        extract_zig_call_target(func_node, content),
                        node,
                    ));
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        traverse_zig(
            child,
            content,
            file_path,
            nodes,
            calls,
            next_parent_qualified_name,
            next_caller_id,
        );
    }
}

/// Recursively traverses a Java AST.
fn traverse_java<'a>(
    node: tree_sitter::Node<'a>,
    content: &'a str,
    file_path: &str,
    nodes: &mut Vec<Node>,
    calls: &mut Vec<RawCall>,
    current_parent_qualified_name: Option<&str>,
    current_caller_id: Option<&str>,
) {
    let mut next_parent_qualified_name = current_parent_qualified_name;
    let mut next_caller_id = current_caller_id;

    #[allow(unused_assignments)]
    let mut qname_holder = String::new();
    #[allow(unused_assignments)]
    let mut id_holder = String::new();

    let kind = node.kind();
    match kind {
        "class_declaration" | "interface_declaration" => {
            let name_node = node
                .child_by_field_name("name")
                .or_else(|| find_child_by_kind(node, "identifier"));
            if let Some(name_node) = name_node {
                let name = name_node
                    .utf8_text(content.as_bytes())
                    .unwrap_or("")
                    .to_string();
                let qname = if let Some(parent_qname) = current_parent_qualified_name {
                    format!("{}::{}", parent_qname, name)
                } else {
                    name.clone()
                };
                let id = format!("{}::{}", file_path, qname);
                let doc = get_doc_comment(node, content);
                let sig = get_signature(node, content);
                let start_point = node.start_position();
                let end_point = node.end_position();

                let node_kind = if kind == "class_declaration" {
                    "class"
                } else {
                    "interface"
                };

                nodes.push(Node {
                    id: id.clone(),
                    name: name.clone(),
                    kind: node_kind.to_string(),
                    qualified_name: Some(qname.clone()),
                    file_path: file_path.to_string(),
                    start_line: (start_point.row + 1) as i64,
                    end_line: (end_point.row + 1) as i64,
                    start_column: start_point.column as i64,
                    end_column: end_point.column as i64,
                    signature: Some(sig),
                    doc_comment: doc,
                });

                qname_holder = qname;
                next_parent_qualified_name = Some(&qname_holder);
                next_caller_id = None;
            }
        }
        "method_declaration" | "constructor_declaration" => {
            let name_node = node
                .child_by_field_name("name")
                .or_else(|| find_child_by_kind(node, "identifier"));
            if let Some(name_node) = name_node {
                let name = name_node
                    .utf8_text(content.as_bytes())
                    .unwrap_or("")
                    .to_string();
                let qname = if let Some(parent_qname) = current_parent_qualified_name {
                    format!("{}::{}", parent_qname, name)
                } else {
                    name.clone()
                };
                let id = format!("{}::{}", file_path, qname);
                let doc = get_doc_comment(node, content);
                let sig = get_signature(node, content);
                let start_point = node.start_position();
                let end_point = node.end_position();

                let node_kind = if kind == "method_declaration" {
                    "method"
                } else {
                    "constructor"
                };

                nodes.push(Node {
                    id: id.clone(),
                    name: name.clone(),
                    kind: node_kind.to_string(),
                    qualified_name: Some(qname),
                    file_path: file_path.to_string(),
                    start_line: (start_point.row + 1) as i64,
                    end_line: (end_point.row + 1) as i64,
                    start_column: start_point.column as i64,
                    end_column: end_point.column as i64,
                    signature: Some(sig),
                    doc_comment: doc,
                });

                id_holder = id;
                next_caller_id = Some(&id_holder);
            }
        }
        "method_invocation" => {
            if let Some(caller) = current_caller_id {
                if let Some(name_node) = node.child_by_field_name("name") {
                    let method_name = name_node
                        .utf8_text(content.as_bytes())
                        .unwrap_or("")
                        .to_string();
                    calls.push(raw_call(caller, method_name, node));
                }
            }
        }
        "object_creation_expression" => {
            if let Some(caller) = current_caller_id {
                if let Some(type_node) = node.child_by_field_name("type") {
                    let type_name = type_node
                        .utf8_text(content.as_bytes())
                        .unwrap_or("")
                        .to_string();
                    calls.push(raw_call(caller, type_name, node));
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        traverse_java(
            child,
            content,
            file_path,
            nodes,
            calls,
            next_parent_qualified_name,
            next_caller_id,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rust_code() {
        let rust_code = r#"
/// A simple Rust struct.
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// Point implementation.
impl Point {
    /// Create a new point.
    pub fn new(x: i32, y: i32) -> Self {
        Point { x, y }
    }

    /// Retrieve sum of coordinates.
    pub fn sum(&self) -> i32 {
        self.helper()
    }

    fn helper(&self) -> i32 {
        self.x + self.y
    }
}

/// Enum for directions.
pub enum Direction {
    Up,
    Down,
}

/// Trait for displaying point information.
pub trait Printable {
    fn print(&self);
}

fn test_free_fn() {
    let p = Point::new(1, 2);
    p.sum();
}
"#;

        let (nodes, calls) = parse_code("src/point.rs", rust_code, "rust").unwrap();
        let edges = resolve_calls_local(&nodes, &calls);

        // Let's assert nodes
        let struct_node = nodes
            .iter()
            .find(|n| n.name == "Point" && n.kind == "struct")
            .unwrap();
        assert_eq!(struct_node.id, "src/point.rs::Point");
        assert_eq!(
            struct_node.doc_comment.as_deref(),
            Some("/// A simple Rust struct.")
        );
        assert_eq!(struct_node.signature.as_deref(), Some("pub struct Point"));

        let enum_node = nodes
            .iter()
            .find(|n| n.name == "Direction" && n.kind == "enum")
            .unwrap();
        assert_eq!(enum_node.id, "src/point.rs::Direction");
        assert_eq!(
            enum_node.doc_comment.as_deref(),
            Some("/// Enum for directions.")
        );

        let trait_node = nodes
            .iter()
            .find(|n| n.name == "Printable" && n.kind == "trait")
            .unwrap();
        assert_eq!(trait_node.id, "src/point.rs::Printable");
        assert_eq!(
            trait_node.doc_comment.as_deref(),
            Some("/// Trait for displaying point information.")
        );

        // Methods within impl block
        let new_method = nodes
            .iter()
            .find(|n| n.name == "new" && n.kind == "method")
            .unwrap();
        assert_eq!(new_method.id, "src/point.rs::Point::new");
        assert_eq!(
            new_method.doc_comment.as_deref(),
            Some("/// Create a new point.")
        );
        assert_eq!(
            new_method.signature.as_deref(),
            Some("pub fn new(x: i32, y: i32) -> Self")
        );

        let sum_method = nodes
            .iter()
            .find(|n| n.name == "sum" && n.kind == "method")
            .unwrap();
        assert_eq!(sum_method.id, "src/point.rs::Point::sum");
        assert_eq!(
            sum_method.doc_comment.as_deref(),
            Some("/// Retrieve sum of coordinates.")
        );

        let helper_method = nodes
            .iter()
            .find(|n| n.name == "helper" && n.kind == "method")
            .unwrap();
        assert_eq!(helper_method.id, "src/point.rs::Point::helper");

        let free_fn = nodes
            .iter()
            .find(|n| n.name == "test_free_fn" && n.kind == "function")
            .unwrap();
        assert_eq!(free_fn.id, "src/point.rs::test_free_fn");

        // Edges: sum calls helper
        let edge_sum_helper = edges
            .iter()
            .find(|e| e.source_id == sum_method.id && e.target_id == helper_method.id)
            .unwrap();
        assert_eq!(edge_sum_helper.kind, "calls");

        // Edges: test_free_fn calls Point::new (via new) and sum
        let edge_free_new = edges
            .iter()
            .find(|e| e.source_id == free_fn.id && e.target_id == new_method.id)
            .unwrap();
        assert_eq!(edge_free_new.kind, "calls");

        let edge_free_sum = edges
            .iter()
            .find(|e| e.source_id == free_fn.id && e.target_id == sum_method.id)
            .unwrap();
        assert_eq!(edge_free_sum.kind, "calls");
    }

    #[test]
    fn test_resolve_calls_global_prefers_explicit_namespace() {
        let nodes = vec![
            Node {
                id: "src/shapes.rs::Point::new".to_string(),
                name: "new".to_string(),
                kind: "method".to_string(),
                qualified_name: Some("Point::new".to_string()),
                file_path: "src/shapes.rs".to_string(),
                start_line: 1,
                end_line: 1,
                start_column: 0,
                end_column: 0,
                signature: None,
                doc_comment: None,
            },
            Node {
                id: "src/shapes.rs::Line::new".to_string(),
                name: "new".to_string(),
                kind: "method".to_string(),
                qualified_name: Some("Line::new".to_string()),
                file_path: "src/shapes.rs".to_string(),
                start_line: 2,
                end_line: 2,
                start_column: 0,
                end_column: 0,
                signature: None,
                doc_comment: None,
            },
            Node {
                id: "src/shapes.rs::build".to_string(),
                name: "build".to_string(),
                kind: "function".to_string(),
                qualified_name: Some("build".to_string()),
                file_path: "src/shapes.rs".to_string(),
                start_line: 3,
                end_line: 3,
                start_column: 0,
                end_column: 0,
                signature: None,
                doc_comment: None,
            },
        ];
        let calls = vec![RawCall {
            caller_id: "src/shapes.rs::build".to_string(),
            callee_name: "Line::new".to_string(),
            line: 3,
            column: 4,
        }];

        let edges = resolve_calls_local(&nodes, &calls);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source_id, "src/shapes.rs::build");
        assert_eq!(edges[0].target_id, "src/shapes.rs::Line::new");
    }

    #[test]
    fn test_parse_go_code() {
        let go_code = r#"
package geometry

// Point represents a 2D point.
type Point struct {
	X, Y int
}

// Printer is an interface.
type Printer interface {
	Print()
}

// NewPoint creates a new point.
func NewPoint(x, y int) *Point {
	return &Point{X: x, Y: y}
}

// Distance calculates distance.
func (p *Point) Distance() float64 {
	p.helper()
	return 0.0
}

func (p *Point) helper() {
	// nested helper
}
"#;

        let (nodes, calls) = parse_code("geometry.go", go_code, "go").unwrap();
        let edges = resolve_calls_local(&nodes, &calls);

        // Assert nodes
        let struct_node = nodes
            .iter()
            .find(|n| n.name == "Point" && n.kind == "struct")
            .unwrap();
        assert_eq!(struct_node.id, "geometry.go::Point");
        assert_eq!(
            struct_node.doc_comment.as_deref(),
            Some("// Point represents a 2D point.")
        );
        assert_eq!(struct_node.signature.as_deref(), Some("type Point"));

        let interface_node = nodes
            .iter()
            .find(|n| n.name == "Printer" && n.kind == "interface")
            .unwrap();
        assert_eq!(interface_node.id, "geometry.go::Printer");
        assert_eq!(
            interface_node.doc_comment.as_deref(),
            Some("// Printer is an interface.")
        );

        let func_node = nodes
            .iter()
            .find(|n| n.name == "NewPoint" && n.kind == "function")
            .unwrap();
        assert_eq!(func_node.id, "geometry.go::NewPoint");
        assert_eq!(
            func_node.doc_comment.as_deref(),
            Some("// NewPoint creates a new point.")
        );
        assert_eq!(
            func_node.signature.as_deref(),
            Some("func NewPoint(x, y int) *Point")
        );

        let method_node = nodes
            .iter()
            .find(|n| n.name == "Distance" && n.kind == "method")
            .unwrap();
        assert_eq!(method_node.id, "geometry.go::Point::Distance");
        assert_eq!(
            method_node.doc_comment.as_deref(),
            Some("// Distance calculates distance.")
        );
        assert_eq!(
            method_node.signature.as_deref(),
            Some("func (p *Point) Distance() float64")
        );

        let helper_method = nodes
            .iter()
            .find(|n| n.name == "helper" && n.kind == "method")
            .unwrap();
        assert_eq!(helper_method.id, "geometry.go::Point::helper");

        // Edges: Distance calls helper
        let edge_call = edges
            .iter()
            .find(|e| e.source_id == method_node.id && e.target_id == helper_method.id)
            .unwrap();
        assert_eq!(edge_call.kind, "calls");
    }

    #[test]
    fn test_parse_java_code() {
        let java_code = r#"
/**
 * A sample Java class.
 */
public class App {
    private String name;

    /**
     * Constructor.
     */
    public App(String name) {
        this.name = name;
        init();
    }

    /**
     * Init method.
     */
    public void init() {
        // init
    }

    /**
     * Run method.
     */
    public void run() {
        System.out.println("Run");
    }

    /**
     * Main entry point.
     */
    public static void main(String[] args) {
        App app = new App("Demo");
        app.run();
    }
}
"#;

        let (nodes, calls) = parse_code("src/App.java", java_code, "java").unwrap();
        let edges = resolve_calls_local(&nodes, &calls);

        // Find class node
        let class_node = nodes
            .iter()
            .find(|n| n.name == "App" && n.kind == "class")
            .unwrap();
        assert_eq!(class_node.id, "src/App.java::App");
        assert_eq!(
            class_node.doc_comment.as_deref(),
            Some("/**\n * A sample Java class.\n */")
        );
        assert_eq!(class_node.signature.as_deref(), Some("public class App"));

        // Find constructor node
        let constr_node = nodes
            .iter()
            .find(|n| n.name == "App" && n.kind == "constructor")
            .unwrap();
        assert_eq!(constr_node.id, "src/App.java::App::App");
        assert_eq!(
            constr_node.doc_comment.as_deref(),
            Some("/**\n     * Constructor.\n     */")
        );
        assert_eq!(
            constr_node.signature.as_deref(),
            Some("public App(String name)")
        );

        // Find init method
        let init_method = nodes
            .iter()
            .find(|n| n.name == "init" && n.kind == "method")
            .unwrap();
        assert_eq!(init_method.id, "src/App.java::App::init");
        assert_eq!(
            init_method.doc_comment.as_deref(),
            Some("/**\n     * Init method.\n     */")
        );
        assert_eq!(init_method.signature.as_deref(), Some("public void init()"));

        // Find run method
        let run_method = nodes
            .iter()
            .find(|n| n.name == "run" && n.kind == "method")
            .unwrap();
        assert_eq!(run_method.id, "src/App.java::App::run");

        // Find main method
        let main_method = nodes
            .iter()
            .find(|n| n.name == "main" && n.kind == "method")
            .unwrap();
        assert_eq!(main_method.id, "src/App.java::App::main");

        // Check call edges
        // Constructor App calls init
        let edge_constr_init = edges
            .iter()
            .find(|e| e.source_id == constr_node.id && e.target_id == init_method.id)
            .unwrap();
        assert_eq!(edge_constr_init.kind, "calls");

        // main calls constructor (via new App)
        let edge_main_constr = edges
            .iter()
            .find(|e| e.source_id == main_method.id && e.target_id == constr_node.id)
            .unwrap();
        assert_eq!(edge_main_constr.kind, "calls");

        // main calls run method (via app.run)
        let edge_main_run = edges
            .iter()
            .find(|e| e.source_id == main_method.id && e.target_id == run_method.id)
            .unwrap();
        assert_eq!(edge_main_run.kind, "calls");
    }

    #[test]
    fn test_parse_c_code() {
        let c_code = r#"
// Point struct.
struct Point {
    int x;
    int y;
};

// Helper function.
int helper(int value) {
    return value + 1;
}

// Main entry point.
int main(void) {
    return helper(41);
}
"#;

        let (nodes, calls) = parse_code("src/main.c", c_code, "c").unwrap();
        let edges = resolve_calls_local(&nodes, &calls);

        let struct_node = nodes
            .iter()
            .find(|n| n.name == "Point" && n.kind == "struct")
            .unwrap();
        assert_eq!(struct_node.id, "src/main.c::Point");
        assert_eq!(struct_node.doc_comment.as_deref(), Some("// Point struct."));

        let helper = nodes
            .iter()
            .find(|n| n.name == "helper" && n.kind == "function")
            .unwrap();
        assert_eq!(helper.id, "src/main.c::helper");
        assert_eq!(helper.doc_comment.as_deref(), Some("// Helper function."));

        let main = nodes
            .iter()
            .find(|n| n.name == "main" && n.kind == "function")
            .unwrap();
        assert_eq!(main.id, "src/main.c::main");

        let edge_main_helper = edges
            .iter()
            .find(|e| e.source_id == main.id && e.target_id == helper.id)
            .unwrap();
        assert_eq!(edge_main_helper.kind, "calls");
    }

    #[test]
    fn test_parse_cpp_code() {
        let cpp_code = r#"
namespace demo {
// Point class.
class Point {
public:
    int sum() {
        return helper();
    }

    int helper() {
        return 42;
    }
};

int run() {
    Point point;
    return point.sum();
}
}
"#;

        let (nodes, calls) = parse_code("src/point.cpp", cpp_code, "cpp").unwrap();
        let edges = resolve_calls_local(&nodes, &calls);

        let namespace = nodes
            .iter()
            .find(|n| n.name == "demo" && n.kind == "namespace")
            .unwrap();
        assert_eq!(namespace.id, "src/point.cpp::demo");

        let class_node = nodes
            .iter()
            .find(|n| n.name == "Point" && n.kind == "class")
            .unwrap();
        assert_eq!(class_node.id, "src/point.cpp::demo::Point");
        assert_eq!(class_node.doc_comment.as_deref(), Some("// Point class."));

        let sum_method = nodes
            .iter()
            .find(|n| n.name == "sum" && n.kind == "method")
            .unwrap();
        assert_eq!(sum_method.id, "src/point.cpp::demo::Point::sum");

        let helper_method = nodes
            .iter()
            .find(|n| n.name == "helper" && n.kind == "method")
            .unwrap();
        assert_eq!(helper_method.id, "src/point.cpp::demo::Point::helper");

        let run = nodes
            .iter()
            .find(|n| n.name == "run" && n.kind == "function")
            .unwrap();
        assert_eq!(run.id, "src/point.cpp::demo::run");

        let edge_sum_helper = edges
            .iter()
            .find(|e| e.source_id == sum_method.id && e.target_id == helper_method.id)
            .unwrap();
        assert_eq!(edge_sum_helper.kind, "calls");

        let edge_run_sum = edges
            .iter()
            .find(|e| e.source_id == run.id && e.target_id == sum_method.id)
            .unwrap();
        assert_eq!(edge_run_sum.kind, "calls");
    }

    #[test]
    fn test_parse_zig_code() {
        let zig_code = r#"
/// Point type.
const Point = struct {
    x: i32,
    y: i32,

    /// Sum fields.
    fn sum(self: Point) i32 {
        return self.helper();
    }

    fn helper(self: Point) i32 {
        return self.x + self.y;
    }
};

fn makePoint() Point {
    return Point{ .x = 1, .y = 2 };
}

pub fn main() void {
    const point = makePoint();
    _ = point.sum();
}
"#;

        let (nodes, calls) = parse_code("src/main.zig", zig_code, "zig").unwrap();
        let edges = resolve_calls_local(&nodes, &calls);

        let struct_node = nodes
            .iter()
            .find(|n| n.name == "Point" && n.kind == "struct")
            .unwrap();
        assert_eq!(struct_node.id, "src/main.zig::Point");
        assert_eq!(struct_node.doc_comment.as_deref(), Some("/// Point type."));

        let sum_method = nodes
            .iter()
            .find(|n| n.name == "sum" && n.kind == "method")
            .unwrap();
        assert_eq!(sum_method.id, "src/main.zig::Point::sum");
        assert_eq!(sum_method.doc_comment.as_deref(), Some("/// Sum fields."));

        let helper_method = nodes
            .iter()
            .find(|n| n.name == "helper" && n.kind == "method")
            .unwrap();
        assert_eq!(helper_method.id, "src/main.zig::Point::helper");

        let make_point = nodes
            .iter()
            .find(|n| n.name == "makePoint" && n.kind == "function")
            .unwrap();
        assert_eq!(make_point.id, "src/main.zig::makePoint");

        let main = nodes
            .iter()
            .find(|n| n.name == "main" && n.kind == "function")
            .unwrap();
        assert_eq!(main.id, "src/main.zig::main");

        let edge_sum_helper = edges
            .iter()
            .find(|e| e.source_id == sum_method.id && e.target_id == helper_method.id)
            .unwrap();
        assert_eq!(edge_sum_helper.kind, "calls");

        let edge_main_make_point = edges
            .iter()
            .find(|e| e.source_id == main.id && e.target_id == make_point.id)
            .unwrap();
        assert_eq!(edge_main_make_point.kind, "calls");

        let edge_main_sum = edges
            .iter()
            .find(|e| e.source_id == main.id && e.target_id == sum_method.id)
            .unwrap();
        assert_eq!(edge_main_sum.kind, "calls");
    }

    #[test]
    fn test_parse_cpp_out_of_line_method() {
        // An out-of-line method defined inside its namespace should inherit the
        // namespace prefix so it matches the in-class declaration's qname.
        let cpp_code = r#"
namespace demo {
class Point {
public:
    int sum();
};

int Point::sum() {
    return 42;
}
}
"#;

        let (nodes, _calls) = parse_code("src/point.cpp", cpp_code, "cpp").unwrap();

        let sum_method = nodes
            .iter()
            .find(|n| n.name == "sum" && n.kind == "method")
            .unwrap();
        assert_eq!(sum_method.id, "src/point.cpp::demo::Point::sum");
    }

    #[test]
    fn test_parse_zig_keyword_in_string_is_not_a_type() {
        // The container-kind heuristic must not treat a string literal (or a
        // plain value) containing "struct"/"enum"/"union" as a type symbol.
        let zig_code = r#"
const message = "this mentions a struct and an enum";
const Real = struct {
    x: i32,
};
"#;

        let (nodes, _calls) = parse_code("src/main.zig", zig_code, "zig").unwrap();

        assert!(
            nodes.iter().all(|n| n.name != "message"),
            "string-valued const should not be indexed as a type"
        );
        assert!(
            nodes.iter().any(|n| n.name == "Real" && n.kind == "struct"),
            "genuine struct declaration should still be indexed"
        );
    }
}
