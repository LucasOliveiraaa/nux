//! Loading a `.nux` file from disk: read, register, lex, parse and evaluate.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use nux_executor::Executor;
use nux_lexer::{LexError, Lexer, SourceArena, Span, render_diagnostic};
use nux_parser::ParseError;

use crate::{EvalError, EvalResult, Host, eval_program_with_host};

/// Any error produced while loading a file.
#[derive(Debug)]
pub enum LoadError {
    /// The file could not be read (missing, unreadable or not UTF-8).
    Io {
        path: PathBuf,
        error: std::io::Error,
    },
    /// The lexer itself could not be built.
    LexerInit(String),
    Lex(LexError),
    Parse(ParseError),
    Eval(EvalError),
}

impl LoadError {
    /// The source location of the error, if it has one.
    pub fn span(&self) -> Option<Span> {
        match self {
            LoadError::Io { .. } | LoadError::LexerInit(_) => None,
            LoadError::Lex(e) => Some(e.span),
            LoadError::Parse(e) => Some(e.span),
            LoadError::Eval(e) => Some(e.span),
        }
    }

    /// Renders the error, with a source excerpt when it has a span.
    pub fn render(&self, arena: &SourceArena) -> String {
        match self.span() {
            Some(span) => render_diagnostic(arena, &self.to_string(), span),
            None => format!("error: {self}\n"),
        }
    }
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io { path, error } => {
                write!(f, "failed to read `{}`: {error}", path.display())
            }
            LoadError::LexerInit(message) => write!(f, "failed to build the lexer: {message}"),
            LoadError::Lex(e) => e.fmt(f),
            LoadError::Parse(e) => e.fmt(f),
            LoadError::Eval(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoadError::Io { error, .. } => Some(error),
            LoadError::LexerInit(_) => None,
            LoadError::Lex(e) => Some(e),
            LoadError::Parse(e) => Some(e),
            LoadError::Eval(e) => Some(e),
        }
    }
}

impl From<LexError> for LoadError {
    fn from(e: LexError) -> Self {
        LoadError::Lex(e)
    }
}

impl From<ParseError> for LoadError {
    fn from(e: ParseError) -> Self {
        LoadError::Parse(e)
    }
}

impl From<EvalError> for LoadError {
    fn from(e: EvalError) -> Self {
        LoadError::Eval(e)
    }
}

/// Building the lexer's DFA is expensive, so it is done once per process.
fn shared_lexer() -> Result<&'static Lexer, LoadError> {
    static LEXER: OnceLock<Result<Lexer, String>> = OnceLock::new();
    LEXER
        .get_or_init(|| Lexer::cached().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| LoadError::LexerInit(e.clone()))
}

/// [`load_file_with_host`] against a host without executables.
pub fn load_file(
    path: impl AsRef<Path>,
    arena: &mut SourceArena,
    executor: &mut Executor,
) -> Result<EvalResult, LoadError> {
    load_file_with_host(path, arena, executor, &Host::empty())
}

/// Reads `path`, registers it in `arena`, then lexes, parses and evaluates it
/// with `executor`.
///
/// The file stays registered even when a later stage fails, so the returned
/// error can be rendered against `arena` with [`LoadError::render`].
pub fn load_file_with_host(
    path: impl AsRef<Path>,
    arena: &mut SourceArena,
    executor: &mut Executor,
    host: &Host,
) -> Result<EvalResult, LoadError> {
    let path = path.as_ref();
    let id = arena.load(path).map_err(|error| LoadError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let file = arena.get(id).expect("file was just registered");

    let tokens = shared_lexer()?.lex(file)?;
    let program = nux_parser::parse(&tokens)?;
    Ok(eval_program_with_host(&program, executor, host)?)
}
