//! Java AST traversal: classes/interfaces, methods/constructors, method
//! invocations, and object-creation (constructor call) sites.

use super::common::{find_child_by_kind, get_doc_comment, get_signature, raw_call, text_for_node};
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
    package_name: Option<&str>,
    imports: &[String],
    local_types: Option<&std::collections::HashMap<String, String>>,
) {
    let mut next_parent_qualified_name = current_parent_qualified_name;
    let mut next_caller_id = current_caller_id;
    let mut next_controller_prefixes = current_controller_prefixes;
    let mut next_local_types = local_types;
    #[allow(unused_assignments)]
    let mut local_types_holder = std::collections::HashMap::new();

    #[allow(unused_assignments)]
    let mut qname_holder = String::new();
    #[allow(unused_assignments)]
    let mut id_holder = String::new();
    #[allow(unused_assignments)]
    let mut class_prefixes_holder = Vec::new();

    let kind = node.kind();
    match kind {
        "class_declaration" | "interface_declaration" => {
            local_types_holder = collect_java_types(node, content);
            next_local_types = Some(&local_types_holder);

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
                } else if let Some(pkg) = package_name {
                    format!("{}::{}", pkg, name)
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
                    resolution_kind: None,
                    confidence: None,
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
            local_types_holder = collect_java_types(node, content);
            if let Some(parent_types) = local_types {
                for (k, v) in parent_types {
                    local_types_holder
                        .entry(k.clone())
                        .or_insert_with(|| v.clone());
                }
            }
            next_local_types = Some(&local_types_holder);

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
                    resolution_kind: None,
                    confidence: None,
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
                                            let route_id = format!(
                                                "{}::{}::route::{} {}",
                                                file_path, qname, http_method, combined_path
                                            );
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
                                                resolution_kind: None,
                                                confidence: None,
                                            });

                                            // Raw call linking the route node (as caller) to the method (as callee)
                                            let mut rcall =
                                                raw_call(&route_id, qname.clone(), node);
                                            rcall.relationship_kind = "route_handler".to_string();
                                            rcall.target_qualified_hint = Some(qname.clone());
                                            rcall.resolution_hint = Some(6);
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
                    let mut call = raw_call(caller, method_name.clone(), node);
                    call.call_kind = Some("method".to_string());
                    call.package_or_namespace = package_name.map(|s| s.to_string());

                    if let Some(object_node) = node.child_by_field_name("object") {
                        let receiver = object_node
                            .utf8_text(content.as_bytes())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if !receiver.is_empty() {
                            call.receiver_expr = Some(receiver.clone());
                            if let Some(types) = next_local_types {
                                if let Some(t) = types.get(&receiver) {
                                    call.receiver_type = Some(t.clone());
                                    call.import_hint = find_import_hint(t, imports);
                                    if let Some(service) = grpc_client_service(t) {
                                        call.relationship_kind = "grpc_calls".to_string();
                                        call.target_qualified_hint =
                                            Some(format!("grpc::{service}::{method_name}"));
                                        call.resolution_hint = Some(8);
                                    }
                                }
                            }
                            if call.import_hint.is_none() {
                                if let Some(hint) = find_import_hint(&receiver, imports) {
                                    call.import_hint = Some(hint);
                                }
                            }
                        }
                    }
                    calls.push(call);
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
                    let mut call = raw_call(caller, type_name.clone(), node);
                    call.call_kind = Some("constructor".to_string());
                    call.receiver_type = Some(type_name.clone());
                    call.import_hint = find_import_hint(&type_name, imports);
                    call.package_or_namespace = package_name.map(|s| s.to_string());
                    calls.push(call);
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
            package_name,
            imports,
            next_local_types,
        );
    }
}

/// Extract framework relationships that are not ordinary Java invocation
/// edges. Every emitted relation has an indexed source node and a parser
/// supplied framework evidence tier; resolution never promotes it to `exact`.
pub(super) fn extract_framework_relationships<'a>(
    root: tree_sitter::Node<'a>,
    content: &'a str,
    file_path: &str,
    nodes: &mut Vec<Node>,
    calls: &mut Vec<RawCall>,
    package_name: Option<&str>,
) {
    let mut cursor = root.walk();
    for class in root.children(&mut cursor) {
        extract_class_framework_relationships(
            class,
            content,
            file_path,
            nodes,
            calls,
            package_name,
            None,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn extract_class_framework_relationships<'a>(
    node: tree_sitter::Node<'a>,
    content: &'a str,
    file_path: &str,
    nodes: &mut Vec<Node>,
    calls: &mut Vec<RawCall>,
    package_name: Option<&str>,
    parent_qname: Option<&str>,
) {
    if !matches!(node.kind(), "class_declaration" | "interface_declaration") {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            extract_class_framework_relationships(
                child,
                content,
                file_path,
                nodes,
                calls,
                package_name,
                parent_qname,
            );
        }
        return;
    }
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = text_for_node(name_node, content);
    let qname = parent_qname
        .map(|parent| format!("{parent}::{name}"))
        .or_else(|| package_name.map(|package| format!("{package}::{name}")))
        .unwrap_or(name.clone());
    let class_id = format!("{file_path}::{qname}");
    let annotations = annotation_names(node, content);
    let component = annotations.iter().any(|annotation| {
        matches!(
            annotation.as_str(),
            "Component"
                | "Service"
                | "Repository"
                | "Controller"
                | "RestController"
                | "Configuration"
        )
    });

    if let Some(prefix) = annotation_literal(node, content, "ConfigurationProperties") {
        let config_qname = format!("config::{prefix}::{qname}");
        let config_id =
            push_framework_node(nodes, file_path, &config_qname, "config_key", node, content);
        push_framework_relation(
            calls,
            &config_id,
            name.clone(),
            node,
            "configuration_binds",
            false,
            Some(qname.clone()),
            6,
        );
    }

    let grpc_service = if annotations
        .iter()
        .any(|annotation| annotation == "GrpcService")
    {
        grpc_service_name(node, content)
    } else {
        None
    };
    let feign_service = annotation_literal(node, content, "FeignClient");
    let constructors = direct_children_of_kind(node, "constructor_declaration");

    for field in direct_children_of_kind(node, "field_declaration") {
        if component && has_annotation(field, content, &["Autowired", "Inject", "Resource"]) {
            if let Some(ty) = field
                .child_by_field_name("type")
                .map(|n| text_for_node(n, content))
            {
                push_framework_relation(
                    calls,
                    &class_id,
                    ty,
                    field,
                    "injected_into",
                    true,
                    None,
                    6,
                );
            }
        }
        if component {
            if let Some(key) = annotation_placeholder(field, content, "Value") {
                let key_qname = format!("config::{key}::{qname}");
                let key_id =
                    push_framework_node(nodes, file_path, &key_qname, "config_key", field, content);
                push_framework_relation(
                    calls,
                    &key_id,
                    name.clone(),
                    field,
                    "configuration_binds",
                    false,
                    Some(qname.clone()),
                    6,
                );
            }
        }
    }

    for constructor in &constructors {
        let annotated = has_annotation(*constructor, content, &["Autowired", "Inject", "Resource"]);
        if component && (annotated || constructors.len() == 1) {
            let tier = if annotated { 6 } else { 7 };
            for parameter in descendants_of_kind(*constructor, "formal_parameter") {
                if let Some(ty) = parameter
                    .child_by_field_name("type")
                    .map(|n| text_for_node(n, content))
                {
                    push_framework_relation(
                        calls,
                        &class_id,
                        ty,
                        parameter,
                        "injected_into",
                        true,
                        None,
                        tier,
                    );
                }
            }
        }
    }

    for method in direct_children_of_kind(node, "method_declaration") {
        let Some(method_name_node) = method.child_by_field_name("name") else {
            continue;
        };
        let method_name = text_for_node(method_name_node, content);
        let method_qname = format!("{qname}::{method_name}");
        let method_id = format!("{file_path}::{method_qname}");

        if let Some(service) = feign_service.as_deref() {
            if has_mapping_annotation(method, content) {
                let endpoint_qname = format!("feign::{service}::{method_qname}");
                let endpoint_id = push_framework_node(
                    nodes,
                    file_path,
                    &endpoint_qname,
                    "feign_endpoint",
                    method,
                    content,
                );
                push_framework_relation(
                    calls,
                    &method_id,
                    method_name.clone(),
                    method,
                    "feign_calls",
                    false,
                    Some(endpoint_qname),
                    6,
                );
                let _ = endpoint_id;
            }
        }

        if let Some(service) = grpc_service.as_deref() {
            let endpoint_qname = format!("grpc::{service}::{method_name}");
            push_framework_node(
                nodes,
                file_path,
                &endpoint_qname,
                "grpc_endpoint",
                method,
                content,
            );
        }

        if has_annotation(method, content, &["EventListener"]) {
            if let Some(event) = event_listener_type(method, content)
                .or_else(|| first_parameter_type(method, content))
            {
                push_framework_relation(
                    calls,
                    &method_id,
                    event,
                    method,
                    "consumes_event",
                    true,
                    None,
                    6,
                );
            }
        }
        let local_types = collect_java_types(method, content);
        for invocation in descendants_of_kind(method, "method_invocation") {
            let Some(invoked) = invocation.child_by_field_name("name") else {
                continue;
            };
            if text_for_node(invoked, content) != "publishEvent" {
                continue;
            }
            let Some(arguments) = find_child_by_kind(invocation, "argument_list") else {
                continue;
            };
            let mut event_type = None;
            let mut event_anchor = None;
            let mut cursor = arguments.walk();
            for argument in arguments.children(&mut cursor) {
                if argument.kind() == "object_creation_expression" {
                    event_type = argument
                        .child_by_field_name("type")
                        .map(|ty| text_for_node(ty, content));
                    event_anchor = Some(argument);
                    break;
                }
                if argument.kind() == "identifier" {
                    let name = text_for_node(argument, content);
                    if let Some(ty) = local_types.get(&name) {
                        event_type = Some(ty.clone());
                        event_anchor = Some(argument);
                        break;
                    }
                }
            }
            if let (Some(event_type), Some(event_anchor)) = (event_type, event_anchor) {
                push_framework_relation(
                    calls,
                    &method_id,
                    event_type,
                    event_anchor,
                    "publishes_event",
                    false,
                    None,
                    7,
                );
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "class_declaration" | "interface_declaration") {
            extract_class_framework_relationships(
                child,
                content,
                file_path,
                nodes,
                calls,
                package_name,
                Some(&qname),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_framework_relation(
    calls: &mut Vec<RawCall>,
    source_id: &str,
    target_name: String,
    node: tree_sitter::Node,
    kind: &str,
    reverse: bool,
    target_qualified_hint: Option<String>,
    resolution_hint: i64,
) {
    let mut relation = raw_call(source_id, target_name, node);
    relation.relationship_kind = kind.to_string();
    relation.reverse_edge = reverse;
    relation.target_qualified_hint = target_qualified_hint;
    relation.resolution_hint = Some(resolution_hint);
    calls.push(relation);
}

fn push_framework_node(
    nodes: &mut Vec<Node>,
    file_path: &str,
    qname: &str,
    kind: &str,
    anchor: tree_sitter::Node,
    content: &str,
) -> String {
    let id = format!("{file_path}::{qname}");
    if nodes.iter().any(|node| node.id == id) {
        return id;
    }
    let start = anchor.start_position();
    let end = anchor.end_position();
    nodes.push(Node {
        id: id.clone(),
        name: qname.rsplit("::").next().unwrap_or(qname).to_string(),
        kind: kind.to_string(),
        qualified_name: Some(qname.to_string()),
        file_path: file_path.to_string(),
        start_line: (start.row + 1) as i64,
        end_line: (end.row + 1) as i64,
        start_column: start.column as i64,
        end_column: end.column as i64,
        signature: Some(get_signature(anchor, content)),
        doc_comment: get_doc_comment(anchor, content),
        is_test: false,
        resolution_kind: None,
        confidence: None,
    });
    id
}

fn descendants_of_kind<'a>(node: tree_sitter::Node<'a>, kind: &str) -> Vec<tree_sitter::Node<'a>> {
    let mut out = Vec::new();
    fn walk<'a>(node: tree_sitter::Node<'a>, kind: &str, out: &mut Vec<tree_sitter::Node<'a>>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == kind {
                out.push(child);
            }
            walk(child, kind, out);
        }
    }
    walk(node, kind, &mut out);
    out
}

fn direct_children_of_kind<'a>(
    node: tree_sitter::Node<'a>,
    kind: &str,
) -> Vec<tree_sitter::Node<'a>> {
    let Some(body) = find_child_by_kind(node, "class_body")
        .or_else(|| find_child_by_kind(node, "interface_body"))
    else {
        return Vec::new();
    };
    let mut cursor = body.walk();
    body.children(&mut cursor)
        .filter(|child| child.kind() == kind)
        .collect()
}

fn annotation_names(node: tree_sitter::Node, content: &str) -> Vec<String> {
    find_annotations(node)
        .into_iter()
        .filter_map(|annotation| {
            find_child_by_kind(annotation, "identifier").map(|name| text_for_node(name, content))
        })
        .collect()
}

fn has_annotation(node: tree_sitter::Node, content: &str, expected: &[&str]) -> bool {
    annotation_names(node, content)
        .iter()
        .any(|name| expected.contains(&name.as_str()))
}

fn annotation_literal(
    node: tree_sitter::Node,
    content: &str,
    annotation_name: &str,
) -> Option<String> {
    find_annotations(node).into_iter().find_map(|annotation| {
        (find_child_by_kind(annotation, "identifier")
            .map(|name| text_for_node(name, content))
            .as_deref()
            == Some(annotation_name))
        .then(|| {
            descendants_of_kind(annotation, "string_literal")
                .into_iter()
                .next()
        })
        .flatten()
        .and_then(|literal| extract_string_literal_value(literal, content))
    })
}

fn annotation_placeholder(
    node: tree_sitter::Node,
    content: &str,
    annotation_name: &str,
) -> Option<String> {
    annotation_literal(node, content, annotation_name).and_then(|value| {
        value
            .strip_prefix("${")
            .and_then(|key| key.strip_suffix('}'))
            .map(str::to_string)
    })
}

fn has_mapping_annotation(node: tree_sitter::Node, content: &str) -> bool {
    has_annotation(
        node,
        content,
        &[
            "RequestMapping",
            "GetMapping",
            "PostMapping",
            "PutMapping",
            "DeleteMapping",
            "PatchMapping",
        ],
    )
}

fn event_listener_type(node: tree_sitter::Node, content: &str) -> Option<String> {
    find_annotations(node).into_iter().find_map(|annotation| {
        let is_listener = find_child_by_kind(annotation, "identifier")
            .map(|name| text_for_node(name, content))
            .as_deref()
            == Some("EventListener");
        if !is_listener {
            return None;
        }
        let text = text_for_node(annotation, content);
        text.split('(')
            .nth(1)
            .and_then(|value| value.split(".class").next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn first_parameter_type(node: tree_sitter::Node, content: &str) -> Option<String> {
    descendants_of_kind(node, "formal_parameter")
        .into_iter()
        .next()
        .and_then(|parameter| parameter.child_by_field_name("type"))
        .map(|ty| text_for_node(ty, content))
}

fn grpc_service_name(node: tree_sitter::Node, content: &str) -> Option<String> {
    let text = get_signature(node, content);
    let marker = text.split("extends ").nth(1)?.split_whitespace().next()?;
    marker.split("Grpc.").next().map(str::to_string)
}

fn grpc_client_service(receiver_type: &str) -> Option<&str> {
    receiver_type
        .split("Grpc.")
        .next()
        .filter(|_| receiver_type.contains("Stub"))
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
                vec!["ANY".to_string()]
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
    let path = format!("{}{}", p, s);
    if path.is_empty() {
        "/".to_string()
    } else {
        path
    }
}

fn find_import_hint(name: &str, imports: &[String]) -> Option<String> {
    for imp in imports {
        if imp == name || imp.ends_with(&format!(".{}", name)) {
            return Some(imp.clone());
        }
    }
    None
}

fn get_identifier_text(n: tree_sitter::Node, content: &str) -> String {
    if n.kind() == "identifier" {
        n.utf8_text(content.as_bytes())
            .unwrap_or("")
            .trim()
            .to_string()
    } else {
        if let Some(id_node) = find_child_by_kind(n, "identifier") {
            id_node
                .utf8_text(content.as_bytes())
                .unwrap_or("")
                .trim()
                .to_string()
        } else {
            let mut cursor = n.walk();
            for child in n.children(&mut cursor) {
                let s = get_identifier_text(child, content);
                if !s.is_empty() {
                    return s;
                }
            }
            "".to_string()
        }
    }
}

fn collect_java_types(
    node: tree_sitter::Node,
    content: &str,
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    walk_java_types(node, content, &mut map);
    map
}

fn walk_java_types(
    n: tree_sitter::Node,
    content: &str,
    map: &mut std::collections::HashMap<String, String>,
) {
    let kind = n.kind();
    match kind {
        "formal_parameter" => {
            let type_node = n.child_by_field_name("type");
            let name_node = n
                .child_by_field_name("name")
                .or_else(|| find_child_by_kind(n, "variable_declarator_id"));
            if let (Some(t), Some(name_id)) = (type_node, name_node) {
                let type_str = t
                    .utf8_text(content.as_bytes())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let name_str = get_identifier_text(name_id, content);
                if !name_str.is_empty() && !type_str.is_empty() {
                    map.insert(name_str, type_str);
                }
            }
        }
        "local_variable_declaration" | "field_declaration" => {
            let type_node = n.child_by_field_name("type");
            if let Some(t) = type_node {
                let type_str = t
                    .utf8_text(content.as_bytes())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !type_str.is_empty() {
                    let mut cursor = n.walk();
                    for child in n.children(&mut cursor) {
                        if child.kind() == "variable_declarator" {
                            if let Some(name_id) = child
                                .child_by_field_name("name")
                                .or_else(|| find_child_by_kind(child, "variable_declarator_id"))
                            {
                                let name_str = get_identifier_text(name_id, content);
                                if !name_str.is_empty() {
                                    map.insert(name_str, type_str.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        _ => {
            let mut cursor = n.walk();
            for child in n.children(&mut cursor) {
                if child.kind() != "class_declaration" && child.kind() != "interface_declaration" {
                    walk_java_types(child, content, map);
                }
            }
        }
    }
}
