//! Go AST traversal: type declarations (structs/interfaces), functions, methods
//! with receiver types, and call sites.

use super::common::{find_child_by_kind, get_doc_comment, get_signature, raw_call};
use crate::db::{Node, RawCall};

/// Recursively traverses a Go AST.
#[allow(clippy::too_many_arguments)]
pub(super) fn traverse_go<'a>(
    node: tree_sitter::Node<'a>,
    content: &'a str,
    file_path: &str,
    nodes: &mut Vec<Node>,
    calls: &mut Vec<RawCall>,
    current_caller_id: Option<&str>,
    package_name: Option<&str>,
    imports: &[(Option<String>, String)],
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
                            is_test: false,
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
                    is_test: false,
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
                    is_test: false,
                });

                id_holder = id;
                next_caller_id = Some(&id_holder);
            }
        }
        "call_expression" => {
            if let Some(caller) = current_caller_id {
                if let Some(func_node) = node.child_by_field_name("function") {
                    let func_name = extract_go_call_target(func_node, content);
                    let mut call = raw_call(caller, func_name, node);
                    call.package_or_namespace = package_name.map(|s| s.to_string());

                    if func_node.kind() == "selector_expression" {
                        call.call_kind = Some("method".to_string());
                        if let Some(operand_node) = func_node.child_by_field_name("operand") {
                            let receiver = operand_node
                                .utf8_text(content.as_bytes())
                                .unwrap_or("")
                                .trim()
                                .to_string();
                            if !receiver.is_empty() {
                                call.receiver_expr = Some(receiver.clone());
                                if let Some(hint) = find_go_import_hint(&receiver, imports) {
                                    call.import_hint = Some(hint);
                                }
                            }
                        }
                    } else if func_node.kind() == "identifier" {
                        call.call_kind = Some("function".to_string());
                    }
                    calls.push(call);
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        traverse_go(
            child,
            content,
            file_path,
            nodes,
            calls,
            next_caller_id,
            package_name,
            imports,
        );
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

pub(super) fn collect_go_imports(
    root: tree_sitter::Node,
    content: &str,
) -> Vec<(Option<String>, String)> {
    let mut imports = Vec::new();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "import_declaration" {
            let mut spec_cursor = child.walk();
            for spec_child in child.children(&mut spec_cursor) {
                if spec_child.kind() == "import_spec" {
                    add_import_spec(spec_child, content, &mut imports);
                } else if spec_child.kind() == "import_spec_list" {
                    let mut list_cursor = spec_child.walk();
                    for list_child in spec_child.children(&mut list_cursor) {
                        if list_child.kind() == "import_spec" {
                            add_import_spec(list_child, content, &mut imports);
                        }
                    }
                }
            }
        }
    }
    imports
}

fn add_import_spec(
    node: tree_sitter::Node,
    content: &str,
    imports: &mut Vec<(Option<String>, String)>,
) {
    let alias = node.child_by_field_name("name").map(|n| {
        n.utf8_text(content.as_bytes())
            .unwrap_or("")
            .trim()
            .to_string()
    });
    if let Some(path_node) = node.child_by_field_name("path") {
        let path_str = path_node
            .utf8_text(content.as_bytes())
            .unwrap_or("")
            .trim()
            .trim_matches('"')
            .to_string();
        imports.push((alias, path_str));
    }
}

fn find_go_import_hint(name: &str, imports: &[(Option<String>, String)]) -> Option<String> {
    for (alias, path) in imports {
        if let Some(alias_str) = alias {
            if alias_str == name {
                return Some(path.clone());
            }
        } else {
            if let Some(last_seg) = path.split('/').next_back() {
                if last_seg == name {
                    return Some(path.clone());
                }
            }
        }
    }
    None
}
