use std::collections::HashSet;
use std::sync::OnceLock;

use nux_eval::{Dependency, SourceKind, VersionDef};
use nux_executor::Executor;
use nux_lexer::{Lexer, SourceArena};

use super::*;

fn build(name: &str, version: &str) -> Dependency {
    Dependency::Build {
        name: name.into(),
        version: version.into(),
    }
}

fn runtime(name: &str, version: &str) -> Dependency {
    Dependency::Runtime {
        name: name.into(),
        version: version.into(),
    }
}

fn add(
    resolver: &mut Resolver,
    name: &str,
    version: &str,
    dependencies: &[Dependency],
) -> PackageVersionId {
    let def = VersionDef {
        version: version.into(),
        commands: vec![BashCommand {
            is_unsafe: false,
            command: format!("build {name} {version}"),
        }],
        source: None,
        dependencies: dependencies.iter().cloned().collect(),
        bin: None,
        bin_name: None,
        libraries: Vec::new(),
        host: None,
    };
    resolver.add_version(name.into(), def).unwrap()
}

/// The plan as `name@version` strings.
fn plan(resolver: &Resolver, requests: &[(&str, &str)]) -> Result<Vec<String>, ResolveError> {
    let requests: Vec<_> = requests.iter().map(|(n, v)| Request::new(*n, *v)).collect();
    Ok(resolver
        .resolve(&requests)?
        .into_iter()
        .map(|p| format!("{}@{}", p.name, p.version))
        .collect())
}

fn req(text: &str) -> VersionReq {
    VersionReq::parse(text).unwrap()
}

fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

#[test]
fn versions_compare_by_value() {
    assert!(v("1.10.0") > v("1.9.3"));
    assert!(v("2") > v("1.99"));
    assert!(req("=1.2").matches(&v("1.2.0")));
    assert_ne!(
        v("1.2"),
        v("1.2.0"),
        "spelling still distinguishes versions"
    );
    assert!(v("1.0.rc1") < v("1.0.0"));
    assert!(Version::parse("").is_err());
    assert!(Version::parse("1..2").is_err());
    assert!(Version::parse("1 2").is_err());
}

#[test]
fn requirements_match_versions() {
    let cases = [
        ("latest", "0.0.1", true),
        ("*", "99", true),
        ("4.2.0", "4.2.0", true),
        ("4.2", "4.2.0", true),
        ("=4.2.0", "4.2.1", false),
        (">=1.2, <2", "1.9.9", true),
        (">=1.2, <2", "2.0", false),
        (">1.2", "1.2", false),
        ("<=1.2", "1.2.0", true),
        ("^1.2.3", "1.9.0", true),
        ("^1.2.3", "2.0.0", false),
        ("^1.2.3", "1.2.2", false),
        ("^0.2.3", "0.2.9", true),
        ("^0.2.3", "0.3.0", false),
        ("^0.0.3", "0.0.4", false),
        ("~1.2.3", "1.2.9", true),
        ("~1.2.3", "1.3.0", false),
        ("~1", "1.9", true),
        ("~1", "2.0", false),
    ];
    for (requirement, version, expected) in cases {
        assert_eq!(
            req(requirement).matches(&v(version)),
            expected,
            "{requirement} vs {version}"
        );
    }
    assert!(VersionReq::parse(">=").is_err());
    assert!(VersionReq::parse(">=1,").is_err());
    assert!(VersionReq::parse("^beta").is_err());
}

#[test]
fn selects_highest_matching_version() {
    let mut r = Resolver::new();
    let old = add(&mut r, "gcc", "9.1", &[]);
    let mid = add(&mut r, "gcc", "11.0", &[]);
    let new = add(&mut r, "gcc", "12.2", &[]);

    assert_eq!(r.select("gcc", "latest"), Ok(new));
    assert_eq!(r.select("gcc", "9.1"), Ok(old));
    assert_eq!(r.select("gcc", ">=10, <12"), Ok(mid));
    assert_eq!(r.select("gcc", "^11"), Ok(mid));
    assert_eq!(r.select("gcc", "~9"), Ok(old));
}

#[test]
fn dependencies_use_version_specific_edges() {
    let mut r = Resolver::new();
    add(&mut r, "gcc", "9.1", &[]);
    add(&mut r, "gcc", "12.2", &[]);
    add(&mut r, "git", "2.40", &[]);
    add(&mut r, "cmake", "3.0", &[build("gcc", "^9")]);
    add(
        &mut r,
        "cmake",
        "4.2.0",
        &[build("gcc", ">=12"), runtime("git", "latest")],
    );

    assert_eq!(
        plan(&r, &[("cmake", "3.0")]).unwrap(),
        ["gcc@9.1", "cmake@3.0"]
    );
    assert_eq!(
        plan(&r, &[("cmake", "latest")]).unwrap(),
        ["gcc@12.2", "git@2.40", "cmake@4.2.0"]
    );

    let resolved = r.resolve(&[Request::new("cmake", "4.2.0")]).unwrap();
    let cmake = resolved.last().unwrap();
    assert_eq!(
        cmake
            .commands
            .iter()
            .map(|c| c.command.as_str())
            .collect::<Vec<_>>(),
        ["build cmake 4.2.0"]
    );
    assert_eq!(
        cmake.dependencies,
        [
            ResolvedDependency {
                id: resolved[0].id,
                name: "gcc".into(),
                version: "12.2".into(),
                requirement: ">=12".into(),
                kind: DependencyKind::Build,
            },
            ResolvedDependency {
                id: resolved[1].id,
                name: "git".into(),
                version: "2.40".into(),
                requirement: "latest".into(),
                kind: DependencyKind::Runtime,
            },
        ]
    );
}

#[test]
fn transitive_dependencies_exclude_unrelated_versions() {
    let mut r = Resolver::new();
    add(&mut r, "base", "2", &[]);
    add(&mut r, "base", "3", &[]);
    add(&mut r, "lib", "1.4", &[build("base", "=2")]);
    add(&mut r, "lib", "2.0", &[build("base", "3")]);
    add(&mut r, "app", "1", &[build("lib", "^1")]);
    add(&mut r, "unrelated", "1", &[build("base", "3")]);

    assert_eq!(
        plan(&r, &[("app", "1")]).unwrap(),
        ["base@2", "lib@1.4", "app@1"]
    );
}

#[test]
fn different_versions_of_one_package_can_both_be_required() {
    let mut r = Resolver::new();
    add(&mut r, "python", "2.7", &[]);
    add(&mut r, "python", "3.12", &[]);
    add(&mut r, "legacy", "1", &[build("python", "^2")]);
    add(&mut r, "modern", "1", &[build("python", "^3")]);

    assert_eq!(
        plan(&r, &[("modern", "latest"), ("legacy", "latest")]).unwrap(),
        ["python@2.7", "legacy@1", "python@3.12", "modern@1"]
    );
}

#[test]
fn shared_dependencies_appear_once() {
    let mut r = Resolver::new();
    add(&mut r, "d", "1", &[]);
    add(&mut r, "b", "1", &[build("d", "latest")]);
    add(&mut r, "c", "1", &[build("d", "1"), runtime("d", "^1")]);
    add(
        &mut r,
        "a",
        "1",
        &[build("c", "latest"), build("b", "latest")],
    );

    let order = plan(&r, &[("a", "1"), ("b", "1"), ("a", "latest")]).unwrap();
    assert_eq!(order, ["d@1", "b@1", "c@1", "a@1"]);
    let unique: HashSet<_> = order.iter().collect();
    assert_eq!(unique.len(), order.len());
}

#[test]
fn order_does_not_depend_on_registration_or_request_order() {
    let packages: [(&str, &[Dependency]); 4] = [
        ("z", &[]),
        ("y", &[build("z", "1")]),
        ("x", &[build("z", "1")]),
        ("w", &[build("y", "1"), build("x", "1")]),
    ];
    let mut forward = Resolver::new();
    for (name, deps) in packages.iter() {
        add(&mut forward, name, "1", deps);
    }
    let mut backward = Resolver::new();
    for (name, deps) in packages.iter().rev() {
        add(&mut backward, name, "1", deps);
    }

    let expected = ["z@1", "x@1", "y@1", "w@1"];
    assert_eq!(plan(&forward, &[("w", "1"), ("x", "1")]).unwrap(), expected);
    assert_eq!(
        plan(&backward, &[("x", "1"), ("w", "1")]).unwrap(),
        expected
    );
}

#[test]
fn missing_packages_and_versions_are_reported() {
    let mut r = Resolver::new();
    add(&mut r, "gcc", "9.1", &[]);
    add(&mut r, "gcc", "12.2", &[]);
    add(&mut r, "needs-ghost", "1", &[build("ghost", "latest")]);
    add(&mut r, "needs-new-gcc", "1", &[build("gcc", ">=13")]);
    add(&mut r, "contradiction", "1", &[build("gcc", ">=12, <10")]);
    add(&mut r, "malformed", "1", &[build("gcc", ">=")]);

    assert_eq!(
        plan(&r, &[("nope", "latest")]),
        Err(ResolveError::UnknownPackage {
            name: "nope".into()
        })
    );
    assert_eq!(
        plan(&r, &[("gcc", "10")]).unwrap_err().to_string(),
        "no version of `gcc` satisfies `10` (available: 9.1, 12.2)"
    );
    assert_eq!(
        plan(&r, &[("needs-ghost", "1")]).unwrap_err().to_string(),
        "in dependencies of `needs-ghost` 1: package `ghost` is not defined"
    );
    assert_eq!(
        plan(&r, &[("needs-new-gcc", "1")]).unwrap_err(),
        ResolveError::InDependency {
            name: "needs-new-gcc".into(),
            version: "1".into(),
            error: Box::new(ResolveError::NoMatchingVersion {
                name: "gcc".into(),
                requirement: ">=13".into(),
                available: vec!["9.1".into(), "12.2".into()],
            }),
        }
    );
    assert!(matches!(
        plan(&r, &[("contradiction", "1")]),
        Err(ResolveError::InDependency { error, .. })
            if matches!(*error, ResolveError::NoMatchingVersion { .. })
    ));
    assert!(matches!(
        plan(&r, &[("malformed", "1")]),
        Err(ResolveError::InDependency { error, .. })
            if matches!(*error, ResolveError::InvalidRequirement { .. })
    ));

    // Broken definitions only matter when they are required.
    assert_eq!(plan(&r, &[("gcc", "latest")]).unwrap(), ["gcc@12.2"]);
}

#[test]
fn duplicate_and_invalid_versions_are_rejected() {
    let mut r = Resolver::new();
    add(&mut r, "gcc", "9.1", &[]);
    let def = |version: &str| VersionDef {
        version: version.into(),
        commands: Vec::new(),
        source: None,
        dependencies: HashSet::new(),
        bin: None,
        bin_name: None,
        libraries: Vec::new(),
        host: None,
    };
    assert_eq!(
        r.add_version("gcc".into(), def("9.1")),
        Err(ResolveError::DuplicateVersion {
            name: "gcc".into(),
            version: "9.1".into()
        })
    );
    assert!(matches!(
        r.add_version("gcc".into(), def("9 1")),
        Err(ResolveError::InvalidVersion { .. })
    ));
    assert_eq!(r.package("gcc").unwrap().versions.len(), 1);
}

#[test]
fn self_dependency_is_a_cycle() {
    let mut r = Resolver::new();
    add(&mut r, "a", "1", &[build("a", "1")]);
    assert_eq!(
        plan(&r, &[("a", "1")]),
        Err(ResolveError::Cycle(vec![
            ("a".into(), "1".into()),
            ("a".into(), "1".into())
        ]))
    );
}

#[test]
fn cycle_between_packages_is_reported_with_versions() {
    let mut r = Resolver::new();
    add(&mut r, "a", "1", &[build("b", "latest")]);
    add(&mut r, "b", "1", &[build("c", "latest")]);
    add(&mut r, "c", "1", &[runtime("a", "1")]);
    add(&mut r, "top", "1", &[build("a", "1")]);
    assert_eq!(
        plan(&r, &[("top", "1")]).unwrap_err().to_string(),
        "dependency cycle: a 1 -> b 1 -> c 1 -> a 1"
    );
}

#[test]
fn cycle_between_versions_of_one_package() {
    let mut r = Resolver::new();
    add(&mut r, "a", "1", &[build("a", "2")]);
    add(&mut r, "a", "2", &[build("a", "1")]);
    assert_eq!(
        plan(&r, &[("a", "2")]).unwrap_err().to_string(),
        "dependency cycle: a 1 -> a 2 -> a 1"
    );
}

#[test]
fn newer_version_may_depend_on_older_one() {
    // A compiler bootstrapped by its previous release is not a cycle.
    let mut r = Resolver::new();
    add(&mut r, "cc", "1", &[]);
    add(&mut r, "cc", "2", &[build("cc", "1")]);
    add(&mut r, "cc", "3", &[build("cc", "2")]);
    assert_eq!(
        plan(&r, &[("cc", "latest")]).unwrap(),
        ["cc@1", "cc@2", "cc@3"]
    );
}

#[test]
fn cycles_outside_the_request_are_ignored() {
    let mut r = Resolver::new();
    add(&mut r, "a", "1", &[build("b", "1")]);
    add(&mut r, "b", "1", &[build("a", "1")]);
    add(&mut r, "ok", "1", &[]);
    assert_eq!(plan(&r, &[("ok", "1")]).unwrap(), ["ok@1"]);
}

#[test]
fn graph_stages_can_be_used_separately() {
    let mut r = Resolver::new();
    let base = add(&mut r, "base", "1", &[]);
    let unused = add(&mut r, "base", "2", &[]);
    let app = add(&mut r, "app", "1", &[build("base", "1")]);

    let graph = r.build_graph();
    assert_eq!(graph.len(), 3);
    assert_eq!(graph.targets(app).collect::<Vec<_>>(), [base]);

    let required = prune::prune(&graph, &[app]).unwrap();
    assert!(!required.contains(&unused));
    assert_eq!(order::find_cycle(&graph, &required), None);
    assert_eq!(order::build_order(&graph, &required), [base, app]);
}

fn lexer() -> &'static Lexer {
    static LEXER: OnceLock<Lexer> = OnceLock::new();
    LEXER.get_or_init(|| Lexer::new().unwrap())
}

fn eval(src: &str) -> nux_eval::EvalResult {
    let mut arena = SourceArena::new();
    let id = arena.add("test.nux", src);
    let tokens = lexer().lex(arena.get(id).unwrap()).unwrap();
    let program = nux_parser::parse(&tokens).unwrap();
    nux_eval::eval_program(&program, &mut Executor::new()).unwrap()
}

#[test]
fn resolves_evaluated_definitions() {
    let mut r = Resolver::new();
    r.add_package(eval(r#"program gcc { version("9.1"); version("12.2"); }"#))
        .unwrap();
    r.add_package(eval(
        r#"program cmake {
            version("3.0.0");
            source(git = "https://example.com/cmake.git");
            add_dependency(gcc, "^9");
            bash { make old }

            version("4.2.0");
            source(url = "https://example.com/cmake-4.2.0.tar.gz");
            add_dependency(gcc, ">=12");
            bash { make new }
        }"#,
    ))
    .unwrap();
    r.add_package(eval(
        r#"program wine { version("9.0"); add_dependency(cmake, "~3.0"); }"#,
    ))
    .unwrap();

    let resolved = r.resolve(&[Request::new("wine", "latest")]).unwrap();
    let names: Vec<_> = resolved
        .iter()
        .map(|p| format!("{}@{}", p.name, p.version))
        .collect();
    assert_eq!(names, ["gcc@9.1", "cmake@3.0.0", "wine@9.0"]);

    let cmake = &resolved[1];
    assert_eq!(
        cmake
            .commands
            .iter()
            .map(|c| c.command.as_str())
            .collect::<Vec<_>>(),
        ["make old"]
    );
    assert!(matches!(
        cmake.source.as_ref().unwrap().kind,
        SourceKind::Git(_)
    ));
    assert_eq!(cmake.dependencies[0].version, "9.1");
}

#[test]
fn host_provided_versions_are_preferred_when_they_match() {
    let mut r = Resolver::new();
    let host = add(&mut r, "gcc", "12.2.0", &[]);
    let built = add(&mut r, "gcc", "14.2.0", &[]);
    // Mark 12.2.0 as provided by the host.
    let (package, _) = r.get(host);
    let index = r.by_name[&package.name];
    r.packages[index].versions[0].host = Some(nux_eval::HostBinary {
        name: "gcc".into(),
        path: "/usr/bin/gcc".into(),
    });

    assert_eq!(r.select("gcc", "latest"), Ok(host));
    assert_eq!(r.select("gcc", ">=12"), Ok(host));
    // A requirement the host version does not meet still selects a build.
    assert_eq!(r.select("gcc", "^14"), Ok(built));
    assert_eq!(r.select("gcc", "14.2.0"), Ok(built));

    let resolved = r.resolve(&[Request::new("gcc", "latest")]).unwrap();
    assert_eq!(
        resolved[0].host.as_ref().unwrap().path,
        std::path::Path::new("/usr/bin/gcc")
    );
}

#[test]
fn preferred_versions_win_while_they_match() {
    let mut r = Resolver::new();
    add(&mut r, "lib", "1.0", &[]);
    let locked = add(&mut r, "lib", "1.1", &[]);
    let newest = add(&mut r, "lib", "1.2", &[]);
    add(&mut r, "lib", "2.0", &[]);
    add(&mut r, "app", "1", &[build("lib", "^1")]);

    assert!(r.prefer("lib", "1.1"));
    assert!(!r.prefer("lib", "9.9"));
    assert!(!r.prefer("ghost", "1"));
    assert_eq!(r.select("lib", "^1"), Ok(locked));
    assert_eq!(plan(&r, &[("app", "1")]).unwrap(), ["lib@1.1", "app@1"]);
    // A requirement the preferred version does not meet ignores it.
    assert_eq!(r.select("lib", ">=1.2, <2"), Ok(newest));
}
