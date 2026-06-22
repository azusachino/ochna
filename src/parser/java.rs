//! Java AST traversal: classes/interfaces, methods/constructors, method
//! invocations, and object-creation (constructor call) sites.

use super::common::{find_child_by_kind, get_doc_comment, get_signature, raw_call};
use crate::db::{Node, RawCall};

/// Recursively traverses a Java AST.
pub(super) fn traverse_java<'a>(
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
