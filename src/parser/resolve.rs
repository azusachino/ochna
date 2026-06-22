//! Resolve raw call sites into concrete call edges against the project-wide
//! [`SymbolIndex`], recording calls that match no known symbol as unresolved.

use super::interner::{SymbolCandidate, SymbolIndex, SymbolIx};
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
        let same_file = caller.and_then(|caller| {
            index
                .by_name_file
                .get(simple_name)
                .and_then(|by_file| by_file.get(&caller.file_path))
        });
        let working_set = if let Some(indices) = same_file.filter(|indices| !indices.is_empty()) {
            indices.as_slice()
        } else {
            candidates.as_slice()
        };

        let target_id = if working_set.len() == 1 {
            index.strings[index.symbols[working_set[0] as usize].id as usize].clone()
        } else {
            let caller_namespace = caller.and_then(|symbol| {
                symbol
                    .namespace
                    .map(|namespace| index.strings[namespace as usize].as_str())
            });
            let target_ix = disambiguate(
                working_set,
                &index.symbols,
                &index.strings,
                call_namespace,
                caller_namespace,
            )
            .unwrap_or(working_set[0]);
            index.strings[index.symbols[target_ix as usize].id as usize].clone()
        };

        edges.push(Edge {
            source_id: call.caller_id.clone(),
            target_id,
            kind: "calls".to_string(),
        });
    }

    edges.sort_by(|a, b| {
        (&a.source_id, &a.target_id, &a.kind).cmp(&(&b.source_id, &b.target_id, &b.kind))
    });
    edges.dedup_by(|a, b| {
        a.source_id == b.source_id && a.target_id == b.target_id && a.kind == b.kind
    });

    (edges, unresolved)
}

/// Pick a target among several same-named candidates using namespace and
/// receiver context. Case A: the call carries an explicit namespace
/// (e.g. `Point::new`). Case B: fall back to the caller's own struct/class.
fn disambiguate(
    candidates: &[SymbolIx],
    symbols: &[SymbolCandidate],
    interned_strings: &[String],
    call_namespace: Option<&str>,
    caller_namespace: Option<&str>,
) -> Option<SymbolIx> {
    // Case A: explicit namespace on the call target.
    if let Some(namespace) = call_namespace {
        if let Some(candidate) = candidates.iter().copied().find(|ix| {
            symbols
                .get(*ix as usize)
                .and_then(|symbol| symbol.namespace)
                .is_some_and(|candidate_namespace| {
                    interned_strings[candidate_namespace as usize] == namespace
                })
        }) {
            return Some(candidate);
        }
    }

    // Case B: caller is a method/constructor — prefer a target on the same receiver.
    if let Some(namespace) = caller_namespace {
        if let Some(candidate) = candidates.iter().copied().find(|ix| {
            symbols
                .get(*ix as usize)
                .and_then(|symbol| symbol.namespace)
                .is_some_and(|candidate_namespace| {
                    interned_strings[candidate_namespace as usize] == namespace
                })
        }) {
            return Some(candidate);
        }
    }

    None
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
