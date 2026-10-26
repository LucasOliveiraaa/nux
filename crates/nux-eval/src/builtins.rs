//! Built-in functions such as `version`, `source` and `add_dependency`.

use nux_lexer::Span;
use nux_parser::ast::Ident;

use std::collections::HashMap;

use crate::build_systems::{BuildSystem, Common, assignment};
use crate::evaluator::{Evaluator, ParsedFuncArg};
use crate::{
    Dependency, EvalError, HostBinary, Library, Result, Source, SourceKind, Value, ValueType,
    VersionDef,
};

impl Evaluator<'_> {
    pub(crate) fn eval_function_call(
        &mut self,
        callee: &Ident,
        call_span: Span,
        args: Vec<ParsedFuncArg>,
    ) -> Result<Value> {
        match callee.name.as_str() {
            "print" => {
                for arg in args {
                    println!("{}", arg.value);
                }
                Ok(Value::Null)
            }
            "version" => {
                let [version] = resolve_function_args(
                    callee,
                    call_span,
                    args,
                    &[Param::required("version", ValueType::String)],
                )?;
                let version = version.unwrap().to_string();
                if self.context.has_version(&version) {
                    return Err(EvalError::new(
                        format!("version `{version}` is already declared"),
                        call_span,
                    ));
                }
                self.context.finish_version();
                self.context.current = Some(VersionDef::new(version));
                Ok(Value::Null)
            }
            "source" => {
                let [git, url, local, tag, proxy] = resolve_function_args(
                    callee,
                    call_span,
                    args,
                    &[
                        Param::optional("git", ValueType::String),
                        Param::optional("url", ValueType::String),
                        Param::optional("local", ValueType::String),
                        Param::optional("tag", ValueType::String),
                        Param::optional("proxy", ValueType::String),
                    ],
                )?;
                let kind = match (git, url, local) {
                    (Some(git), None, None) => SourceKind::Git(git.to_string()),
                    (None, Some(url), None) => SourceKind::Url(url.to_string()),
                    (None, None, Some(local)) => SourceKind::Local(local.to_string()),
                    _ => {
                        return Err(EvalError::new(
                            "`source` takes exactly one of `git`, `url` or `local`",
                            call_span,
                        ));
                    }
                };
                if tag.is_some() && !matches!(kind, SourceKind::Git(_)) {
                    return Err(EvalError::new(
                        "`tag` can only be used with `git`",
                        call_span,
                    ));
                }
                let current = self.context.current_version("`source`", call_span)?;
                if current.source.is_some() {
                    return Err(EvalError::new("source is already set", call_span));
                }
                current.source = Some(Source {
                    kind,
                    tag: tag.map(|t| t.to_string()),
                    proxy: proxy.map(|p| p.to_string()),
                });
                Ok(Value::Null)
            }
            "set_bin" => {
                let [path, name] = resolve_function_args(
                    callee,
                    call_span,
                    args,
                    &[
                        Param::required("path", ValueType::String),
                        Param::optional("name", ValueType::String),
                    ],
                )?;
                let path = prefix_path("set_bin", path.unwrap().to_string(), call_span)?;
                let name = name.map(|n| n.to_string());
                if let Some(name) = &name {
                    link_name(name, call_span)?;
                }
                let current = self.context.current_version("`set_bin`", call_span)?;
                if current.bin.is_some() {
                    return Err(EvalError::new("bin is already set", call_span));
                }
                current.bin = Some(path);
                current.bin_name = name;
                Ok(Value::Null)
            }
            "export_lib" => {
                let [name, lib, include] = resolve_function_args(
                    callee,
                    call_span,
                    args,
                    &[
                        Param::required("name", ValueType::String),
                        Param::optional("lib", ValueType::String),
                        Param::optional("include", ValueType::String),
                    ],
                )?;
                let name = name.unwrap().to_string();
                if name.is_empty() || name.contains(['/', ' ']) {
                    return Err(EvalError::new(
                        format!(
                            "`export_lib` expects a library name such as \"z\" for libz, found `{name}`"
                        ),
                        call_span,
                    ));
                }
                let lib = prefix_path(
                    "export_lib",
                    lib.map_or("lib".into(), |l| l.to_string()),
                    call_span,
                )?;
                // `include = ""` exports a library without headers.
                let include = match include.map_or("include".into(), |i| i.to_string()) {
                    include if include.is_empty() => None,
                    include => Some(prefix_path("export_lib", include, call_span)?),
                };
                let current = self.context.current_version("`export_lib`", call_span)?;
                if current.libraries.iter().any(|l| l.name == name) {
                    return Err(EvalError::new(
                        format!("library `{name}` is already exported"),
                        call_span,
                    ));
                }
                current.libraries.push(Library { name, lib, include });
                Ok(Value::Null)
            }
            "host_bin" => {
                let [name] = resolve_function_args(
                    callee,
                    call_span,
                    args,
                    &[Param::required("name", ValueType::String)],
                )?;
                Ok(self
                    .host
                    .find(&name.unwrap().to_string())
                    .map_or(Value::Null, |path| {
                        Value::String(path.display().to_string())
                    }))
            }
            "host_version" => {
                let [name, flag] = resolve_function_args(
                    callee,
                    call_span,
                    args,
                    &[
                        Param::required("name", ValueType::String),
                        Param::optional("flag", ValueType::String),
                    ],
                )?;
                let flag = flag.map_or("--version".into(), |f| f.to_string());
                Ok(self
                    .host
                    .version(&name.unwrap().to_string(), &flag)
                    .map_or(Value::Null, Value::String))
            }
            "use_host" => {
                let [name] = resolve_function_args(
                    callee,
                    call_span,
                    args,
                    &[Param::required("name", ValueType::String)],
                )?;
                let name = name.unwrap().to_string();
                let Some(path) = self.host.find(&name) else {
                    return Err(EvalError::new(
                        format!(
                            "the host has no `{name}` executable; guard this call with `if host_bin(\"{name}\")`"
                        ),
                        call_span,
                    ));
                };
                let current = self.context.current_version("`use_host`", call_span)?;
                if current.host.is_some() {
                    return Err(EvalError::new("host is already set", call_span));
                }
                current.host = Some(HostBinary { name, path });
                Ok(Value::Null)
            }
            "add_dependency" => {
                let [name, version, ty] = resolve_function_args(
                    callee,
                    call_span,
                    args,
                    &[
                        Param::required("name", ValueType::String),
                        Param::optional("version", ValueType::String),
                        Param::optional("type", ValueType::String),
                    ],
                )?;
                let name = name.unwrap().to_string();
                let version = version
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "latest".to_string());
                let ty = ty
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "build".to_string());
                let dependency = match ty.as_str() {
                    "build" => Dependency::Build { name, version },
                    "runtime" => Dependency::Runtime { name, version },
                    _ => {
                        return Err(EvalError::new(
                            format!("invalid dependency type `{ty}`; must be `build` or `runtime`"),
                            call_span,
                        ));
                    }
                };

                self.context
                    .current_version("`add_dependency`", call_span)?
                    .dependencies
                    .insert(dependency);
                Ok(Value::Null)
            }
            "exec" => {
                let [program, arguments] = resolve_function_args(
                    callee,
                    call_span,
                    args,
                    &[
                        Param::required("program", ValueType::String),
                        Param::optional("args", ValueType::List),
                    ],
                )?;
                let program = program.unwrap().to_string();
                let arguments = string_list(arguments, "args", callee, call_span)?;
                if program.is_empty() {
                    return Err(EvalError::new("`exec` needs a program to run", call_span));
                }
                let output = self.executor.run(&program, &arguments).map_err(|e| {
                    EvalError::new(format!("cannot run `{program}`: {e}"), call_span)
                })?;
                Ok(Value::Object(HashMap::from([
                    ("success".to_string(), Value::Boolean(output.success())),
                    ("status".to_string(), Value::Number(output.status as f64)),
                    ("stdout".to_string(), Value::String(output.stdout)),
                    ("stderr".to_string(), Value::String(output.stderr)),
                ])))
            }
            "trim" => {
                let [text] = resolve_function_args(
                    callee,
                    call_span,
                    args,
                    &[Param::required("text", ValueType::String)],
                )?;
                Ok(Value::String(text.unwrap().to_string().trim().to_string()))
            }
            "cmake" | "meson" | "make" | "autotools" | "cargo" | "go" => {
                self.build_system(callee, call_span, args)
            }
            _ => Err(EvalError::new(
                format!("undefined function `{}`", callee.name),
                callee.span,
            )),
        }
    }
}

impl Evaluator<'_> {
    /// `cmake(...)`, `meson(...)` and the other build-system helpers: adds
    /// the commands of a standard build to the current version.
    fn build_system(
        &mut self,
        callee: &Ident,
        call_span: Span,
        args: Vec<ParsedFuncArg>,
    ) -> Result<Value> {
        use ValueType::{Boolean, List, Number, String as Str};
        let name = callee.name.as_str();
        let specific = match name {
            "cmake" => vec![
                Param::optional("build_type", Str),
                Param::optional("build_dir", Str),
                Param::optional("generator", Str),
            ],
            "meson" => vec![
                Param::optional("build_type", Str),
                Param::optional("build_dir", Str),
            ],
            "make" => vec![
                Param::optional("targets", List),
                Param::optional("install", Str),
                Param::optional("prefix_var", Str),
            ],
            "autotools" => vec![Param::optional("reconf", Boolean)],
            "cargo" => vec![
                Param::optional("features", List),
                Param::optional("locked", Boolean),
            ],
            "go" => vec![
                Param::optional("packages", List),
                Param::optional("ldflags", Str),
            ],
            _ => unreachable!("not a build-system helper: {name}"),
        };
        let mut params = vec![
            Param::optional("options", List),
            Param::optional("dir", Str),
            Param::optional("prefix", Str),
            Param::optional("jobs", Number),
            Param::optional("env", List),
        ];
        params.extend(specific);
        let mut values = resolve_args(callee, call_span, args, &params)?.into_iter();
        let mut next = || values.next().flatten();

        let mut common = Common {
            options: string_list(next(), "options", callee, call_span)?,
            ..Common::default()
        };
        if let Some(dir) = next() {
            common.dir = dir.to_string();
        }
        if let Some(prefix) = next() {
            common.prefix = prefix.to_string();
        }
        if let Some(Value::Number(jobs)) = next() {
            if jobs < 1.0 || jobs.fract() != 0.0 {
                return Err(EvalError::new(
                    format!("`jobs` of `{name}` must be a positive whole number"),
                    call_span,
                ));
            }
            common.jobs = (jobs as u64).to_string();
        }
        for item in string_list(next(), "env", callee, call_span)? {
            let pair = assignment(&item).ok_or_else(|| {
                EvalError::new(
                    format!("`env` of `{name}` expects `NAME=value` entries, found `{item}`"),
                    call_span,
                )
            })?;
            common.env.push(pair);
        }

        let string = |value: Option<Value>, default: &str| {
            value.map_or(default.to_string(), |v| v.to_string())
        };
        let flag = |value: Option<Value>| matches!(value, Some(Value::Boolean(true)));
        let system = match name {
            "cmake" => BuildSystem::Cmake {
                build_type: string(next(), "Release"),
                build_dir: string(next(), "build"),
                generator: next().map(|v| v.to_string()),
            },
            "meson" => BuildSystem::Meson {
                build_type: string(next(), "release"),
                build_dir: string(next(), "build"),
            },
            "make" => BuildSystem::Make {
                targets: string_list(next(), "targets", callee, call_span)?,
                // `install = ""` skips the install step.
                install: Some(string(next(), "install")).filter(|t| !t.is_empty()),
                prefix_var: string(next(), "PREFIX"),
            },
            "autotools" => BuildSystem::Autotools {
                reconf: flag(next()),
            },
            "cargo" => BuildSystem::Cargo {
                features: string_list(next(), "features", callee, call_span)?,
                locked: flag(next()),
            },
            "go" => {
                let packages = match next() {
                    None => vec![".".to_string()],
                    packages => string_list(packages, "packages", callee, call_span)?,
                };
                BuildSystem::Go {
                    packages,
                    ldflags: next().map(|v| v.to_string()),
                }
            }
            _ => unreachable!(),
        };

        let what = format!("`{name}`");
        self.context
            .current_version(&what, call_span)?
            .commands
            .extend(system.commands(&common));
        Ok(Value::Null)
    }
}

/// The strings of a list argument; an absent argument is an empty list.
fn string_list(
    value: Option<Value>,
    param: &str,
    callee: &Ident,
    span: Span,
) -> Result<Vec<String>> {
    let Some(Value::List(items)) = value else {
        return Ok(Vec::new());
    };
    items
        .into_iter()
        .map(|item| match item {
            Value::String(s) => Ok(s),
            other => Err(EvalError::new(
                format!(
                    "`{param}` of `{}` must be a list of strings, found a {} in it",
                    callee.name,
                    other.get_type()
                ),
                span,
            )),
        })
        .collect()
}

pub(crate) struct Param {
    name: &'static str,
    ty: ValueType,
    required: bool,
}

impl Param {
    pub(crate) fn required(name: &'static str, ty: ValueType) -> Self {
        Self {
            name,
            ty,
            required: true,
        }
    }

    pub(crate) fn optional(name: &'static str, ty: ValueType) -> Self {
        Self {
            name,
            ty,
            required: false,
        }
    }
}

/// Matches call arguments to `expected` parameters. Positional arguments fill
/// parameters in order and must precede named ones. Returns one slot per
/// parameter; required slots are always `Some`.
pub(crate) fn resolve_function_args<const N: usize>(
    callee: &Ident,
    call_span: Span,
    args: Vec<ParsedFuncArg>,
    expected: &[Param; N],
) -> Result<[Option<Value>; N]> {
    let resolved = resolve_args(callee, call_span, args, expected)?;
    Ok(resolved
        .try_into()
        .unwrap_or_else(|_| unreachable!("one slot per parameter")))
}

/// [`resolve_function_args`] for a parameter list built at run time.
pub(crate) fn resolve_args(
    callee: &Ident,
    call_span: Span,
    args: Vec<ParsedFuncArg>,
    expected: &[Param],
) -> Result<Vec<Option<Value>>> {
    let n = expected.len();
    let mut resolved: Vec<Option<Value>> = vec![None; n];
    let mut next_positional = 0;
    let mut seen_named = false;
    let function = &callee.name;

    for arg in args {
        let index = match &arg.name {
            None if seen_named => {
                return Err(EvalError::new(
                    "positional arguments must come before named arguments",
                    arg.span,
                ));
            }
            None if next_positional >= n => {
                return Err(EvalError::new(
                    format!("`{function}` takes at most {n} argument(s)"),
                    arg.span,
                ));
            }
            None => {
                next_positional += 1;
                next_positional - 1
            }
            Some(name) => {
                seen_named = true;
                let Some(index) = expected.iter().position(|p| p.name == name.name) else {
                    return Err(EvalError::new(
                        format!("`{function}` has no parameter named `{}`", name.name),
                        name.span,
                    ));
                };
                index
            }
        };

        let param = &expected[index];
        if resolved[index].is_some() {
            return Err(EvalError::new(
                format!("argument `{}` is given more than once", param.name),
                arg.span,
            ));
        }
        if arg.value.value_type() != param.ty {
            return Err(EvalError::new(
                format!(
                    "argument `{}` of `{function}` must be a {}, found {}",
                    param.name,
                    param.ty,
                    arg.value.get_type()
                ),
                arg.span,
            ));
        }
        resolved[index] = Some(arg.value);
    }

    if let Some(missing) = expected
        .iter()
        .zip(&resolved)
        .find(|(param, value)| param.required && value.is_none())
    {
        return Err(EvalError::new(
            format!("missing argument `{}` for `{function}`", missing.0.name),
            call_span,
        ));
    }

    Ok(resolved)
}

/// Checks that `path` stays inside the install prefix.
fn prefix_path(function: &str, path: String, span: Span) -> Result<String> {
    let escapes = std::path::Path::new(&path).components().any(|c| {
        !matches!(
            c,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    });
    if path.is_empty() || escapes {
        return Err(EvalError::new(
            format!("`{function}` expects a path inside the install prefix, found `{path}`"),
            span,
        ));
    }
    Ok(path)
}

/// Checks a `set_bin` link name: one file name in `bin/`, which Nux must be
/// able to tell from its own temporary files (`.<name>.tmp`).
fn link_name(name: &str, span: Span) -> std::result::Result<(), EvalError> {
    let problem = if name.is_empty() {
        Some("must not be empty")
    } else if name.contains('/') {
        Some("must be a file name, without `/`")
    } else if name.starts_with('.') {
        Some("must not start with `.`")
    } else if name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        Some("must not contain spaces or control characters")
    } else {
        None
    };
    match problem {
        Some(problem) => Err(EvalError::new(
            format!("the link name `{name}` {problem}"),
            span,
        )),
        None => Ok(()),
    }
}
