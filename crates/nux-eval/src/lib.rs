//! Evaluates parsed `.nux` programs into package definitions: one
//! [`VersionDef`] per `version(...)` call.

use std::collections::HashSet;

use nux_executor::Executor;
use nux_lexer::{Diagnostic, SourceArena, Span, render_diagnostic};
use nux_parser::ast;

mod build_systems;
mod builtins;
mod context;
mod evaluator;
mod host;
mod loader;
mod value;

pub use host::Host;
pub use loader::{LoadError, load_file, load_file_with_host};
pub use value::{Value, ValueType};

use evaluator::Evaluator;

#[derive(Debug, Clone)]
pub struct Source {
    pub kind: SourceKind,
    /// The git tag to check out; only valid with `SourceKind::Git`.
    pub tag: Option<String>,
    pub proxy: Option<String>,
}

#[derive(Debug, Clone)]
pub enum SourceKind {
    Url(String),
    Git(String),
    Local(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Dependency {
    Build { name: String, version: String },
    Runtime { name: String, version: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BashCommand {
    pub is_unsafe: bool,
    pub command: String,
}

/// Everything declared between one `version(...)` call and the next (or the
/// end of the program).
#[derive(Debug, Clone)]
pub struct VersionDef {
    pub version: String,
    pub commands: Vec<BashCommand>,
    pub source: Option<Source>,
    pub dependencies: HashSet<Dependency>,
    /// The executable registered by `set_bin`, relative to the install prefix.
    pub bin: Option<String>,
    /// The name of its link in `bin/`, when `set_bin` gives one; otherwise
    /// the executable's file name.
    pub bin_name: Option<String>,
    /// Libraries registered by `export_lib`, in declaration order.
    pub libraries: Vec<Library>,
    /// Set by `use_host`: the host already provides this version, so it is
    /// never downloaded or built.
    pub host: Option<HostBinary>,
}

impl VersionDef {
    fn new(version: String) -> Self {
        Self {
            version,
            commands: Vec::new(),
            source: None,
            dependencies: HashSet::new(),
            bin: None,
            bin_name: None,
            libraries: Vec::new(),
            host: None,
        }
    }
}

/// A library a package installs for its dependents to compile against.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Library {
    /// The name the linker uses: `z` for `-lz` and `libz.so`.
    pub name: String,
    /// Directory holding `lib<name>.so` or `lib<name>.a`, relative to the
    /// install prefix.
    pub lib: String,
    /// Header directory relative to the install prefix, if any.
    pub include: Option<String>,
}

/// An executable found on the host machine.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HostBinary {
    pub name: String,
    pub path: std::path::PathBuf,
}

/// A program and every version it declares, in declaration order.
pub struct EvalResult {
    pub name: String,
    pub versions: Vec<VersionDef>,
}

impl EvalResult {
    pub fn version(&self, version: &str) -> Option<&VersionDef> {
        self.versions.iter().find(|v| v.version == version)
    }
}

/// A runtime error located by `span`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalError {
    pub message: String,
    pub span: Span,
}

impl EvalError {
    fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }

    pub fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error(self.message.clone(), self.span)
    }

    pub fn render(&self, arena: &SourceArena) -> String {
        render_diagnostic(arena, &self.message, self.span)
    }
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EvalError {}

type Result<T> = std::result::Result<T, EvalError>;

/// Evaluates `program` against a host without executables, so the result
/// only depends on the program; see [`eval_program_with_host`].
pub fn eval_program(
    program: &ast::Program,
    executor: &mut Executor,
) -> std::result::Result<EvalResult, EvalError> {
    eval_program_with_host(program, executor, &Host::empty())
}

/// Evaluates `program`; `host_bin`, `host_version` and `use_host` look at `host`.
pub fn eval_program_with_host(
    program: &ast::Program,
    executor: &mut Executor,
    host: &Host,
) -> std::result::Result<EvalResult, EvalError> {
    Evaluator::new(executor, host).eval_program(program)
}

#[cfg(test)]
mod tests;
