use crate::db::{Edge, Node, RawCall, UnresolvedRef};
use std::collections::HashMap;
use std::error::Error;
use tree_sitter::Parser;

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

/// Parse Rust, Go, or Java source and extract nodes (symbols) plus the raw,
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

    if lang == "rust" || lang == "rs" {
        parser.set_language(&tree_sitter_rust::language())?;
    } else if lang == "go" {
        parser.set_language(&tree_sitter_go::language())?;
    } else if lang == "java" {
        parser.set_language(&tree_sitter_java::language())?;
    } else {
        return Err(format!("Unsupported language: {}", language).into());
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
    }

    Ok((nodes, raw_calls))
}

/// Resolve raw call sites into concrete edges against a project-wide symbol
/// index, preferring a target in the caller's own file before falling back to a
/// unique match anywhere in the project. Calls that match no known symbol are
/// returned separately as [`UnresolvedRef`]s.
///
/// - `name_to_ids` maps a symbol's simple name to every node id that bears it.
/// - `id_to_file` maps a node id to the file it is defined in.
pub fn resolve_calls_global(
    calls: &[RawCall],
    name_to_ids: &HashMap<String, Vec<String>>,
    id_to_file: &HashMap<String, String>,
) -> (Vec<Edge>, Vec<UnresolvedRef>) {
    let mut edges = Vec::new();
    let mut unresolved = Vec::new();

    for call in calls {
        let name_parts: Vec<&str> = call.callee_name.split("::").collect();
        let simple_name = name_parts.last().copied().unwrap_or(&call.callee_name);

        let candidates = match name_to_ids.get(simple_name) {
            Some(ids) if !ids.is_empty() => ids,
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
        let caller_file = id_to_file.get(&call.caller_id);
        let same_file: Vec<&String> = candidates
            .iter()
            .filter(|id| id_to_file.get(*id) == caller_file && caller_file.is_some())
            .collect();
        let working_set: Vec<&String> = if !same_file.is_empty() {
            same_file
        } else {
            candidates.iter().collect()
        };

        let target_id = if working_set.len() == 1 {
            working_set[0].clone()
        } else {
            disambiguate(&working_set, &name_parts, &call.caller_id)
                .unwrap_or_else(|| working_set[0].clone())
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
fn disambiguate(candidates: &[&String], name_parts: &[&str], caller_id: &str) -> Option<String> {
    // Case A: explicit namespace on the call target.
    if name_parts.len() >= 2 {
        let ns_infix = format!("::{}::", name_parts[name_parts.len() - 2]);
        if let Some(m) = candidates.iter().find(|id| id.contains(&ns_infix)) {
            return Some((*m).clone());
        }
    }

    // Case B: caller is a method/constructor — prefer a target on the same receiver.
    let caller_parts: Vec<&str> = caller_id.split("::").collect();
    if caller_parts.len() >= 3 {
        let struct_prefix = format!("::{}::", caller_parts[caller_parts.len() - 2]);
        if let Some(m) = candidates.iter().find(|id| id.contains(&struct_prefix)) {
            return Some((*m).clone());
        }
    }

    None
}

/// Resolve call sites against only the given `nodes` (single-file scope). A thin
/// wrapper over [`resolve_calls_global`] used in tests and isolated parsing.
pub fn resolve_calls_local(nodes: &[Node], calls: &[RawCall]) -> Vec<Edge> {
    let mut name_to_ids: HashMap<String, Vec<String>> = HashMap::new();
    let mut id_to_file: HashMap<String, String> = HashMap::new();
    for n in nodes {
        name_to_ids
            .entry(n.name.clone())
            .or_default()
            .push(n.id.clone());
        id_to_file.insert(n.id.clone(), n.file_path.clone());
    }
    resolve_calls_global(calls, &name_to_ids, &id_to_file).0
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

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
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

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            traverse_go(child, content, file_path, nodes, calls, next_caller_id);
        }
    }
}

/// Helper to find receiver type inside Go receiver parameter list.
fn get_go_receiver_type(receiver_node: tree_sitter::Node, content: &str) -> Option<String> {
    fn find_type_id(node: tree_sitter::Node, content: &str) -> Option<String> {
        if node.kind() == "type_identifier" {
            return Some(node.utf8_text(content.as_bytes()).unwrap_or("").to_string());
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                if let Some(res) = find_type_id(child, content) {
                    return Some(res);
                }
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
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == kind {
                return Some(child);
            }
        }
    }
    None
}

/// Extracts clean signature from node.
fn get_signature(node: tree_sitter::Node, content: &str) -> String {
    let mut body_node = None;
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        let kind = child.kind();
        if kind == "block"
            || kind == "field_declaration_list"
            || kind == "declaration_list"
            || kind == "interface_type"
            || kind == "struct_type"
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

    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
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
}
