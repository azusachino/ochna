//! Java AST traversal: classes/interfaces, methods/constructors, method
//! invocations, and object-creation (constructor call) sites.

use super::common::{find_child_by_kind, get_doc_comment, get_signature, raw_call};
use crate::db::{Node, RawCall};

/// Recursively traverses a Java AST.
#[allow(clippy::too_many_arguments)]
pub(super) fn traverse_java<'a>(
    node: tree_sitter::Node<'a>,
    content: &'a str,
    file_path: &str,
    nodes: &mut Vec<Node>,
    calls: &mut Vec<RawCall>,
    current_parent_qualified_name: Option<&str>,
    current_caller_id: Option<&str>,
    current_controller_prefixes: Option<&[String]>,
) {
    let mut next_parent_qualified_name = current_parent_qualified_name;
    let mut next_caller_id = current_caller_id;
    let mut next_controller_prefixes = current_controller_prefixes;

    #[allow(unused_assignments)]
    let mut qname_holder = String::new();
    #[allow(unused_assignments)]
    let mut id_holder = String::new();
    #[allow(unused_assignments)]
    let mut class_prefixes_holder = Vec::new();

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
                    is_test: false,
                });

                qname_holder = qname;
                next_parent_qualified_name = Some(&qname_holder);
                next_caller_id = None;

                // Extract controller info
                let mut is_controller = false;
                let mut prefixes = Vec::new();
                if kind == "class_declaration" {
                    let annotations = find_annotations(node);
                    for ann in annotations {
                        let ann_name = find_child_by_kind(ann, "identifier")
                            .map(|n| n.utf8_text(content.as_bytes()).unwrap_or(""))
                            .unwrap_or("");
                        if ann_name == "RestController" || ann_name == "Controller" {
                            is_controller = true;
                        }
                        if ann_name == "RequestMapping" {
                            let extracted = extract_annotation_paths(ann, content);
                            if !extracted.is_empty() {
                                prefixes.extend(extracted);
                            }
                        }
                    }
                }

                if is_controller {
                    if prefixes.is_empty() {
                        prefixes.push("".to_string());
                    }
                    class_prefixes_holder = prefixes;
                    next_controller_prefixes = Some(&class_prefixes_holder);
                }
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
                    qualified_name: Some(qname.clone()),
                    file_path: file_path.to_string(),
                    start_line: (start_point.row + 1) as i64,
                    end_line: (end_point.row + 1) as i64,
                    start_column: start_point.column as i64,
                    end_column: end_point.column as i64,
                    signature: Some(sig.clone()),
                    doc_comment: doc.clone(),
                    is_test: false,
                });

                id_holder = id;
                next_caller_id = Some(&id_holder);

                // If it is a method declaration in a controller class, check routing annotations
                if kind == "method_declaration" {
                    if let Some(prefixes) = current_controller_prefixes {
                        let annotations = find_annotations(node);
                        for ann in annotations {
                            let ann_name = find_child_by_kind(ann, "identifier")
                                .map(|n| n.utf8_text(content.as_bytes()).unwrap_or(""))
                                .unwrap_or("");

                            if ann_name == "RequestMapping"
                                || ann_name == "GetMapping"
                                || ann_name == "PostMapping"
                                || ann_name == "PutMapping"
                                || ann_name == "DeleteMapping"
                                || ann_name == "PatchMapping"
                            {
                                let method_paths = extract_annotation_paths(ann, content);
                                let http_methods = extract_request_methods(ann, content);

                                let paths_to_combine = if method_paths.is_empty() {
                                    vec!["".to_string()]
                                } else {
                                    method_paths
                                };

                                for class_prefix in prefixes {
                                    for method_path in &paths_to_combine {
                                        let combined_path =
                                            combine_paths(class_prefix, method_path);
                                        for http_method in &http_methods {
                                            let route_id =
                                                format!("route:{}:{}", http_method, combined_path);
                                            let route_name =
                                                format!("{} {}", http_method, combined_path);

                                            nodes.push(Node {
                                                id: route_id.clone(),
                                                name: route_name.clone(),
                                                kind: "route".to_string(),
                                                qualified_name: Some(route_name),
                                                file_path: file_path.to_string(),
                                                start_line: (start_point.row + 1) as i64,
                                                end_line: (end_point.row + 1) as i64,
                                                start_column: start_point.column as i64,
                                                end_column: end_point.column as i64,
                                                signature: Some(sig.clone()),
                                                doc_comment: doc.clone(),
                                                is_test: false,
                                            });

                                            // Raw call linking the route node (as caller) to the method (as callee)
                                            let rcall = raw_call(&route_id, qname.clone(), node);
                                            calls.push(rcall);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
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
            next_controller_prefixes,
        );
    }
}

fn find_annotations<'a>(node: tree_sitter::Node<'a>) -> Vec<tree_sitter::Node<'a>> {
    let mut annotations = Vec::new();

    // Find a child whose kind is "modifiers"
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            let mut mod_cursor = child.walk();
            for mod_child in child.children(&mut mod_cursor) {
                if mod_child.kind() == "annotation" || mod_child.kind() == "marker_annotation" {
                    annotations.push(mod_child);
                }
            }
        }
        if child.kind() == "annotation" || child.kind() == "marker_annotation" {
            annotations.push(child);
        }
    }

    annotations.sort_by_key(|n| n.id());
    annotations.dedup_by_key(|n| n.id());
    annotations
}

fn extract_annotation_paths(annotation_node: tree_sitter::Node, content: &str) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(arg_list) = find_child_by_kind(annotation_node, "annotation_argument_list") {
        let mut cursor = arg_list.walk();
        for child in arg_list.children(&mut cursor) {
            match child.kind() {
                "string_literal" => {
                    if let Some(s) = extract_string_literal_value(child, content) {
                        paths.push(s);
                    }
                }
                "element_value_pair" => {
                    let key = find_child_by_kind(child, "identifier")
                        .map(|k| k.utf8_text(content.as_bytes()).unwrap_or(""))
                        .unwrap_or("");
                    if key == "value" || key == "path" {
                        if let Some(val_node) = find_element_value_pair_value(child) {
                            match val_node.kind() {
                                "string_literal" => {
                                    if let Some(s) = extract_string_literal_value(val_node, content)
                                    {
                                        paths.push(s);
                                    }
                                }
                                "element_value_array_initializer" => {
                                    let mut arr_cursor = val_node.walk();
                                    for arr_child in val_node.children(&mut arr_cursor) {
                                        if arr_child.kind() == "string_literal" {
                                            if let Some(s) =
                                                extract_string_literal_value(arr_child, content)
                                            {
                                                paths.push(s);
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    paths
}

fn find_element_value_pair_value(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if kind != "identifier" && kind != "=" {
            return Some(child);
        }
    }
    None
}

fn extract_string_literal_value(node: tree_sitter::Node, content: &str) -> Option<String> {
    let text = node.utf8_text(content.as_bytes()).unwrap_or("");
    if (text.starts_with('"') && text.ends_with('"'))
        || (text.starts_with('\'') && text.ends_with('\''))
    {
        Some(text[1..text.len() - 1].to_string())
    } else {
        Some(text.to_string())
    }
}

fn extract_request_methods(annotation_node: tree_sitter::Node, content: &str) -> Vec<String> {
    let name = find_child_by_kind(annotation_node, "identifier")
        .map(|n| n.utf8_text(content.as_bytes()).unwrap_or(""))
        .unwrap_or("");

    match name {
        "GetMapping" => vec!["GET".to_string()],
        "PostMapping" => vec!["POST".to_string()],
        "PutMapping" => vec!["PUT".to_string()],
        "DeleteMapping" => vec!["DELETE".to_string()],
        "PatchMapping" => vec!["PATCH".to_string()],
        "RequestMapping" => {
            let mut methods = Vec::new();
            if let Some(arg_list) = find_child_by_kind(annotation_node, "annotation_argument_list")
            {
                let mut cursor = arg_list.walk();
                for child in arg_list.children(&mut cursor) {
                    if child.kind() == "element_value_pair" {
                        let key = find_child_by_kind(child, "identifier")
                            .map(|k| k.utf8_text(content.as_bytes()).unwrap_or(""))
                            .unwrap_or("");
                        if key == "method" {
                            if let Some(val_node) = find_element_value_pair_value(child) {
                                match val_node.kind() {
                                    "field_access" => {
                                        if let Some(m) =
                                            extract_method_from_field_access(val_node, content)
                                        {
                                            methods.push(m);
                                        }
                                    }
                                    "element_value_array_initializer" => {
                                        let mut arr_cursor = val_node.walk();
                                        for arr_child in val_node.children(&mut arr_cursor) {
                                            if arr_child.kind() == "field_access" {
                                                if let Some(m) = extract_method_from_field_access(
                                                    arr_child, content,
                                                ) {
                                                    methods.push(m);
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }
            if methods.is_empty() {
                vec!["GET".to_string()]
            } else {
                methods
            }
        }
        _ => vec![],
    }
}

fn extract_method_from_field_access(node: tree_sitter::Node, content: &str) -> Option<String> {
    let mut last_id = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            last_id = Some(child);
        }
    }
    last_id.map(|id_node| {
        id_node
            .utf8_text(content.as_bytes())
            .unwrap_or("")
            .to_string()
    })
}

fn combine_paths(prefix: &str, suffix: &str) -> String {
    let mut p = prefix.trim().to_string();
    let mut s = suffix.trim().to_string();
    if !p.starts_with('/') && !p.is_empty() {
        p = format!("/{}", p);
    }
    if p.ends_with('/') {
        p.pop();
    }
    if !s.starts_with('/') && !s.is_empty() {
        s = format!("/{}", s);
    }
    format!("{}{}", p, s)
}
