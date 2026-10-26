use hashbrown::HashSet;
use regex_automata::{
    Anchored, Input,
    dfa::{Automaton, OverlappingState, dense::DFA},
};
use thiserror::Error;

use crate::{buff::BufWriter, dsecfile::DSecFile, search::BuilderError};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Token<K> {
    /// The kind of the token, which is a generic type `K`.
    pub kind: K,
    /// The content of the token as a string.
    pub content: String,
    /// The line number where the token starts (0-based).
    pub line: usize,
    /// The starting column of the token (0-based).
    pub start: usize,
    /// The ending column of the token (0-based, exclusive).
    pub end: usize,
}

#[derive(Debug, Error)]
pub enum TokenizeError {
    #[error("unknown content at line {line}, column {column}")]
    UnknownContent { line: usize, column: usize },
}

macro_rules! invalid_dsf {
    ($msg:expr) => {
        $msg.ok_or(BuilderError::InvalidDSecFileFormat)?
    };
}

pub struct SearchEngine<K: Clone + PartialEq> {
    pub(crate) dfa: DFA<Vec<u32>>,
    pub(crate) kinds: Vec<K>,
    pub(crate) ignore: HashSet<usize>,
    pub(crate) nline_tk: usize,
    pub(crate) llcomment_tk: usize,
    pub(crate) lcomment_tk: usize,
    pub(crate) rcomment_tk: usize,
}

impl<K: Clone + PartialEq + From<String> + ToString> SearchEngine<K> {
    pub fn tokenize(&self, content: &str) -> Result<Vec<Token<K>>, TokenizeError> {
        let mut tokens = vec![];

        let mut pos = 0;
        let mut line = 0;
        let mut column = 0;

        let mut in_block_comment = false;
        let mut in_line_comment = false;

        while pos < content.len() {
            // Anchor at `pos`: an unanchored search can skip ahead past
            // bytes it can't match instead of erroring on them right here.
            let input = Input::new(&content[pos..]).anchored(Anchored::Yes);

            let mut overlap_state = OverlappingState::start();
            let mut best: Option<(usize, usize)> = None;

            loop {
                self.dfa
                    .try_search_overlapping_fwd(&input, &mut overlap_state)
                    .map_err(|_| TokenizeError::UnknownContent { line, column })?;

                let Some(hm) = overlap_state.get_match() else {
                    break;
                };

                let id = hm.pattern().as_usize();
                let offset = hm.offset();

                best = Some(match best {
                    None => (id, offset),
                    Some((_, best_offset)) if offset > best_offset => (id, offset),
                    Some((best_id, best_offset)) if offset == best_offset => {
                        (best_id.min(id), best_offset)
                    }
                    Some(existing) => existing,
                });
            }

            let Some((id, len)) = best else {
                return Err(TokenizeError::UnknownContent { line, column });
            };

            if len == 0 {
                return Err(TokenizeError::UnknownContent { line, column });
            }

            let kind = self.kinds[id].clone();
            let token_str = &content[pos..pos + len];
            pos += len;
            column += len;

            let tk = Token {
                kind,
                content: token_str.to_string(),
                line,
                start: column - len,
                end: column,
            };

            if id == self.nline_tk {
                line += 1;
                column = 0;
                in_line_comment = false;
            }
            if id == self.llcomment_tk {
                in_line_comment = true;
            }
            if id == self.lcomment_tk || id == self.rcomment_tk {
                in_block_comment = id == self.lcomment_tk;
            }
            if !self.ignore.contains(&id)
                && !in_line_comment
                && !in_block_comment
                && id != self.rcomment_tk
            {
                tokens.push(tk);
            }
        }

        Ok(tokens)
    }

    pub fn from_dsf(dsf: &DSecFile, curr_version: u32) -> Result<Self, BuilderError> {
        Self::read_dsf(dsf, curr_version, |name, _| Some(K::from(name)))
    }

    /// Like [`from_dsf`](Self::from_dsf), for an engine built with `kinds`
    /// in this order: the stored kinds must be exactly these (compared with
    /// `to_string`). A stale or damaged file is then an error, instead of a
    /// panic in a `From<String>` that does not know the name.
    pub fn from_dsf_with_kinds(
        dsf: &DSecFile,
        curr_version: u32,
        kinds: &[K],
    ) -> Result<Self, BuilderError> {
        Self::read_dsf(dsf, curr_version, |name, index| {
            kinds
                .get(index)
                .filter(|kind| kind.to_string() == name)
                .cloned()
        })
    }

    /// Reads the `SEARCH` section; `kind` turns the `index`-th stored name
    /// into a kind, or rejects it.
    fn read_dsf(
        dsf: &DSecFile,
        curr_version: u32,
        mut kind: impl FnMut(String, usize) -> Option<K>,
    ) -> Result<Self, BuilderError> {
        let section = dsf
            .get("SEARCH")
            .ok_or(BuilderError::MissingSearchSection)?;
        let mut buff = section.as_buff();

        let version = invalid_dsf!(buff.next_u32());

        if version != curr_version {
            return Err(BuilderError::VersionMismatch {
                expected: curr_version,
                found: version,
            });
        }

        let dfa_size = invalid_dsf!(buff.next_u32()) as usize;
        let dfa_data = invalid_dsf!(buff.next_bytes(dfa_size));

        // `DFA::from_bytes` needs 4-byte aligned input. A section's bytes are
        // only aligned if the allocator happens to align a `Vec<u8>`: copy
        // them into memory aligned for `u32`.
        let mut aligned = vec![0u32; dfa_data.len().div_ceil(4)];
        // SAFETY: the `u32`s are viewed as their bytes, which any value has.
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(aligned.as_mut_ptr().cast::<u8>(), aligned.len() * 4)
        };
        bytes[..dfa_data.len()].copy_from_slice(dfa_data);
        // Validates the whole DFA, so a damaged file cannot cause bad reads.
        let (dfa_ref, _) = DFA::from_bytes(&bytes[..dfa_data.len()])?;
        let dfa = dfa_ref.to_owned();

        let mut kinds = vec![];
        let kind_count = invalid_dsf!(buff.next_u32());
        for index in 0..kind_count as usize {
            let name = invalid_dsf!(buff.next_string());
            kinds.push(invalid_dsf!(kind(name, index)));
        }
        // `tokenize` looks matches up by pattern id.
        if dfa.pattern_len() != kinds.len() {
            return Err(BuilderError::InvalidDSecFileFormat);
        }

        let mut ignore = HashSet::new();
        let ignore_count = invalid_dsf!(buff.next_u32());
        for _ in 0..ignore_count {
            let id = invalid_dsf!(buff.next_u32()) as usize;
            ignore.insert(id);
        }

        let nline_tk = invalid_dsf!(buff.next_u32()) as usize;
        let llcomment_tk = invalid_dsf!(buff.next_u32()) as usize;
        let lcomment_tk = invalid_dsf!(buff.next_u32()) as usize;
        let rcomment_tk = invalid_dsf!(buff.next_u32()) as usize;

        Ok(Self {
            dfa,
            kinds,
            ignore,
            nline_tk,
            llcomment_tk,
            lcomment_tk,
            rcomment_tk,
        })
    }

    pub fn to_dsf(&self, dsf: &mut DSecFile, curr_version: u32) -> Result<(), BuilderError> {
        let mut buff = BufWriter::new();

        let (dfa_data, _) = self.dfa.to_bytes_native_endian();
        buff.write_u32(curr_version);
        buff.write_u32(dfa_data.len() as u32);
        buff.write_bytes(&dfa_data);

        buff.write_u32(self.kinds.len() as u32);
        for kind in &self.kinds {
            buff.write_string(&kind.to_string());
        }

        buff.write_u32(self.ignore.len() as u32);
        for id in &self.ignore {
            buff.write_u32(*id as u32);
        }

        buff.write_u32(self.nline_tk as u32);
        buff.write_u32(self.llcomment_tk as u32);
        buff.write_u32(self.lcomment_tk as u32);
        buff.write_u32(self.rcomment_tk as u32);

        dsf.set("SEARCH", buff.as_ref());

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::EngineBuilder;

    fn engine() -> SearchEngine<String> {
        let mut builder = EngineBuilder::new();
        builder.add_pattern("word".to_string(), "[a-z]+");
        let space = builder.add_pattern("space".to_string(), " ");
        builder.ignore(space);
        builder.build().unwrap()
    }

    fn kinds(tokens: &[Token<String>]) -> Vec<(&str, &str)> {
        tokens
            .iter()
            .map(|t| (t.kind.as_str(), t.content.as_str()))
            .collect()
    }

    /// Saves `engine` after another section and reads it back from disk.
    fn round_trip(engine: &SearchEngine<String>, name: &str) -> DSecFile {
        let path = std::env::temp_dir().join(format!("tfla-{name}-{}.dsf", std::process::id()));
        let mut dsf = DSecFile::new(&path);
        dsf.add("pad", b"ab".to_vec()).unwrap();
        engine.to_dsf(&mut dsf, 7).unwrap();
        dsf.save().unwrap();
        let read = DSecFile::from_file(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        read
    }

    #[test]
    fn engines_survive_a_round_trip_through_a_file() {
        let original = engine();
        let dsf = round_trip(&original, "round-trip");
        let expected = ["word".to_string(), "space".to_string()];
        let loaded = SearchEngine::from_dsf_with_kinds(&dsf, 7, &expected).unwrap();
        let text = "hello big world";
        assert_eq!(
            kinds(&loaded.tokenize(text).unwrap()),
            kinds(&original.tokenize(text).unwrap())
        );
        assert_eq!(
            kinds(&loaded.tokenize(text).unwrap()),
            [("word", "hello"), ("word", "big"), ("word", "world")]
        );
        assert!(SearchEngine::<String>::from_dsf(&dsf, 7).is_ok());
    }

    #[test]
    fn stale_or_damaged_files_are_errors() {
        let dsf = round_trip(&engine(), "damaged");
        let check = |dsf: &DSecFile, kinds: &[&str]| {
            let kinds: Vec<String> = kinds.iter().map(|k| k.to_string()).collect();
            SearchEngine::from_dsf_with_kinds(dsf, 7, &kinds).err()
        };
        assert!(matches!(
            SearchEngine::<String>::from_dsf(&dsf, 8),
            Err(BuilderError::VersionMismatch {
                expected: 8,
                found: 7
            })
        ));
        assert!(matches!(
            check(&dsf, &["word", "blank"]),
            Some(BuilderError::InvalidDSecFileFormat)
        ));
        assert!(matches!(
            check(&dsf, &["word"]),
            Some(BuilderError::InvalidDSecFileFormat)
        ));

        // Flipped bytes inside the DFA, and a truncated section.
        let mut search = dsf.get("SEARCH").unwrap().data.clone();
        for byte in &mut search[40..200] {
            *byte ^= 0x5a;
        }
        let mut damaged = DSecFile::new("damaged.dsf");
        damaged.set("SEARCH", search.clone());
        assert!(check(&damaged, &["word", "space"]).is_some());
        damaged.set("SEARCH", search[..search.len() / 2].to_vec());
        assert!(check(&damaged, &["word", "space"]).is_some());
    }
}
