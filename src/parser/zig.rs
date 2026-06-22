//! Zig AST traversal: container-typed consts (struct/enum/union), functions and
//! methods, and call sites.

use super::common::{
    find_child_by_kind, last_descendant_by_kind, push_symbol, qualified_name, raw_call,
    text_for_node,
};
use crate::db::{Node, RawCall};

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

pub(super) fn traverse_zig<'a>(
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
