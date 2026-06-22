//! The project-wide symbol index and its string interner. Symbols are stored as
//! integer surrogates ([`SymbolIx`]) over interned strings to keep the call-edge
//! resolution tables compact and cache-friendly.

use crate::db::Node;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;

pub type SymbolIx = u32;
pub type InternedString = u32;
pub type CandidateList = SmallVec<[SymbolIx; 4]>;
pub type CandidateByFile = FxHashMap<InternedString, CandidateList>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolCandidate {
    pub id: InternedString,
    pub file_path: InternedString,
    pub namespace: Option<InternedString>,
}

#[derive(Debug, Default)]
pub struct SymbolIndex {
    pub symbols: Vec<SymbolCandidate>,
    pub strings: Vec<String>,
    pub by_name: FxHashMap<String, CandidateList>,
    pub by_name_file: FxHashMap<String, CandidateByFile>,
    pub by_id: FxHashMap<String, SymbolIx>,
}

#[derive(Debug, Default)]
pub struct SymbolIndexBuilder {
    symbols: Vec<SymbolCandidate>,
    by_name: FxHashMap<String, CandidateList>,
    by_name_file: FxHashMap<String, CandidateByFile>,
    by_id: FxHashMap<String, SymbolIx>,
    interner: StringInterner,
}

impl SymbolIndexBuilder {
    pub fn push(
        &mut self,
        id: &str,
        name: &str,
        file_path: &str,
        qualified_name: Option<&str>,
    ) -> SymbolIx {
        let ix = self.symbols.len() as SymbolIx;
        let id_ix = self.interner.intern(id);
        let file_path_ix = self.interner.intern(file_path);
        let namespace_ix = qualified_name
            .and_then(parent_namespace)
            .map(|namespace| self.interner.intern(namespace));

        self.symbols
            .push(SymbolCandidate::new(id_ix, file_path_ix, namespace_ix));
        self.by_name.entry(name.to_string()).or_default().push(ix);
        self.by_name_file
            .entry(name.to_string())
            .or_default()
            .entry(file_path_ix)
            .or_default()
            .push(ix);
        self.by_id.insert(id.to_string(), ix);
        ix
    }

    pub fn finish(self) -> SymbolIndex {
        SymbolIndex {
            symbols: self.symbols,
            strings: self.interner.into_strings(),
            by_name: self.by_name,
            by_name_file: self.by_name_file,
            by_id: self.by_id,
        }
    }
}

impl SymbolIndex {
    pub fn from_nodes(nodes: &[Node]) -> Self {
        let mut builder = SymbolIndexBuilder::default();
        for node in nodes {
            builder.push(
                &node.id,
                &node.name,
                &node.file_path,
                node.qualified_name.as_deref(),
            );
        }
        builder.finish()
    }
}

#[derive(Debug, Default)]
pub struct StringInterner {
    strings: Vec<String>,
    by_value: FxHashMap<String, InternedString>,
}

impl StringInterner {
    pub fn intern(&mut self, value: &str) -> InternedString {
        if let Some(ix) = self.by_value.get(value) {
            return *ix;
        }

        let ix = self.strings.len() as InternedString;
        let value = value.to_string();
        self.strings.push(value.clone());
        self.by_value.insert(value, ix);
        ix
    }

    pub fn resolve(&self, ix: InternedString) -> &str {
        &self.strings[ix as usize]
    }

    pub fn into_strings(self) -> Vec<String> {
        self.strings
    }
}

impl SymbolCandidate {
    pub fn new(
        id: InternedString,
        file_path: InternedString,
        namespace: Option<InternedString>,
    ) -> Self {
        Self {
            id,
            file_path,
            namespace,
        }
    }
}

fn parent_namespace(qualified_name: &str) -> Option<&str> {
    let (parent, _) = qualified_name.rsplit_once("::")?;
    parent.rsplit("::").next()
}
