//! C/C++ AST traversal: namespaces, classes/structs, (out-of-line) functions and
//! methods, and call sites. Shared by the C and C++ grammars.

use super::common::{
    find_child_by_kind, last_descendant_by_kind, normalize_qualified_name, push_symbol,
    qualified_name, raw_call, text_for_node,
};
use crate::db::{Node, RawCall};

#[derive(Clone, Copy)]
pub(super) struct CLikeContext<'a> {
    pub(super) parent_qualified_name: Option<&'a str>,
    pub(super) parent_is_type: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn traverse_c_like<'a>(
    node: tree_sitter::Node<'a>,
    content: &'a str,
    file_path: &str,
    nodes: &mut Vec<Node>,
    calls: &mut Vec<RawCall>,
    context: CLikeContext<'a>,
    current_caller_id: Option<&str>,
    local_ptrs: Option<&std::collections::HashSet<String>>,
) {
    let mut next_context = context;
    let mut next_caller_id = current_caller_id;
    let mut next_local_ptrs = local_ptrs;
    #[allow(unused_assignments)]
    let mut local_ptrs_holder = std::collections::HashSet::new();

    #[allow(unused_assignments)]
    let mut qname_holder = String::new();
    #[allow(unused_assignments)]
    let mut id_holder = String::new();

    let kind = node.kind();
    if kind == "function_definition" {
        local_ptrs_holder = collect_c_function_pointers(node, content);
        next_local_ptrs = Some(&local_ptrs_holder);
    }

    match kind {
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
                    let func_name = extract_c_call_target(func_node, content);
                    let mut call = raw_call(caller, func_name.clone(), node);
                    call.package_or_namespace =
                        context.parent_qualified_name.map(|s| s.to_string());

                    if func_node.kind() == "field_expression" {
                        call.call_kind = Some("method".to_string());
                        if let Some(obj_node) = func_node.child(0) {
                            let receiver = text_for_node(obj_node, content).trim().to_string();
                            if !receiver.is_empty() {
                                call.receiver_expr = Some(receiver);
                            }
                        }
                    } else if func_node.kind() == "identifier" {
                        if let Some(ptrs) = next_local_ptrs {
                            if ptrs.contains(&func_name) {
                                call.call_kind = Some("indirect".to_string());
                            }
                        }
                        if call.call_kind.is_none() {
                            let is_all_uppercase = func_name
                                .chars()
                                .all(|c| !c.is_alphabetic() || c.is_uppercase());
                            if is_all_uppercase && !func_name.is_empty() {
                                call.call_kind = Some("macro".to_string());
                            } else {
                                call.call_kind = Some("function".to_string());
                            }
                        }
                    }
                    calls.push(call);
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
            next_local_ptrs,
        );
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

fn collect_c_function_pointers(
    node: tree_sitter::Node,
    content: &str,
) -> std::collections::HashSet<String> {
    let mut ptrs = std::collections::HashSet::new();
    fn walk(n: tree_sitter::Node, content: &str, ptrs: &mut std::collections::HashSet<String>) {
        if n.kind() == "pointer_declarator" {
            if let Some(parent) = n.parent() {
                if parent.kind() == "parenthesized_declarator" {
                    if let Some(id_node) = last_descendant_by_kind(
                        parent,
                        &[
                            "qualified_identifier",
                            "field_identifier",
                            "type_identifier",
                            "identifier",
                        ],
                    ) {
                        let name = text_for_node(id_node, content).trim().to_string();
                        if !name.is_empty() {
                            ptrs.insert(name);
                        }
                    }
                }
            }
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            if child.kind() != "function_definition" {
                walk(child, content, ptrs);
            }
        }
    }
    walk(node, content, &mut ptrs);
    ptrs
}
