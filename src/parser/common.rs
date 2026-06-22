//! Tree-sitter helpers shared across the per-language traversals: node lookup,
//! signature/doc extraction, name normalization, and symbol/raw-call builders.

use crate::db::{Node, RawCall};

/// Build a [`RawCall`] from a caller id, callee name, and the call-site AST node
/// (whose start position records where the call occurs).
pub(super) fn raw_call(
    caller_id: &str,
    callee_name: String,
    call_node: tree_sitter::Node,
) -> RawCall {
    let pos = call_node.start_position();
    RawCall::new(
        caller_id.to_string(),
        callee_name,
        (pos.row + 1) as i64,
        pos.column as i64,
    )
}

/// Helper to locate child of specific tree-sitter kind.
pub(super) fn find_child_by_kind<'a>(
    node: tree_sitter::Node<'a>,
    kind: &str,
) -> Option<tree_sitter::Node<'a>> {
    node.children(&mut node.walk())
        .find(|child| child.kind() == kind)
}

/// Extracts clean signature from node.
pub(super) fn get_signature(node: tree_sitter::Node, content: &str) -> String {
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
pub(super) fn get_doc_comment(node: tree_sitter::Node, content: &str) -> Option<String> {
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

pub(super) fn text_for_node(node: tree_sitter::Node, content: &str) -> String {
    node.utf8_text(content.as_bytes())
        .unwrap_or("")
        .trim()
        .to_string()
}

pub(super) fn last_descendant_by_kind<'a>(
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

pub(super) fn normalize_qualified_name(name: &str) -> String {
    name.split_whitespace().collect::<String>()
}

pub(super) fn push_symbol(
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
        is_test: false,
    });
}

pub(super) fn qualified_name(parent: Option<&str>, name: &str) -> String {
    if let Some(parent) = parent {
        format!("{}::{}", parent, name)
    } else {
        name.to_string()
    }
}
