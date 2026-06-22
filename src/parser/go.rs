//! Go AST traversal: type declarations (structs/interfaces), functions, methods
//! with receiver types, and call sites.

use super::common::{find_child_by_kind, get_doc_comment, get_signature, raw_call};
use crate::db::{Node, RawCall};

/// Recursively traverses a Go AST.
pub(super) fn traverse_go<'a>(
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
