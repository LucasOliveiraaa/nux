//! Caching the lexer on disk.
//!
//! Building the lexer's DFA from the token patterns takes about a second;
//! reading it back from a file takes a few milliseconds. The cache is
//! `lexer-<fingerprint>.dsf` in the cache directory, where the fingerprint
//! covers the token patterns and this crate's version, so a changed lexer
//! never uses an old cache.
//!
//! The cache is only an optimization: a missing, stale, damaged or
//! unwritable cache means the lexer is built as usual (and the cache
//! rewritten when possible), never an error.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::Result;
use tfla::dsecfile::DSecFile;
use tfla::search::SearchEngine;

use crate::{Lexer, TokenKind};

/// Bump when the engine is built differently in a way the token patterns
/// do not show, such as a new DFA configuration.
const FORMAT: u32 = 1;

impl Lexer {
    /// The lexer, from the [cache directory](Self::cache_dir) when it holds
    /// one for this version of the lexer; otherwise built, then cached.
    pub fn cached() -> Result<Self> {
        match Self::cache_dir() {
            Some(dir) => Self::cached_in(&dir),
            None => Self::new(),
        }
    }

    /// [`Lexer::cached`], with the cache in `dir`.
    pub fn cached_in(dir: &Path) -> Result<Self> {
        let path = dir.join(file_name());
        if let Some(lexer) = Self::load(&path) {
            return Ok(lexer);
        }
        let lexer = Self::new()?;
        // Best effort: without a cache, the next run builds it again.
        let _ = lexer.save(&path);
        Ok(lexer)
    }

    /// `$NUX_CACHE_DIR`, otherwise `$XDG_CACHE_HOME/nux`, otherwise
    /// `~/.cache/nux`; `None` when none of these variables is set.
    pub fn cache_dir() -> Option<PathBuf> {
        let var = |name| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        var("NUX_CACHE_DIR")
            .or_else(|| var("XDG_CACHE_HOME").map(|d| d.join("nux")))
            .or_else(|| var("HOME").map(|h| h.join(".cache").join("nux")))
    }

    /// The cached lexer at `path`, if it is there and intact.
    fn load(path: &Path) -> Option<Self> {
        let dsf = DSecFile::from_file(path).ok()?;
        if dsf.get("KEY")?.as_str()? != key() {
            return None;
        }
        let kinds: Vec<TokenKind> = TokenKind::into_iter().collect();
        let engine = SearchEngine::from_dsf_with_kinds(&dsf, FORMAT, &kinds).ok()?;
        Some(Self { engine })
    }

    /// Writes the lexer to `path` atomically, so concurrent runs never read
    /// half a file, then removes caches of other lexer versions.
    fn save(&self, path: &Path) -> io::Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let mut dsf = DSecFile::new(path);
        dsf.set("KEY", key());
        self.engine.to_dsf(&mut dsf, FORMAT).map_err(io::Error::other)?;
        let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
        dsf.save_as(&temporary).map_err(io::Error::other)?;
        if let Err(error) = std::fs::rename(&temporary, path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        for entry in std::fs::read_dir(dir)?.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("lexer-") && name.ends_with(".dsf") && entry.path() != path {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
}

/// What a cache must have been made from: this crate's version, the cache
/// format, and every token kind with its pattern, in order.
fn key() -> String {
    let mut key = format!("nux-lexer {} format {FORMAT}", env!("CARGO_PKG_VERSION"));
    for kind in TokenKind::into_iter() {
        key.push_str(&format!("\n{kind} {}", kind.pattern()));
    }
    key
}

/// `lexer-<FNV-1a hash of the key>.dsf`.
fn file_name() -> String {
    let hash = key().bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x0100_0000_01b3)
    });
    format!("lexer-{hash:016x}.dsf")
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::SourceArena;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nux-lexer-cache-{}", std::process::id())).join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Every `.nux` file of the examples, lexed.
    fn lex_examples(lexer: &Lexer) -> Vec<String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example");
        let mut files = Vec::new();
        for repository in ["repository", "advanced/repository", "minimal/repository"] {
            for entry in std::fs::read_dir(root.join(repository)).unwrap() {
                files.push(entry.unwrap().path());
            }
        }
        files.sort();
        let mut arena = SourceArena::new();
        files
            .iter()
            .map(|file| {
                let id = arena.load(file).unwrap();
                let tokens = lexer.lex(arena.get(id).unwrap()).unwrap();
                tokens.iter().map(|t| format!("{:?} {:?}", t.kind, t.content)).collect::<Vec<_>>().join("\n")
            })
            .collect()
    }

    #[test]
    fn a_cached_lexer_lexes_like_a_built_one_and_loads_fast() {
        let dir = dir("same");
        let built = Lexer::cached_in(&dir).unwrap();
        let path = dir.join(file_name());
        assert!(path.is_file());

        let started = Instant::now();
        let loaded = Lexer::load(&path).expect("the cache was just written");
        assert!(started.elapsed() < Duration::from_millis(500), "{:?}", started.elapsed());
        assert_eq!(lex_examples(&loaded), lex_examples(&built));
        assert_eq!(lex_examples(&loaded), lex_examples(&Lexer::new().unwrap()));
    }

    #[test]
    fn bad_caches_are_rebuilt_and_old_ones_removed() {
        let dir = dir("bad");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(file_name());
        let old = dir.join("lexer-0000000000000000.dsf");
        std::fs::write(&old, "an older lexer").unwrap();

        // Garbage, then a cache from another version of the lexer.
        std::fs::write(&path, "not a lexer").unwrap();
        Lexer::cached_in(&dir).unwrap();
        assert!(Lexer::load(&path).is_some(), "rewritten");
        assert!(!old.exists());

        let mut other = DSecFile::from_file(&path).unwrap();
        other.set("KEY", "nux-lexer 0.0.0");
        other.save().unwrap();
        assert!(Lexer::load(&path).is_none());
        Lexer::cached_in(&dir).unwrap();
        assert!(Lexer::load(&path).is_some());

        // A damaged DFA is caught, not trusted.
        let mut damaged = DSecFile::from_file(&path).unwrap();
        let mut search = damaged.get("SEARCH").unwrap().data.clone();
        let middle = search.len() / 2;
        for byte in &mut search[middle..middle + 64] {
            *byte = !*byte;
        }
        damaged.set("SEARCH", search);
        damaged.save().unwrap();
        assert!(Lexer::load(&path).is_none());
        Lexer::cached_in(&dir).unwrap();
        assert!(Lexer::load(&path).is_some());
    }

    #[test]
    fn an_unwritable_cache_still_gives_a_lexer() {
        let file = dir("unwritable");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "a file where the directory should be").unwrap();
        let lexer = Lexer::cached_in(&file).unwrap();
        assert!(!lex_examples(&lexer).is_empty());
    }
}
