use std::path::PathBuf;

use nux_eval::{Dependency, LoadError, SourceKind, load_file};
use nux_executor::Executor;
use nux_lexer::SourceArena;

/// Writes `contents` to a fresh file under cargo's per-test temp dir.
fn write(name: &str, contents: impl AsRef<[u8]>) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("loader");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn load(path: &PathBuf) -> (SourceArena, Result<nux_eval::EvalResult, LoadError>) {
    let mut arena = SourceArena::new();
    let result = load_file(path, &mut arena, &mut Executor::new());
    (arena, result)
}

#[test]
fn loads_and_evaluates_a_file() {
    let path = write(
        "ok.nux",
        r#"program cmake {
    let version = "4.2.0";
    version(version);
    source(git = "https://github.com/Kitware/CMake.git");
    add_dependency(gcc);

    bash {
        ./bootstrap --prefix=$NUX_PREFIX -- v${version}
        make -j2
    }
}
"#,
    );
    let (arena, result) = load(&path);
    let result = result.unwrap_or_else(|e| panic!("{}", e.render(&arena)));

    assert_eq!(result.name, "cmake");
    let [result] = &result.versions[..] else { panic!("expected one version") };
    assert_eq!(result.version, "4.2.0");
    assert!(result.dependencies.contains(&Dependency::Build { name: "gcc".into(), version: "latest".into() }));
    assert!(matches!(result.source.as_ref().unwrap().kind, SourceKind::Git(_)));
    assert_eq!(
        result.commands,
        ["./bootstrap --prefix=$NUX_PREFIX -- v4.2.0", "make -j2"]
    );

    // The file is registered in the arena under the path it was loaded from.
    let file = arena.iter().next().unwrap();
    assert_eq!(file.path(), path);
}

#[test]
fn missing_file_is_an_io_error() {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("does-not-exist.nux");
    let (arena, result) = load(&path);
    let err = result.err().unwrap();
    assert!(matches!(err, LoadError::Io { .. }));
    assert!(err.span().is_none());
    let rendered = err.render(&arena);
    assert!(rendered.starts_with(&format!("error: failed to read `{}`: ", path.display())));
    assert_eq!(arena.iter().count(), 0);
}

#[test]
fn invalid_utf8_is_an_io_error() {
    let path = write("binary.nux", [b'p', 0xff, 0xfe]);
    let (_, result) = load(&path);
    assert!(matches!(result.err().unwrap(), LoadError::Io { .. }));
}

#[test]
fn lex_error_diagnostic() {
    let path = write("lex.nux", "program p {\n    let s = \"ünïcode;\n}\n");
    let (arena, result) = load(&path);
    let err = result.err().unwrap();
    assert!(matches!(err, LoadError::Lex(_)));
    assert_eq!(
        err.render(&arena),
        format!(
            "error: unterminated string literal\n \
             --> {}:2:13\n  \
             |\n\
             2 |     let s = \"ünïcode;\n  \
             |             ^\n",
            path.display()
        )
    );
}

#[test]
fn parse_error_diagnostic_at_end_of_file() {
    let path = write("parse.nux", "program p {\n    let x = 1;\n");
    let (arena, result) = load(&path);
    let err = result.err().unwrap();
    assert!(matches!(err, LoadError::Parse(_)));
    assert_eq!(
        err.render(&arena),
        format!(
            "error: expected `}}`, found end of input\n \
             --> {}:3:1\n  \
             |\n\
             3 | \n  \
             | ^\n",
            path.display()
        )
    );
}

#[test]
fn eval_error_diagnostic_spanning_lines() {
    let path = write(
        "eval.nux",
        "program p {\n    version(\"1\");\n    source(git = \"a\",\n           url = \"b\");\n}\n",
    );
    let (arena, result) = load(&path);
    let err = result.err().unwrap();
    assert!(matches!(err, LoadError::Eval(_)));
    assert_eq!(
        err.render(&arena),
        format!(
            "error: `source` takes exactly one of `git`, `url` or `local`\n \
             --> {}:3:5\n  \
             |\n\
             3 |     source(git = \"a\",\n  \
             |     ^^^^^^^^^^^^^^^^^\n\
             4 |            url = \"b\");\n  \
             | ^^^^^^^^^^^^^^^^^^^^^\n",
            path.display()
        )
    );
}

#[test]
fn spans_resolve_to_their_own_file() {
    let good = write("multi-good.nux", "program a { version(\"1\"); }");
    let bad = write("multi-bad.nux", "program b { version(1); }");

    let mut arena = SourceArena::new();
    let mut executor = Executor::new();
    load_file(&good, &mut arena, &mut executor).unwrap();
    let err = load_file(&bad, &mut arena, &mut executor).err().unwrap();

    let span = err.span().unwrap();
    assert_eq!(arena.path(span.file()), Some(bad.as_path()));
    assert_eq!(arena.slice(span), Some("1"));
    assert!(err.render(&arena).contains(&format!("--> {}:1:21", bad.display())));
    assert_eq!(arena.iter().count(), 2);
}
