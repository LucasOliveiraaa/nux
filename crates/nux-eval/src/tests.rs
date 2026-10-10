use std::sync::OnceLock;

use nux_lexer::Lexer;

use super::*;

fn lexer() -> &'static Lexer {
    static LEXER: OnceLock<Lexer> = OnceLock::new();
    LEXER.get_or_init(|| Lexer::new().unwrap())
}

fn parse(src: &str) -> (SourceArena, ast::Program) {
    let mut arena = SourceArena::new();
    let id = arena.add("test.nux", src);
    let tokens = lexer().lex(arena.get(id).unwrap()).unwrap();
    let program = nux_parser::parse(&tokens).unwrap_or_else(|e| panic!("{}", e.render(&arena)));
    (arena, program)
}

/// Evaluates `body` inside a program that already sets a version.
fn run(body: &str) -> std::result::Result<EvalResult, (SourceArena, EvalError)> {
    let (arena, program) = parse(&format!("program p {{ version(\"1\"); {body} }}"));
    eval_program(&program, &mut Executor::new()).map_err(|e| (arena, e))
}

/// Runs `body`, expecting an error; returns its message and the source text it points at.
fn run_err(body: &str) -> (String, String) {
    let Err((arena, err)) = run(body) else {
        panic!("expected an error for {body:?}")
    };
    let text = arena.slice(err.span).unwrap().to_string();
    (err.message, text)
}

/// The commands produced by `bash { ... }` blocks, as a quick way to observe values.
fn commands(body: &str) -> Vec<String> {
    match run(body) {
        Ok(result) => result.versions.into_iter().flat_map(|v| v.commands).collect(),
        Err((arena, e)) => panic!("{}", e.render(&arena)),
    }
}

#[test]
fn undefined_interpolation_is_an_error_not_a_panic() {
    assert_eq!(
        run_err(r#"let s = "a ${nope} b";"#),
        (
            "undefined variable `nope` in string interpolation".into(),
            r#""a ${nope} b""#.into()
        )
    );
}

#[test]
fn bash_interpolation_keeps_shell_variables() {
    assert_eq!(
        commands(r#"let v = "4.2"; bash { echo ${v} ${HOME} $PATH }"#),
        ["echo 4.2 ${HOME} $PATH"]
    );
}

#[test]
fn missing_version_points_at_program_name() {
    let (arena, program) = parse("program cmake { }");
    let err = eval_program(&program, &mut Executor::new()).err().unwrap();
    assert_eq!(arena.slice(err.span), Some("cmake"));
    assert!(err.message.contains("does not specify a version"));
}

#[test]
fn builtins_populate_result() {
    let (_, program) = parse(
        r#"program cmake {
            let version = "4.2.0";
            version(version);
            source(git = "https://example.com/${version}.git", proxy = "socks5://p");
            add_dependency(gcc);
            add_dependency("git");
            add_dependency(gcc);
        }"#,
    );
    let result = eval_program(&program, &mut Executor::new()).ok().unwrap();
    assert_eq!(result.name, "cmake");
    let [result] = &result.versions[..] else { panic!("expected one version") };
    assert_eq!(result.version, "4.2.0");
    assert!(result.dependencies.contains(&Dependency::Build { name: "gcc".into(), version: "latest".into() }));
    assert!(result.dependencies.contains(&Dependency::Build { name: "git".into(), version: "latest".into() }));
    let source = result.source.clone().unwrap();
    assert!(matches!(source.kind, SourceKind::Git(ref g) if g == "https://example.com/4.2.0.git"));
    assert_eq!(source.proxy.as_deref(), Some("socks5://p"));
}

#[test]
fn logical_operators_short_circuit() {
    // Used to fail with "Division by zero": both sides were always evaluated.
    assert_eq!(
        commands(
            "let x = 0; if x != 0 && 10 / x > 1 { bash {a} } if x == 0 || 1 / x > 0 { bash {b} }"
        ),
        ["b"]
    );
    assert_eq!(
        run_err("let x = 1 && true;"),
        (
            "binary `&&` expects booleans, found number".into(),
            "1 && true".into()
        )
    );
    // The right side is only evaluated (and type-checked) when needed.
    assert!(run("let x = true || 1;").is_ok());
    assert_eq!(
        run_err("let x = false || 1;"),
        (
            "binary `||` expects booleans, found number".into(),
            "false || 1".into()
        )
    );
}

#[test]
fn scopes_shadow_assign_and_are_freed() {
    assert_eq!(
        commands(
            "let i = 0; let total = 0;
             while i < 4 { let tmp = i * 2; total = total + tmp; i = i + 1; }
             if true { let i = 100; } bash { ${i} ${total} }"
        ),
        ["4 12"]
    );

    let (_, program) =
        parse("program p { version(\"1\"); let i = 0; while i < 50 { if true { i = i + 1; } } }");
    let mut executor = Executor::new();
    let host = Host::empty();
    let mut evaluator = Evaluator::new(&mut executor, &host);
    evaluator.eval_program(&program).ok().unwrap();
    // Only the root scope survives; exited scopes used to be kept forever.
    assert_eq!(evaluator.context.scopes.len(), 1);
}

#[test]
fn block_locals_do_not_leak() {
    assert_eq!(
        run_err("if true { let inner = 1; } let x = inner;"),
        ("undefined variable `inner`".into(), "inner".into())
    );
    assert_eq!(
        run_err("nope = 1;"),
        (
            "cannot assign to undefined variable `nope`".into(),
            "nope".into()
        )
    );
}

#[test]
fn arithmetic_and_grouping() {
    assert_eq!(
        commands(
            "let a = (1 + 2) * 3 - 7 % 4; let b = !(a == 6); let c = 6 & 3 | 8; bash { ${a} ${b} ${c} }"
        ),
        ["6 false 10"]
    );
    // `--` is no longer a comment marker: this is `5 - (-3)`.
    assert_eq!(commands("let d = 5--3; bash { ${d} }"), ["8"]);
    assert_eq!(
        run_err("let x = 1 % 0;"),
        ("modulo by zero".into(), "1 % 0".into())
    );
    assert_eq!(
        run_err("let x = 1 / (2 - 2);"),
        ("division by zero".into(), "1 / (2 - 2)".into())
    );
}

#[test]
fn type_errors_point_at_expression() {
    assert_eq!(
        run_err(r#"let x = 1 + "a";"#),
        (
            "binary `+` cannot be applied to number and string".into(),
            r#"1 + "a""#.into()
        )
    );
    assert_eq!(
        run_err(r#"let x = -"a";"#),
        (
            "unary `-` cannot be applied to a string".into(),
            r#"-"a""#.into()
        )
    );
    assert_eq!(
        run_err("let x = true & 1;"),
        (
            "binary `&` cannot be applied to boolean and number".into(),
            "true & 1".into()
        )
    );
}

#[test]
fn member_access() {
    let expected = std::env::consts::ARCH.to_string();
    assert_eq!(commands("let a = nux.arch; bash { ${a} }"), [expected]);
    assert_eq!(
        run_err("let a = nux.nope;"),
        ("no member `nope` on this object".into(), "nope".into())
    );
    assert_eq!(
        run_err("let n = 1; let a = n.x;"),
        ("cannot access member `x` of a number".into(), "n.x".into())
    );
}

#[test]
fn argument_resolution_errors() {
    assert_eq!(
        run_err("frobnicate();"),
        (
            "undefined function `frobnicate`".into(),
            "frobnicate".into()
        )
    );
    assert_eq!(
        run_err("version(\"1\");"),
        ("version `1` is already declared".into(), "version(\"1\")".into())
    );
    assert_eq!(
        run_err("add_dependency();"),
        (
            "missing argument `name` for `add_dependency`".into(),
            "add_dependency()".into()
        )
    );
    assert_eq!(
        run_err("add_dependency(1);"),
        (
            "argument `name` of `add_dependency` must be a string, found number".into(),
            "1".into()
        )
    );
    assert_eq!(
        run_err("add_dependency(\"a\", \"b\", \"build\", \"c\");"),
        (
            "`add_dependency` takes at most 3 argument(s)".into(),
            "\"c\"".into()
        )
    );
    assert_eq!(
        run_err("source(git = \"a\", branch = \"v1\");"),
        ("`source` has no parameter named `branch`".into(), "branch".into())
    );
    assert_eq!(
        run_err("source(git = \"a\", git = \"b\");"),
        (
            "argument `git` is given more than once".into(),
            "git = \"b\"".into()
        )
    );
    assert_eq!(
        run_err("source(url = \"a\", \"b\");"),
        (
            "positional arguments must come before named arguments".into(),
            "\"b\"".into()
        )
    );
    assert_eq!(
        run_err("source(git = \"a\", url = \"b\");"),
        (
            "`source` takes exactly one of `git`, `url` or `local`".into(),
            "source(git = \"a\", url = \"b\")".into()
        )
    );
}

fn eval(src: &str) -> EvalResult {
    let (arena, program) = parse(src);
    eval_program(&program, &mut Executor::new()).unwrap_or_else(|e| panic!("{}", e.render(&arena)))
}

#[test]
fn each_version_has_its_own_declarations() {
    let result = eval(
        r#"program cmake {
            let repo = "https://example.com/cmake.git";

            version("3.0.0");
            source(git = repo);
            add_dependency(gcc, "^9");
            bash { make v3 }

            let v = "4.2.0";
            version(v);
            source(url = "https://example.com/${v}.tar.gz");
            add_dependency(gcc, ">=12", "build");
            add_dependency(git, type = "runtime");
            bash { make v${v} }
        }"#,
    );
    assert_eq!(result.name, "cmake");
    let versions: Vec<_> = result.versions.iter().map(|v| v.version.as_str()).collect();
    assert_eq!(versions, ["3.0.0", "4.2.0"]);

    let old = result.version("3.0.0").unwrap();
    assert_eq!(old.commands, ["make v3"]);
    assert!(matches!(old.source.as_ref().unwrap().kind, SourceKind::Git(_)));
    assert_eq!(
        old.dependencies,
        HashSet::from([Dependency::Build { name: "gcc".into(), version: "^9".into() }])
    );

    let new = result.version("4.2.0").unwrap();
    assert_eq!(new.commands, ["make v4.2.0"]);
    assert!(matches!(new.source.as_ref().unwrap().kind, SourceKind::Url(ref u) if u.ends_with("4.2.0.tar.gz")));
    assert_eq!(
        new.dependencies,
        HashSet::from([
            Dependency::Build { name: "gcc".into(), version: ">=12".into() },
            Dependency::Runtime { name: "git".into(), version: "latest".into() },
        ])
    );
}

#[test]
fn last_version_is_finalized_at_end_of_program() {
    let result = eval(r#"program p { version("1"); version("2"); bash { last } }"#);
    assert_eq!(result.versions.len(), 2);
    assert!(result.versions[0].commands.is_empty());
    assert_eq!(result.versions[1].version, "2");
    assert_eq!(result.versions[1].commands, ["last"]);
}

#[test]
fn version_can_be_declared_conditionally() {
    let result = eval(
        r#"program p {
            let i = 1;
            while i <= 3 { version("${i}.0"); bash { build ${i} } i = i + 1; }
        }"#,
    );
    let summary: Vec<_> = result
        .versions
        .iter()
        .map(|v| (v.version.as_str(), v.commands.clone()))
        .collect();
    assert_eq!(
        summary,
        [("1.0", vec!["build 1".to_string()]), ("2.0", vec!["build 2".into()]), ("3.0", vec!["build 3".into()])]
    );
}

#[test]
fn source_may_be_set_once_per_version() {
    let result = eval(r#"program p { version("1"); source(git = "a"); version("2"); source(git = "b"); }"#);
    let sources: Vec<_> = result
        .versions
        .iter()
        .map(|v| match &v.source.as_ref().unwrap().kind {
            SourceKind::Git(g) => g.as_str(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(sources, ["a", "b"]);

    assert_eq!(
        run_err(r#"source(git = "a"); source(git = "b");"#),
        ("source is already set".into(), r#"source(git = "b")"#.into())
    );
}

#[test]
fn declarations_before_version_are_errors() {
    for (body, message, text) in [
        ("add_dependency(gcc);", "`add_dependency` must come after a `version(...)` call", "add_dependency(gcc)"),
        (r#"source(git = "a");"#, "`source` must come after a `version(...)` call", r#"source(git = "a")"#),
        ("bash { make }", "`bash` block must come after a `version(...)` call", "bash { make }"),
    ] {
        let src = format!("program p {{ {body} version(\"1\"); }}");
        let (arena, program) = parse(&src);
        let err = eval_program(&program, &mut Executor::new()).err().unwrap();
        assert_eq!(err.message, message);
        assert_eq!(arena.slice(err.span), Some(text));
    }
}

#[test]
fn duplicate_version_is_an_error() {
    assert_eq!(
        run_err(r#"version("2"); version("1");"#),
        ("version `1` is already declared".into(), r#"version("1")"#.into())
    );
}

#[test]
fn git_tag_and_set_bin() {
    let result = eval(
        r#"program cmake {
            let version = "4.2.0";
            version(version);
            source(git = "https://example.com/cmake.git", tag = "v${version}");
            set_bin("bin/cmake");
            version("5.0.0");
        }"#,
    );
    let first = &result.versions[0];
    assert_eq!(first.source.as_ref().unwrap().tag.as_deref(), Some("v4.2.0"));
    assert_eq!(first.bin.as_deref(), Some("bin/cmake"));
    // `set_bin` belongs to the version it was declared in.
    assert_eq!(result.versions[1].bin, None);

    assert_eq!(
        run_err(r#"source(url = "a", tag = "v1");"#),
        ("`tag` can only be used with `git`".into(), r#"source(url = "a", tag = "v1")"#.into())
    );
    assert_eq!(
        run_err(r#"set_bin("a"); set_bin("b");"#),
        ("bin is already set".into(), r#"set_bin("b")"#.into())
    );
    for path in ["", "../escape", "/abs/path"] {
        let (message, _) = run_err(&format!("set_bin(\"{path}\");"));
        assert!(message.starts_with("`set_bin` expects a path inside the install prefix"), "{path}");
    }
}

#[test]
fn export_lib_registers_libraries() {
    let result = eval(
        r#"program zlib {
            version("1.3.1");
            export_lib("z");
            export_lib(name = "zstatic", lib = "lib64", include = "");
        }"#,
    );
    assert_eq!(
        result.versions[0].libraries,
        [
            Library { name: "z".into(), lib: "lib".into(), include: Some("include".into()) },
            Library { name: "zstatic".into(), lib: "lib64".into(), include: None },
        ]
    );
    assert_eq!(
        run_err(r#"export_lib("z"); export_lib("z");"#),
        ("library `z` is already exported".into(), r#"export_lib("z")"#.into())
    );
    assert_eq!(
        run_err(r#"export_lib("lib/z");"#).0,
        "`export_lib` expects a library name such as \"z\" for libz, found `lib/z`"
    );
    assert_eq!(
        run_err(r#"export_lib("z", lib = "../lib");"#).0,
        "`export_lib` expects a path inside the install prefix, found `../lib`"
    );
}

/// A host whose `gcc` reports `version`.
fn host_with_gcc(name: &str, version: &str) -> Host {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("nux-eval-host-{}", std::process::id())).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let gcc = dir.join("gcc");
    std::fs::write(&gcc, format!("#!/bin/sh\necho 'gcc (Debian {version}-1) {version}'\n")).unwrap();
    std::fs::set_permissions(&gcc, std::fs::Permissions::from_mode(0o755)).unwrap();
    Host::new(vec![dir], vec![])
}

const GCC: &str = r#"program gcc {
    let host = host_version("gcc");
    if host {
        version(host);
        use_host("gcc");
    }
    if host != "14.2.0" {
        version("14.2.0");
        bash { make }
    }
}"#;

fn eval_on(host: &Host, src: &str) -> EvalResult {
    let (arena, program) = parse(src);
    eval_program_with_host(&program, &mut Executor::new(), host).unwrap_or_else(|e| panic!("{}", e.render(&arena)))
}

#[test]
fn recipes_see_host_executables() {
    let summary = |result: EvalResult| -> Vec<(String, bool)> {
        result.versions.into_iter().map(|v| (v.version, v.host.is_some())).collect()
    };

    // An older host compiler is offered next to the buildable version.
    let host = host_with_gcc("older", "12.2.0");
    let result = eval_on(&host, GCC);
    assert_eq!(result.versions[0].host.as_ref().unwrap().path, host.find("gcc").unwrap());
    assert_eq!(summary(result), [("12.2.0".into(), true), ("14.2.0".into(), false)]);

    // The same version on the host: nothing to build.
    assert_eq!(summary(eval_on(&host_with_gcc("same", "14.2.0"), GCC)), [("14.2.0".into(), true)]);

    // No host compiler.
    assert_eq!(summary(eval_on(&Host::empty(), GCC)), [("14.2.0".into(), false)]);

    // Missing executables are null, which is falsy.
    let result = eval_on(&host_with_gcc("values", "13.1.0"), r#"program p {
        version("1");
        let v = host_version("gcc");
        if host_bin("clang") { bash { unreachable } }
        if host_bin("gcc") { bash { ${v} } }
    }"#);
    assert_eq!(result.versions[0].commands, ["13.1.0"]);
}

#[test]
fn use_host_requires_the_executable() {
    let (message, text) = run_err(r#"use_host("definitely-not-installed");"#);
    assert_eq!(
        message,
        "the host has no `definitely-not-installed` executable; guard this call with `if host_bin(\"definitely-not-installed\")`"
    );
    assert_eq!(text, r#"use_host("definitely-not-installed")"#);
}

#[test]
fn lists() {
    assert_eq!(commands(r#"let xs = ["a b", 2, true]; bash { ${xs} }"#), ["a b 2 true"]);
    assert_eq!(commands(r#"if [] { bash { empty } } if [1] { bash { full } }"#), ["full"]);
    assert_eq!(commands(r#"if [1, [2]] == [1, [2]] { bash { equal } }"#), ["equal"]);
}

#[test]
fn exec_runs_programs_directly() {
    assert_eq!(
        commands(
            r#"let r = exec("printf", ["%s|", "a b", "$HOME"]);
               let out = r.stdout; let ok = r.success; let code = r.status;
               bash { ${out} ${ok} ${code} }"#
        ),
        ["a b|$HOME| true 0"]
    );
    assert_eq!(
        commands(
            r#"let r = exec("sh", ["-c", "echo err >&2; exit 4"]);
               let err = trim(r.stderr); let ok = r.success; let code = r.status;
               bash { ${err} ${ok} ${code} }"#
        ),
        ["err false 4"]
    );
    // Without arguments, and with a path.
    assert_eq!(commands(r#"let r = exec("/bin/true"); if r.success { bash { yes } }"#), ["yes"]);
}

#[test]
fn exec_errors_point_at_the_call() {
    let (message, text) = run_err(r#"let r = exec("definitely-not-a-program", ["x"]);"#);
    assert!(message.starts_with("cannot run `definitely-not-a-program`: "), "{message}");
    assert_eq!(text, r#"exec("definitely-not-a-program", ["x"])"#);
    assert_eq!(
        run_err(r#"exec("echo", ["a", 1]);"#).0,
        "`args` of `exec` must be a list of strings, found a number in it"
    );
    assert_eq!(
        run_err(r#"exec("echo", "a");"#).0,
        "argument `args` of `exec` must be a list, found string"
    );
    assert_eq!(run_err(r#"exec("");"#).0, "`exec` needs a program to run");
}

#[test]
fn return_ends_the_program() {
    let result = eval(
        r#"program p {
            version("1");
            bash { first }
            let i = 0;
            while true {
                i = i + 1;
                if i == 3 { return; }
                bash { loop ${i} }
            }
            bash { never }
            version("2");
        }"#,
    );
    // The current version is kept; nothing after `return` runs.
    assert_eq!(result.versions.len(), 1);
    assert_eq!(result.versions[0].commands, ["first", "loop 1", "loop 2"]);

    // Returning before any version is still a program without versions.
    let (arena, program) = parse("program p { return; version(\"1\"); }");
    let err = eval_program(&program, &mut Executor::new()).err().unwrap();
    assert!(err.message.contains("does not specify a version"), "{}", err.render(&arena));
}

#[test]
fn return_simplifies_host_recipes() {
    let recipe = r#"program gcc {
        let host = host_version("gcc");
        if host {
            version(host);
            use_host("gcc");
            return;
        }
        version("14.2.0");
        bash { make }
    }"#;
    let host_versions = |result: EvalResult| -> Vec<(String, bool)> {
        result.versions.into_iter().map(|v| (v.version, v.host.is_some())).collect()
    };
    assert_eq!(host_versions(eval_on(&host_with_gcc("return", "14.2.0"), recipe)), [("14.2.0".into(), true)]);
    assert_eq!(host_versions(eval_on(&Host::empty(), recipe)), [("14.2.0".into(), false)]);
}

#[test]
fn build_system_helpers_add_commands() {
    let commands = commands(
        r#"cmake(["-DBUILD_TESTING=OFF"], build_type = "Debug", env = ["CFLAGS=-O1"]);
           make(dir = "tools", jobs = 2, install = "");
           cargo(features = ["cli"], locked = true, prefix = "$NUX_PREFIX/opt");
           go(packages = ["./cmd/x"]);
           meson();
           autotools(options = ["--disable-nls"]);"#,
    );
    assert_eq!(
        commands,
        [
            r#"CFLAGS=-O1 cmake -S . -B build "-DCMAKE_INSTALL_PREFIX=$NUX_PREFIX" -DCMAKE_BUILD_TYPE=Debug -DBUILD_TESTING=OFF"#,
            "CFLAGS=-O1 cmake --build build --parallel $NUX_JOBS",
            "CFLAGS=-O1 cmake --install build",
            r#"make -C tools -j 2 "PREFIX=$NUX_PREFIX""#,
            r#"CARGO_TARGET_DIR="$NUX_BUILD/target" cargo install --path . --root "$NUX_PREFIX/opt" --jobs $NUX_JOBS --locked --features cli"#,
            r#"GOBIN="$NUX_PREFIX/bin" GOPATH="$NUX_BUILD/.go" GOCACHE="$NUX_BUILD/.cache/go-build" go install -p $NUX_JOBS ./cmd/x"#,
            r#"meson setup build . "--prefix=$NUX_PREFIX" --buildtype=release"#,
            "meson compile -C build -j $NUX_JOBS",
            "meson install -C build",
            r#"./configure "--prefix=$NUX_PREFIX" --disable-nls"#,
            "make -C . -j $NUX_JOBS",
            "make -C . install",
        ]
    );
}

#[test]
fn build_system_helper_errors() {
    assert_eq!(run_err("make(jobs = 0);").0, "`jobs` of `make` must be a positive whole number");
    assert_eq!(run_err("make(jobs = 1.5);").0, "`jobs` of `make` must be a positive whole number");
    assert_eq!(run_err(r#"cmake(env = ["CFLAGS"]);"#).0, "`env` of `cmake` expects `NAME=value` entries, found `CFLAGS`");
    assert_eq!(run_err(r#"cargo(features = [1]);"#).0, "`features` of `cargo` must be a list of strings, found a number in it");
    assert_eq!(run_err(r#"go(tag = "x");"#), ("`go` has no parameter named `tag`".into(), "tag".into()));
    let (arena, program) = parse("program p { cmake(); version(\"1\"); }");
    let err = eval_program(&program, &mut Executor::new()).err().unwrap();
    assert_eq!(err.message, "`cmake` must come after a `version(...)` call");
    assert_eq!(arena.slice(err.span), Some("cmake()"));
}

#[test]
fn set_bin_can_name_the_link() {
    let result = eval(
        r#"program lua {
            let release = "5.4";
            version(release);
            set_bin("bin/lua", name = "lua${release}");
            version("5.3");
            set_bin("bin/lua");
        }"#,
    );
    assert_eq!(result.versions[0].bin.as_deref(), Some("bin/lua"));
    assert_eq!(result.versions[0].bin_name.as_deref(), Some("lua5.4"));
    assert_eq!(result.versions[1].bin_name, None);

    let err = |name: &str| run_err(&format!(r#"set_bin("bin/x", name = "{name}");"#)).0;
    assert_eq!(err(""), "the link name `` must not be empty");
    assert_eq!(err("a/b"), "the link name `a/b` must be a file name, without `/`");
    assert_eq!(err(".x"), "the link name `.x` must not start with `.`");
    assert_eq!(err("a b"), "the link name `a b` must not contain spaces or control characters");
}
