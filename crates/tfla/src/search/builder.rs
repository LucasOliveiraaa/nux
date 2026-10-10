use hashbrown::HashSet;
use regex_automata::{
    MatchKind,
    dfa::dense::{self, BuildError, Builder},
    util::wire::DeserializeError,
};
use thiserror::Error;

use crate::search::search::SearchEngine;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SearcherId(pub(crate) usize);

#[derive(Debug, Error)]
pub enum BuilderError {
    #[error("regex error: {0}")]
    RegexError(#[from] BuildError),

    #[error("missing SEARCH section in DSecFile")]
    MissingSearchSection,

    #[error("version mismatch: expected {expected}, found {found}")]
    VersionMismatch { expected: u32, found: u32 },

    #[error("invalid DSecFile format")]
    InvalidDSecFileFormat,

    #[error("deserialization error: {0}")]
    DeserializationError(#[from] DeserializeError),
}

pub struct EngineBuilder<K: Clone + PartialEq + From<String> + ToString> {
    patterns: Vec<String>,
    kinds: Vec<K>,
    ignore: HashSet<SearcherId>,
    nline_tk: Option<SearcherId>,
    llcomment_tk: Option<SearcherId>,
    lcomment_tk: Option<SearcherId>,
    rcomment_tk: Option<SearcherId>,
}

impl<K: Clone + PartialEq + From<String> + ToString> EngineBuilder<K> {
    pub fn new() -> Self {
        Self {
            patterns: Vec::new(),
            kinds: Vec::new(),
            ignore: HashSet::new(),
            nline_tk: None,
            llcomment_tk: None,
            lcomment_tk: None,
            rcomment_tk: None,
        }
    }

    pub fn add_pattern(&mut self, kind: K, pattern: &str) -> SearcherId {
        self.patterns.push(pattern.to_string());
        self.kinds.push(kind);
        SearcherId(self.patterns.len() - 1)
    }

    pub fn ignore(&mut self, id: SearcherId) {
        self.ignore.insert(id);
    }

    pub fn nline_token(&mut self, id: SearcherId) {
        self.nline_tk = Some(id);
    }

    pub fn llcomment_token(&mut self, id: SearcherId) {
        self.llcomment_tk = Some(id);
    }

    pub fn lcomment_token(&mut self, id: SearcherId) {
        self.lcomment_tk = Some(id);
    }

    pub fn rcomment_token(&mut self, id: SearcherId) {
        self.rcomment_tk = Some(id);
    }

    pub fn build(self) -> Result<SearchEngine<K>, BuilderError> {
        let config = dense::Config::new().match_kind(MatchKind::All);
        let dfa = Builder::new()
            .configure(config)
            .build_many(&self.patterns)?;
        Ok(SearchEngine {
            dfa,
            kinds: self.kinds,
            ignore: self.ignore.iter().map(|s| s.0).collect(),
            nline_tk: self.nline_tk.map(|id| id.0).unwrap_or(usize::MAX),
            llcomment_tk: self.llcomment_tk.map(|id| id.0).unwrap_or(usize::MAX),
            lcomment_tk: self.lcomment_tk.map(|id| id.0).unwrap_or(usize::MAX),
            rcomment_tk: self.rcomment_tk.map(|id| id.0).unwrap_or(usize::MAX),
        })
    }
}
