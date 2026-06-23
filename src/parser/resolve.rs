//! Resolve raw call sites into concrete call edges against the project-wide
//! [`SymbolIndex`], recording calls that match no known symbol as unresolved.

use super::interner::{SymbolIndex, SymbolIx};
use crate::db::{Edge, Node, RawCall, UnresolvedRef};

/// Resolve raw call sites into concrete edges against a project-wide symbol
/// index, preferring a target in the caller's own file before falling back to a
/// unique match anywhere in the project. Calls that match no known symbol are
/// returned separately as [`UnresolvedRef`]s.
///
/// - `symbols` stores the candidate metadata indexed by `SymbolIx`.
/// - `by_name` maps a symbol's simple name to every candidate index that bears it.
/// - `by_name_file` maps simple name, then file, to same-file candidate indices.
/// - `by_id` maps a node id to its candidate index.
pub fn resolve_calls_global(
    calls: &[RawCall],
    index: &SymbolIndex,
) -> (Vec<Edge>, Vec<UnresolvedRef>) {
    let mut edges = Vec::new();
    let mut unresolved = Vec::new();

    for call in calls {
        let (call_namespace, simple_name) = call_namespace_and_simple_name(&call.callee_name);

        let candidates = match index.by_name.get(simple_name) {
            Some(indices) if !indices.is_empty() => indices,
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
        let caller = index
            .by_id
            .get(&call.caller_id)
            .and_then(|ix| index.symbols.get(*ix as usize));

        let mut resolved_targets: Vec<(SymbolIx, i64)> = Vec::new();
        let get_string = |ix: u32| -> &str { &index.strings[ix as usize] };

        // Stage 1: Exact qualified hint (resolution_kind = 5, "exact")
        if resolved_targets.is_empty() {
            let mut stage_targets = Vec::new();
            if let Some(ref import_hint) = call.import_hint {
                for &cand_ix in candidates {
                    if let Some(cand) = index.symbols.get(cand_ix as usize) {
                        let cand_qname = get_string(cand.qualified_name);
                        if matches_exact_qualified(cand_qname, import_hint, simple_name) {
                            stage_targets.push((cand_ix, 5));
                        }
                    }
                }
            }
            if stage_targets.len() == 1 {
                resolved_targets = stage_targets;
            }
        }

        // Stage 2: Receiver static type + method (resolution_kind = 4, "receiver_type")
        if resolved_targets.is_empty() {
            let mut stage_targets = Vec::new();
            if let Some(ref rx_type) = call.receiver_type {
                for &cand_ix in candidates {
                    if let Some(cand) = index.symbols.get(cand_ix as usize) {
                        let cand_qname = get_string(cand.qualified_name);
                        if candidate_owner_matches(cand_qname, rx_type) {
                            // If import_hint matches this candidate's file path, promote to exact (kind 5)
                            let kind = if let Some(ref import_hint) = call.import_hint {
                                if matches_import_hint(get_string(cand.file_path), import_hint) {
                                    5
                                } else {
                                    4
                                }
                            } else {
                                4
                            };
                            stage_targets.push((cand_ix, kind));
                        }
                    }
                }
            }
            if !stage_targets.is_empty() {
                let max_kind = stage_targets.iter().map(|&(_, k)| k).max().unwrap_or(4);
                let best_targets: Vec<_> = stage_targets
                    .into_iter()
                    .filter(|&(_, k)| k == max_kind)
                    .collect();
                if best_targets.len() == 1 {
                    resolved_targets = best_targets;
                }
            }
        }

        // Stage 3: Package/import/namespace + name (resolution_kind = 3, "package")
        if resolved_targets.is_empty() {
            // Substage 3a: import hint matching candidate file path
            let mut stage_targets = Vec::new();
            if let Some(ref import_hint) = call.import_hint {
                for &cand_ix in candidates {
                    if let Some(cand) = index.symbols.get(cand_ix as usize) {
                        if matches_import_hint(get_string(cand.file_path), import_hint) {
                            stage_targets.push((cand_ix, 3));
                        }
                    }
                }
            }
            if stage_targets.len() == 1 {
                resolved_targets = stage_targets;
            }

            // Substage 3b: package matching candidate namespace/package
            if resolved_targets.is_empty() {
                let mut stage_targets = Vec::new();
                if let Some(ref pkg) = call.package_or_namespace {
                    for &cand_ix in candidates {
                        if let Some(cand) = index.symbols.get(cand_ix as usize) {
                            let cand_qname = get_string(cand.qualified_name);
                            if candidate_package_matches(cand_qname, pkg) {
                                stage_targets.push((cand_ix, 3));
                            }
                        }
                    }
                }
                if stage_targets.len() == 1 {
                    resolved_targets = stage_targets;
                }
            }

            // Substage 3c: explicit call namespace (e.g. Type::method or pkg::method)
            if resolved_targets.is_empty() {
                let mut stage_targets = Vec::new();
                if let Some(ns) = call_namespace {
                    for &cand_ix in candidates {
                        if let Some(cand) = index.symbols.get(cand_ix as usize) {
                            let cand_qname = get_string(cand.qualified_name);
                            if cand_qname_ends_with_ns_and_name(cand_qname, ns, simple_name) {
                                stage_targets.push((cand_ix, 3));
                            }
                        }
                    }
                }
                if stage_targets.len() == 1 {
                    resolved_targets = stage_targets;
                }
            }
        }

        // Stage 4: Same caller namespace/type (resolution_kind = 2, "namespace")
        if resolved_targets.is_empty() {
            let mut stage_targets = Vec::new();
            if let Some(caller_symbol) = caller {
                if let Some(caller_namespace_ix) = caller_symbol.namespace {
                    let caller_namespace = get_string(caller_namespace_ix);
                    for &cand_ix in candidates {
                        if let Some(cand) = index.symbols.get(cand_ix as usize) {
                            if let Some(cand_namespace_ix) = cand.namespace {
                                if get_string(cand_namespace_ix) == caller_namespace {
                                    stage_targets.push((cand_ix, 2));
                                }
                            }
                        }
                    }
                }
            }
            if stage_targets.len() == 1 {
                resolved_targets = stage_targets;
            }
        }

        // Stage 5: Same file (resolution_kind = 1, "same_file")
        if resolved_targets.is_empty() {
            let mut stage_targets = Vec::new();
            if let Some(caller_symbol) = caller {
                let caller_file = get_string(caller_symbol.file_path);
                for &cand_ix in candidates {
                    if let Some(cand) = index.symbols.get(cand_ix as usize) {
                        if get_string(cand.file_path) == caller_file {
                            stage_targets.push((cand_ix, 1));
                        }
                    }
                }
            }
            // Same-file does not enforce uniqueness-gating (allows multiple candidates in same file)
            if !stage_targets.is_empty() {
                resolved_targets = stage_targets;
            }
        }

        // Stage 6: Unique global name (resolution_kind = 0, "name_only")
        if resolved_targets.is_empty() && candidates.len() == 1 {
            resolved_targets.push((candidates[0], 0));
        }

        if !resolved_targets.is_empty() {
            for (target_ix, kind) in resolved_targets {
                let target_id = get_string(index.symbols[target_ix as usize].id).to_string();
                edges.push(Edge {
                    source_id: call.caller_id.clone(),
                    target_id,
                    kind: "calls".to_string(),
                    resolution_kind: kind,
                });
            }
        } else {
            unresolved.push(UnresolvedRef {
                id: None,
                source_id: call.caller_id.clone(),
                specifier: call.callee_name.clone(),
                kind: "calls".to_string(),
                line: call.line,
                column: call.column,
            });
        }
    }

    edges.sort_by(|a, b| {
        (&a.source_id, &a.target_id, &a.kind)
            .cmp(&(&b.source_id, &b.target_id, &b.kind))
            .then_with(|| b.resolution_kind.cmp(&a.resolution_kind))
    });
    edges.dedup_by(|a, b| {
        a.source_id == b.source_id && a.target_id == b.target_id && a.kind == b.kind
    });

    (edges, unresolved)
}

fn matches_exact_qualified(qualified_name: &str, hint: &str, simple_name: &str) -> bool {
    if !qualified_name.ends_with(simple_name) {
        return false;
    }
    let prefix_len = qualified_name.len() - simple_name.len();
    if prefix_len < 2 {
        return false;
    }
    if &qualified_name[prefix_len - 2..prefix_len] != "::" {
        return false;
    }
    let owner = &qualified_name[..prefix_len - 2];
    let mut owner_parts = owner.split("::");
    let mut hint_parts = hint.split('.');
    loop {
        match (owner_parts.next(), hint_parts.next()) {
            (Some(o), Some(h)) => {
                if o != h {
                    return false;
                }
            }
            (None, None) => return true,
            _ => return false,
        }
    }
}

fn candidate_owner_matches(qualified_name: &str, receiver_type: &str) -> bool {
    if let Some((owner, _)) = qualified_name.rsplit_once("::") {
        if owner == receiver_type {
            return true;
        }
        if owner.ends_with(receiver_type) {
            let prefix_len = owner.len() - receiver_type.len();
            if prefix_len >= 2 && &owner[prefix_len - 2..prefix_len] == "::" {
                return true;
            }
        }
    }
    false
}

fn matches_import_hint(candidate_file_path: &str, import_hint: &str) -> bool {
    if candidate_file_path.contains(import_hint) {
        return true;
    }
    let path_bytes = candidate_file_path.as_bytes();
    let hint_bytes = import_hint.as_bytes();
    if hint_bytes.is_empty() {
        return true;
    }
    let n = path_bytes.len();
    let m = hint_bytes.len();
    if n < m {
        return false;
    }
    for i in 0..=(n - m) {
        let mut matched = true;
        for j in 0..m {
            let p_b = path_bytes[i + j];
            let h_b = hint_bytes[j];
            let char_match = if h_b == b'.' {
                p_b == b'.' || p_b == b'/' || p_b == b'\\'
            } else if h_b == b'/' || h_b == b'\\' {
                p_b == b'/' || p_b == b'\\'
            } else {
                p_b == h_b
            };
            if !char_match {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

fn cand_qname_ends_with_ns_and_name(cand_qname: &str, ns: &str, simple_name: &str) -> bool {
    if !cand_qname.ends_with(simple_name) {
        return false;
    }
    let len_diff = cand_qname.len() - simple_name.len();
    if len_diff < 2 + ns.len() {
        return false;
    }
    if &cand_qname[len_diff - 2..len_diff] != "::" {
        return false;
    }
    let ns_start = len_diff - 2 - ns.len();
    &cand_qname[ns_start..len_diff - 2] == ns
}

fn candidate_package_matches(qualified_name: &str, package: &str) -> bool {
    if let Some((pkg_part, _)) = qualified_name.split_once("::") {
        pkg_part == package
    } else {
        qualified_name == package
    }
}

fn call_namespace_and_simple_name(callee_name: &str) -> (Option<&str>, &str) {
    if let Some((namespace, simple_name)) = callee_name.rsplit_once("::") {
        (namespace.rsplit("::").next(), simple_name)
    } else {
        (None, callee_name)
    }
}

/// Resolve call sites against only the given `nodes` (single-file scope). A thin
/// wrapper over [`resolve_calls_global`] used in tests and isolated parsing.
pub fn resolve_calls_local(nodes: &[Node], calls: &[RawCall]) -> Vec<Edge> {
    let index = SymbolIndex::from_nodes(nodes);
    resolve_calls_global(calls, &index).0
}
