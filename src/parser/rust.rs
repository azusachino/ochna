//! Rust AST traversal: structs, enums, traits, impl methods, free functions,
//! and call/method-call sites.

use super::common::{find_child_by_kind, get_doc_comment, get_signature, raw_call};
use crate::db::{Node, RawCall};

/// Recursively traverses a Rust AST.
pub(super) fn traverse_rust<'a>(
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
                    is_test: false,
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
                    is_test: false,
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
                    is_test: false,
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
                    is_test: false,
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
