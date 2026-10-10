//! Loading every package definition from the configured repositories.
//!
//! Files are evaluated on several threads, but their results are taken in
//! order (repositories in priority order, files by name), so which
//! definition wins and which error is reported never depend on timing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use nux_eval::{EvalResult, Host, load_file_with_host};
use nux_executor::Executor;
use nux_lexer::SourceArena;
use nux_resolver::Resolver;

/// How many files are evaluated at the same time unless configured.
pub const DEFAULT_JOBS: usize = 4;

/// Evaluates every `.nux` file in `repositories` into a resolver, `jobs`
/// files at a time. When several repositories define the same program, the
/// first one wins. Errors are rendered and ready to print after an
/// `error: ` prefix; when several files fail, the first one is reported.
pub fn load(repositories: &[PathBuf], host: &Host, jobs: usize) -> Result<Resolver, String> {
    let mut files = Vec::new();
    for (index, repository) in repositories.iter().enumerate() {
        files.extend(definition_files(repository)?.into_iter().map(|file| (index, file)));
    }
    let results = evaluate(&files, host, jobs);

    let mut resolver = Resolver::new();
    // Program name -> (repository index, file).
    let mut defined: HashMap<String, (usize, PathBuf)> = HashMap::new();
    for ((index, file), result) in files.into_iter().zip(results) {
        let result = result.expect("files before a failure are evaluated")?;
        match defined.get(&result.name) {
            Some((first, _)) if *first < index => continue,
            Some((_, other)) => {
                return Err(format!(
                    "`{}` and `{}` both define program `{}`",
                    other.display(),
                    file.display(),
                    result.name
                ));
            }
            None => {}
        }
        defined.insert(result.name.clone(), (index, file.clone()));
        resolver
            .add_package(result)
            .map_err(|e| format!("in `{}`: {e}", file.display()))?;
    }
    Ok(resolver)
}

/// Evaluates `files` on up to `jobs` threads; the results are in the same
/// order. After a failure, later files are skipped (`None`), but every file
/// before it is still evaluated, so the first failure is always the same.
fn evaluate(files: &[(usize, PathBuf)], host: &Host, jobs: usize) -> Vec<Option<Result<EvalResult, String>>> {
    let results: Vec<Mutex<Option<Result<EvalResult, String>>>> = files.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let first_failure = AtomicUsize::new(usize::MAX);
    let worker = || {
        let mut arena = SourceArena::new();
        let mut executor = Executor::new();
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            if index >= files.len() || index > first_failure.load(Ordering::Relaxed) {
                break;
            }
            let file = &files[index].1;
            let result = load_file_with_host(file, &mut arena, &mut executor, host).map_err(|e| {
                let rendered = e.render(&arena);
                rendered.strip_prefix("error: ").unwrap_or(&rendered).trim_end().to_string()
            });
            if result.is_err() {
                first_failure.fetch_min(index, Ordering::Relaxed);
            }
            *results[index].lock().unwrap() = Some(result);
        }
    };
    let jobs = jobs.clamp(1, files.len().max(1));
    if jobs == 1 {
        worker();
    } else {
        std::thread::scope(|scope| {
            for _ in 0..jobs {
                scope.spawn(worker);
            }
        });
    }
    results.into_iter().map(|r| r.into_inner().unwrap()).collect()
}

/// The `.nux` files directly inside `repository`, sorted by name.
fn definition_files(repository: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = std::fs::read_dir(repository).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("repository `{}` does not exist", repository.display()),
        _ => format!("cannot read repository `{}`: {e}", repository.display()),
    })?;
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| format!("cannot read repository `{}`: {e}", repository.display()))?.path();
        if path.extension().is_some_and(|ext| ext == "nux") && path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use nux_resolver::Request;

    use super::*;

    fn repo(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nux-repo-tests-{}", std::process::id())).join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (file, contents) in files {
            std::fs::write(dir.join(file), contents).unwrap();
        }
        dir
    }

    fn plan(resolver: &Resolver, requests: &[(&str, &str)]) -> Vec<String> {
        let requests: Vec<_> = requests.iter().map(|(n, v)| Request::new(*n, *v)).collect();
        resolver
            .resolve(&requests)
            .unwrap()
            .iter()
            .map(|p| format!("{}@{}", p.name, p.version))
            .collect()
    }

    #[test]
    fn loads_every_definition_and_prunes_to_the_request() {
        let dir = repo(
            "all",
            &[
                ("gcc.nux", r#"program gcc { version("12"); version("13"); }"#),
                ("cmake.nux", r#"program cmake { version("4.2.0"); add_dependency(gcc, "12"); }"#),
                ("unrelated.nux", r#"program unrelated { version("1"); }"#),
                ("notes.txt", "not a definition"),
            ],
        );
        let resolver = load(&[dir], &Host::empty(), DEFAULT_JOBS).unwrap();
        assert_eq!(resolver.packages().len(), 3);
        assert_eq!(plan(&resolver, &[("cmake", "4.2.0")]), ["gcc@12", "cmake@4.2.0"]);
    }

    #[test]
    fn the_first_repository_wins() {
        let local = repo("first", &[("tool.nux", r#"program tool { version("2"); }"#)]);
        let shared = repo("second", &[("tool.nux", r#"program tool { version("1"); }"#)]);
        let resolver = load(&[local, shared], &Host::empty(), DEFAULT_JOBS).unwrap();
        let tool = resolver.package("tool").unwrap();
        assert_eq!(tool.versions.iter().map(|v| v.version.as_str()).collect::<Vec<_>>(), ["2"]);
    }

    /// Names and versions of every package, in the resolver's order.
    fn summary(resolver: &Resolver) -> Vec<String> {
        resolver
            .packages()
            .iter()
            .map(|p| format!("{}:{}", p.name, p.versions.iter().map(|v| v.version.as_str()).collect::<Vec<_>>().join(",")))
            .collect()
    }

    #[test]
    fn parallel_evaluation_gives_the_same_result() {
        let mut files: Vec<(String, String)> = (0..30)
            .map(|i| (format!("p{i:02}.nux"), format!(r#"program p{i} {{ version("{i}"); }}"#)))
            .collect();
        files.push(("tool.nux".into(), r#"program tool { version("local"); }"#.into()));
        let files: Vec<_> = files.iter().map(|(n, c)| (n.as_str(), c.as_str())).collect();
        let first = repo("parallel-first", &files);
        let shared = repo("parallel-second", &[("tool.nux", r#"program tool { version("shared"); }"#)]);
        let repositories = [first, shared];
        let sequential = summary(&load(&repositories, &Host::empty(), 1).unwrap());
        for jobs in [2, 4, 64] {
            assert_eq!(summary(&load(&repositories, &Host::empty(), jobs).unwrap()), sequential, "{jobs} jobs");
        }
        assert!(sequential.contains(&"tool:local".to_string()));
    }

    #[test]
    fn the_first_failing_file_is_reported() {
        let mut files: Vec<(String, String)> =
            (0..20).map(|i| (format!("f{i:02}.nux"), format!(r#"program f{i} {{ version("1"); }}"#))).collect();
        // A slow failure first, and quick ones after it.
        files[3].1 = r#"program f3 { let r = exec("sleep", ["0.3"]); version(1); }"#.into();
        files[12].1 = "program f12 { version(2); }".into();
        files[17].1 = "program f17 {".into();
        let files: Vec<_> = files.iter().map(|(n, c)| (n.as_str(), c.as_str())).collect();
        let dir = repo("first-failure", &files);
        for jobs in [1, 4] {
            let err = load(std::slice::from_ref(&dir), &Host::empty(), jobs).err().unwrap();
            assert!(err.contains("f03.nux"), "{jobs} jobs: {err}");
        }
    }

    #[test]
    fn files_are_evaluated_at_the_same_time() {
        let slow = r#"program NAME { let r = exec("sleep", ["0.4"]); version("1"); }"#;
        let files: Vec<_> = (0..4).map(|i| (format!("s{i}.nux"), slow.replace("NAME", &format!("s{i}")))).collect();
        let files: Vec<_> = files.iter().map(|(n, c)| (n.as_str(), c.as_str())).collect();
        let dir = repo("concurrent", &files);
        // The lexer is built on first use; not part of what is measured.
        load(&[repo("warm-up", &[("w.nux", r#"program w { version("1"); }"#)])], &Host::empty(), 1).unwrap();
        let started = std::time::Instant::now();
        assert_eq!(load(std::slice::from_ref(&dir), &Host::empty(), 4).unwrap().packages().len(), 4);
        // One after the other would take 1.6s.
        assert!(started.elapsed() < std::time::Duration::from_millis(1200), "{:?}", started.elapsed());
    }

    #[test]
    fn useful_errors() {
        let missing = std::env::temp_dir().join("nux-repo-tests-does-not-exist");
        assert_eq!(load(std::slice::from_ref(&missing), &Host::empty(), DEFAULT_JOBS).err().unwrap(), format!("repository `{}` does not exist", missing.display()));

        let dir = repo("broken", &[("bad.nux", "program bad { version(1); }")]);
        let err = load(std::slice::from_ref(&dir), &Host::empty(), DEFAULT_JOBS).err().unwrap();
        assert!(err.starts_with("argument `version` of `version` must be a string, found number\n"), "{err}");
        assert!(err.contains(&format!("--> {}:1:23", dir.join("bad.nux").display())), "{err}");

        let dir = repo("twice", &[("a.nux", r#"program x { version("1"); }"#), ("b.nux", r#"program x { version("2"); }"#)]);
        let err = load(&[dir], &Host::empty(), DEFAULT_JOBS).err().unwrap();
        assert!(err.ends_with("b.nux` both define program `x`"), "{err}");
    }

    #[test]
    fn the_minimal_example_resolves() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example/minimal");
        let env = crate::environment::Environment::load(&root.join("Nux.toml")).unwrap();
        let resolver = load(&env.repositories, &Host::empty(), DEFAULT_JOBS).unwrap();
        let requests: Vec<_> = env.packages.iter().map(|r| (r.name.as_str(), r.version.as_str())).collect();
        // `^1` keeps greeting 2.0.0 out of the build.
        assert_eq!(plan(&resolver, &requests), ["greeting@1.0.0", "octocat@1.0.0", "hello@0.1.0"]);
        assert!(root.join("src/hello/hello.sh").is_file());
    }

    #[test]
    fn the_example_repository_loads() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../example/repository");
        let resolver = load(&[dir], &Host::empty(), DEFAULT_JOBS).unwrap();
        let cmake = &resolver.package("cmake").unwrap().versions[0];
        assert_eq!(cmake.version, "4.2.0");
        assert_eq!(cmake.bin.as_deref(), Some("bin/cmake"));
        assert_eq!(cmake.source.as_ref().unwrap().tag.as_deref(), Some("v4.2.0"));
    }
}
