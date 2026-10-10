use std::collections::HashSet;
use std::process::Command;

use nux_eval::{Dependency, HostBinary, Library, Source, SourceKind, VersionDef};
use nux_resolver::{Request, Resolver};

use super::*;

/// A fresh directory for one test.
fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nux-build-tests-{}", std::process::id())).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Def {
    name: &'static str,
    version: &'static str,
    commands: Vec<String>,
    source: Option<Source>,
    deps: Vec<(&'static str, &'static str)>,
    bin: Option<&'static str>,
    bin_name: Option<&'static str>,
    libraries: Vec<Library>,
    host: Option<HostBinary>,
}

fn def(name: &'static str, version: &'static str) -> Def {
    Def { name, version, commands: Vec::new(), source: None, deps: Vec::new(), bin: None, bin_name: None, libraries: Vec::new(), host: None }
}

impl Def {
    fn cmd(mut self, command: &str) -> Self {
        self.commands.push(command.to_string());
        self
    }
    fn dep(mut self, name: &'static str, version: &'static str) -> Self {
        self.deps.push((name, version));
        self
    }
    /// Registers `bin` without generating it (the commands must create it).
    fn bin_only(mut self, bin: &'static str) -> Self {
        self.bin = Some(bin);
        self
    }
    fn bin(mut self, bin: &'static str) -> Self {
        // A `set_bin` target that the build actually produces.
        let dir = Path::new(bin).parent().unwrap().display().to_string();
        let (name, version) = (self.name, self.version);
        self = self.cmd(&format!(
            "mkdir -p $NUX_PREFIX/{dir} && printf '#!/bin/sh\\necho {name} {version}\\n' > $NUX_PREFIX/{bin} && chmod +x $NUX_PREFIX/{bin}"
        ));
        self.bin = Some(bin);
        self
    }
    /// Links the executable as `name` (`set_bin(..., name = ...)`).
    fn named(mut self, name: &'static str) -> Self {
        self.bin_name = Some(name);
        self
    }

    fn lib(mut self, name: &str) -> Self {
        self.libraries.push(Library { name: name.into(), lib: "lib".into(), include: Some("include".into()) });
        self
    }
    fn host(mut self, path: &Path) -> Self {
        self.host = Some(HostBinary { name: self.name.into(), path: path.into() });
        self
    }
    fn source(mut self, kind: SourceKind, tag: Option<&str>) -> Self {
        self.source = Some(Source { kind, tag: tag.map(Into::into), proxy: None });
        self
    }
}

/// Resolves `requests` against `defs`; returns the build order and the
/// requested ids.
fn resolve(defs: Vec<Def>, requests: &[(&str, &str)]) -> (Vec<ResolvedPackage>, Vec<PackageVersionId>) {
    let mut resolver = Resolver::new();
    for d in defs {
        let dependencies: HashSet<_> = d
            .deps
            .iter()
            .map(|(n, v)| Dependency::Build { name: n.to_string(), version: v.to_string() })
            .collect();
        let version = VersionDef {
            version: d.version.into(),
            commands: d.commands,
            source: d.source,
            dependencies,
            bin: d.bin.map(Into::into),
            bin_name: d.bin_name.map(Into::into),
            libraries: d.libraries,
            host: d.host,
        };
        resolver.add_version(d.name.into(), version).unwrap();
    }
    let requests: Vec<_> = requests.iter().map(|(n, v)| Request::new(*n, *v)).collect();
    let ids = requests.iter().map(|r| resolver.select(&r.name, &r.version).unwrap()).collect();
    (resolver.resolve(&requests).unwrap(), ids)
}

/// Records reporter callbacks as `event name@version` strings.
#[derive(Default)]
struct Recorder(Mutex<Vec<String>>);

impl Recorder {
    fn push(&self, event: &str, step: &Step) {
        self.0.lock().unwrap().push(format!("{event} {}@{}", step.package.name, step.package.version));
    }
    fn events(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl Reporter for Recorder {
    fn download_skipped(&self, step: &Step) {
        self.push("already-downloaded", step);
    }
    fn download_queued(&self, step: &Step) {
        self.push("download", step);
    }
    fn download_finished(&self, step: &Step, result: Result<(), &BuildError>) {
        self.push(if result.is_ok() { "downloaded" } else { "download-failed" }, step);
    }
    fn build_skipped(&self, step: &Step) {
        self.push("already-built", step);
    }
    fn build_started(&self, step: &Step) {
        self.push("build", step);
    }
    fn build_finished(&self, step: &Step, result: Result<(), &BuildError>) {
        self.push(if result.is_ok() { "built" } else { "build-failed" }, step);
    }
}

/// Runs every stage like `nux sync` does.
fn sync(builder: &mut Builder, defs: Vec<Def>, requests: &[(&str, &str)]) -> (BuildPlan, Vec<String>, Result<(), BuildError>) {
    let (packages, ids) = resolve(defs, requests);
    let plan = builder.plan(packages, &ids);
    let recorder = Recorder::default();
    let result = (|| {
        builder.download(&plan, &recorder)?;
        builder.build(&plan, &recorder)?;
        builder.link_executables(&plan)?;
        builder.record_environment("test", Path::new("Nux.toml"), &plan)
    })();
    (plan, recorder.events(), result)
}

fn names(steps: impl Iterator<Item = impl std::ops::Deref<Target = Step>>) -> Vec<String> {
    steps.map(|s| format!("{}@{}", s.package.name, s.package.version)).collect()
}

fn run_bin(builder: &Builder, name: &str) -> String {
    let output = Command::new(builder.bin_path().join(name)).output().unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[test]
fn fresh_sync_builds_links_and_then_is_up_to_date() {
    let home = temp("fresh");
    let mut builder = Builder::new(home.join("nux"));
    let defs = || {
        vec![
            def("lib", "1.0").cmd("echo lib > $NUX_PREFIX/lib.txt"),
            def("app", "2.0").dep("lib", "1.0").bin("bin/app"),
            def("unrelated", "1.0"),
        ]
    };

    let (plan, events, result) = sync(&mut builder, defs(), &[("app", "latest")]);
    result.unwrap();
    assert_eq!(names(plan.to_build()), ["lib@1.0", "app@2.0"]);
    assert_eq!(events, ["build lib@1.0", "built lib@1.0", "build app@2.0", "built app@2.0"]);
    assert_eq!(run_bin(&builder, "app"), "app 2.0");
    assert_eq!(
        std::fs::read_link(builder.bin_path().join("app")).unwrap(),
        Path::new("../packages/app/2.0/install/bin/app")
    );
    // Sources, scratch files and outputs are kept apart; scratch is removed.
    let paths = builder.workspace().package("lib", "1.0");
    assert!(paths.install.join("lib.txt").is_file());
    assert!(!paths.build.exists());
    assert!(paths.metadata.is_file());
    assert!(home.join("nux/state/environments/test.json").is_file());

    let (packages, ids) = resolve(defs(), &[("app", "latest")]);
    let plan = builder.plan(packages, &ids);
    assert!(plan.is_up_to_date());
    let (_, events, result) = sync(&mut builder, defs(), &[("app", "latest")]);
    result.unwrap();
    assert_eq!(events, ["already-built lib@1.0", "already-built app@2.0"]);
}

#[test]
fn existing_installations_are_reused_incrementally() {
    let home = temp("incremental");
    let mut builder = Builder::new(&home);
    sync(&mut builder, vec![def("base", "1").cmd("true")], &[("base", "1")]).2.unwrap();

    let defs = vec![def("base", "1").cmd("true"), def("tool", "1").dep("base", "1").cmd("true")];
    let (plan, events, result) = sync(&mut builder, defs, &[("tool", "1")]);
    result.unwrap();
    assert_eq!(names(plan.to_build()), ["tool@1"]);
    assert_eq!(events, ["already-built base@1", "build tool@1", "built tool@1"]);
}

#[test]
fn changed_definitions_invalidate_dependents() {
    let home = temp("invalidate");
    let mut builder = Builder::new(&home);
    let defs = |base_cmd: &str| vec![def("base", "1").cmd(base_cmd), def("top", "1").dep("base", "1").cmd("true")];
    sync(&mut builder, defs("true"), &[("top", "1")]).2.unwrap();

    // Same version, different commands: both base and its dependent rebuild.
    let (packages, ids) = resolve(defs("echo changed"), &[("top", "1")]);
    let plan = builder.plan(packages, &ids);
    assert_eq!(names(plan.to_build()), ["base@1", "top@1"]);

    // A corrupted record is treated as missing, not as installed.
    let (packages, ids) = resolve(defs("true"), &[("top", "1")]);
    assert!(builder.plan(packages.clone(), &ids).is_up_to_date());
    std::fs::write(builder.workspace().package("top", "1").metadata, "garbage").unwrap();
    assert_eq!(names(builder.plan(packages, &ids).to_build()), ["top@1"]);
}

#[test]
fn failed_build_is_not_marked_installed() {
    let home = temp("failure");
    let mut builder = Builder::new(&home);
    let defs = || {
        vec![
            def("ok", "1").cmd("true"),
            def("broken", "1").dep("ok", "1").cmd("echo starting; echo oops >&2; exit 3").cmd("echo never"),
            def("top", "1").dep("broken", "1"),
        ]
    };
    let (_, events, result) = sync(&mut builder, defs(), &[("top", "1")]);
    let err = result.unwrap_err();
    let BuildError::Command(failure) = &err else { panic!("{err}") };
    let CommandFailure { command, status, tail, .. } = &**failure;
    assert_eq!(command, "echo starting; echo oops >&2; exit 3");
    assert_eq!(status, "exited with exit status: 3");
    assert_eq!(tail, "$ echo starting; echo oops >&2; exit 3\nstarting\noops");
    assert_eq!(events, ["build ok@1", "built ok@1", "build broken@1", "build-failed broken@1"]);

    // The successful dependency is kept; the failure is retried next time.
    let (packages, ids) = resolve(defs(), &[("top", "1")]);
    let plan = builder.plan(packages, &ids);
    assert_eq!(names(plan.to_build()), ["broken@1", "top@1"]);
    assert!(!builder.workspace().package("broken", "1").install.exists());
    assert!(!home.join("state/environments/test.json").exists());
}

#[test]
fn missing_set_bin_target_fails_the_build() {
    let home = temp("missing-bin");
    let mut builder = Builder::new(&home);
    let mut package = def("nobin", "1").cmd("true");
    package.bin = Some("bin/nobin");
    let (_, _, result) = sync(&mut builder, vec![package], &[("nobin", "1")]);
    let err = result.unwrap_err().to_string();
    assert!(err.starts_with("nobin 1 declares `set_bin`, but `"), "{err}");
    assert!(err.ends_with("install/bin/nobin` does not exist"), "{err}");
    assert!(!builder.bin_path().join("nobin").exists());

    let mut package = def("noexec", "1").cmd("mkdir $NUX_PREFIX/bin && touch $NUX_PREFIX/bin/noexec");
    package.bin = Some("bin/noexec");
    let err = sync(&mut builder, vec![package], &[("noexec", "1")]).2.unwrap_err().to_string();
    assert!(err.ends_with("is not executable"), "{err}");
}

#[test]
fn links_follow_version_selection_and_refuse_conflicts() {
    let home = temp("links");
    let mut builder = Builder::new(&home);
    // `py` 2 is only a dependency; the requested `py` 3 owns the command.
    let defs = || {
        vec![
            def("py", "2").bin("bin/py"),
            def("py", "3").bin("bin/py"),
            def("legacy", "1").dep("py", "2"),
        ]
    };
    sync(&mut builder, defs(), &[("legacy", "1"), ("py", "3")]).2.unwrap();
    assert_eq!(run_bin(&builder, "py"), "py 3");

    // Requesting only the old version moves the link (no rebuild needed).
    let (packages, ids) = resolve(defs(), &[("legacy", "1")]);
    let plan = builder.plan(packages, &ids);
    assert_eq!(plan.to_build().count(), 0);
    assert_eq!(plan.links[0].status, LinkStatus::Outdated);
    assert!(!plan.is_up_to_date());
    assert_eq!(builder.link_executables(&plan).unwrap(), 1);
    assert_eq!(run_bin(&builder, "py"), "py 2");

    // Files nux did not create are never replaced.
    std::fs::remove_file(builder.bin_path().join("py")).unwrap();
    std::fs::write(builder.bin_path().join("py"), "mine").unwrap();
    let (_, _, result) = sync(&mut builder, defs(), &[("py", "3")]);
    let err = result.unwrap_err().to_string();
    assert!(err.contains("refusing to replace") && err.ends_with("it is not a link created by nux"), "{err}");
    assert_eq!(std::fs::read_to_string(builder.bin_path().join("py")).unwrap(), "mine");

    std::fs::remove_file(builder.bin_path().join("py")).unwrap();
    std::os::unix::fs::symlink("/usr/bin/env", builder.bin_path().join("py")).unwrap();
    let (packages, ids) = resolve(defs(), &[("py", "3")]);
    assert!(matches!(builder.plan(packages, &ids).links[0].status, LinkStatus::Conflict(_)));
}

#[test]
fn rival_packages_providing_one_command_warn() {
    let home = temp("rivals");
    let builder = Builder::new(&home);
    let (packages, ids) = resolve(
        vec![def("vim", "1").bin("bin/vi"), def("nvi", "1").bin("bin/vi")],
        &[("vim", "1"), ("nvi", "1")],
    );
    let plan = builder.plan(packages, &ids);
    assert_eq!(plan.links.len(), 1);
    assert_eq!(plan.links[0].package, "nvi");
    assert_eq!(plan.warnings, ["`nvi` and `vim` both provide `vi`; linking `nvi`"]);
}

#[test]
fn git_sources_are_downloaded_once_and_built_from_a_copy() {
    let home = temp("git");
    let repo = home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("hello.txt"), "v1").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    git(&repo, &["tag", "v1"]);
    std::fs::write(repo.join("hello.txt"), "v2").unwrap();
    git(&repo, &["commit", "-q", "-am", "two"]);

    let url = format!("file://{}", repo.display());
    let defs = |command: &str| {
        vec![def("hello", "1")
            .source(SourceKind::Git(url.clone()), Some("v1"))
            .cmd(command)
            .cmd("cp hello.txt $NUX_PREFIX/ && test ! -e .git")]
    };
    let mut builder = Builder::new(home.join("nux"));
    let (plan, events, result) = sync(&mut builder, defs("echo dirty > hello.txt"), &[("hello", "1")]);
    result.unwrap();
    assert_eq!(plan.downloads(), 1);
    assert_eq!(events[..2], ["download hello@1", "downloaded hello@1"]);
    let paths = builder.workspace().package("hello", "1");
    assert_eq!(std::fs::read_to_string(paths.install.join("hello.txt")).unwrap(), "dirty\n");
    // The tag was checked out and the cached clone was left untouched.
    assert_eq!(std::fs::read_to_string(paths.source.join("hello.txt")).unwrap(), "v1");

    // A rebuild reuses the cached clone.
    let (plan, events, result) = sync(&mut builder, defs("true"), &[("hello", "1")]);
    result.unwrap();
    assert_eq!(plan.downloads(), 0);
    assert_eq!(events[0], "already-downloaded hello@1");
    assert_eq!(std::fs::read_to_string(paths.install.join("hello.txt")).unwrap(), "v1");
}

#[test]
fn url_downloads_and_failures() {
    let home = temp("url");
    let file = home.join("tool.sh");
    std::fs::write(&file, "#!/bin/sh\necho tool\n").unwrap();
    let mut builder = Builder::new(home.join("nux"));

    let ok = def("tool", "1")
        .source(SourceKind::Url(format!("file://{}", file.display())), None)
        .cmd("mkdir -p $NUX_PREFIX/bin && cp $NUX_SOURCE_PATH $NUX_PREFIX/bin/tool && chmod +x $NUX_PREFIX/bin/tool");
    let mut ok = ok;
    ok.bin = Some("bin/tool");
    let (_, _, result) = sync(&mut builder, vec![ok], &[("tool", "1")]);
    result.unwrap();
    assert_eq!(run_bin(&builder, "tool"), "tool");
    assert!(builder.workspace().package("tool", "1").source.join("tool.sh").is_file());

    // Every failing download is reported; nothing half-downloaded is kept.
    let missing = |name: &'static str| {
        def(name, "1").source(SourceKind::Url(format!("file://{}/{name}.tar.gz", home.display())), None)
    };
    let (_, events, result) = sync(&mut builder, vec![missing("a"), missing("b")], &[("a", "1"), ("b", "1")]);
    let BuildError::Downloads(errors) = result.unwrap_err() else { panic!() };
    assert_eq!(errors.len(), 2);
    assert!(errors[0].to_string().starts_with("failed to download a 1: `curl` exited with"));
    assert!(events.contains(&"download-failed b@1".to_string()));
    assert!(!events.iter().any(|e| e.starts_with("build")));
    let paths = builder.workspace().package("a", "1");
    assert!(!paths.source.exists() && !paths.root.join("source.tmp").exists());
}

#[test]
fn local_sources_are_relative_to_the_local_root() {
    let home = temp("local");
    std::fs::create_dir_all(home.join("project/src")).unwrap();
    std::fs::write(home.join("project/src/data"), "local").unwrap();
    let mut builder = Builder::new(home.join("nux")).with_local_root(home.join("project"));
    let package = def("loc", "1").source(SourceKind::Local("src".into()), None).cmd("cp data $NUX_PREFIX/");
    let (plan, _, result) = sync(&mut builder, vec![package], &[("loc", "1")]);
    result.unwrap();
    assert_eq!(plan.downloads(), 0);
    assert_eq!(plan.steps[0].source, SourceStatus::Local(home.join("project/src")));
    assert!(builder.workspace().package("loc", "1").install.join("data").is_file());

    let package = def("gone", "1").source(SourceKind::Local("nope".into()), None);
    let err = sync(&mut builder, vec![package], &[("gone", "1")]).2.unwrap_err().to_string();
    assert!(err.starts_with("gone 1: local source `") && err.ends_with("nope` does not exist"), "{err}");
}

#[test]
fn dependency_executables_are_on_path_during_builds() {
    let home = temp("dep-path");
    let mut builder = Builder::new(&home);
    let defs = vec![
        def("tool", "1").bin("bin/tool"),
        def("user", "1").dep("tool", "1").cmd("tool > $NUX_PREFIX/out.txt"),
    ];
    sync(&mut builder, defs, &[("user", "1")]).2.unwrap();
    let out = builder.workspace().package("user", "1").install.join("out.txt");
    assert_eq!(std::fs::read_to_string(out).unwrap(), "tool 1\n");
}

fn kinds(items: &[CleanItem]) -> Vec<String> {
    items
        .iter()
        .map(|i| match &i.package {
            Some((n, v)) => format!("{} {n}@{v}", i.kind),
            None => format!("{} {}", i.kind, i.path.file_name().unwrap().to_string_lossy()),
        })
        .collect()
}

#[test]
fn clean_removes_leftovers_but_keeps_installations() {
    let home = temp("clean");
    let archive = home.join("data.txt");
    std::fs::write(&archive, "data").unwrap();
    let mut builder = Builder::new(home.join("nux"));
    let url = SourceKind::Url(format!("file://{}", archive.display()));
    let defs = || vec![def("good", "1").source(url.clone(), None).bin("bin/good"), def("worse", "1").source(url.clone(), None).cmd("false")];
    sync(&mut builder, defs(), &[("good", "1"), ("worse", "1")]).2.unwrap_err();
    let workspace = builder.workspace().clone();
    std::fs::create_dir_all(workspace.package("good", "1").root.join("source.tmp")).unwrap();
    std::fs::write(workspace.bin_dir().join(".x.tmp"), "").unwrap();

    let items = workspace.clean_plan(CleanOptions::default()).unwrap();
    assert_eq!(
        kinds(&items),
        [
            "incomplete download good@1",
            "build log good@1",
            "build directory worse@1",
            "build log worse@1",
            "temporary file .x.tmp",
        ]
    );
    assert!(items.iter().all(|i| i.kind != CleanKind::DownloadedSource));
    workspace.clean(&items).unwrap();
    assert!(workspace.clean_plan(CleanOptions::default()).unwrap().is_empty());
    assert!(plan::executable(&workspace.package("good", "1").install.join("bin/good")).is_ok());

    // With sources: the failed version goes entirely, the installed one
    // only loses its download and stays usable.
    let items = workspace.clean_plan(CleanOptions { sources: true, cache: false }).unwrap();
    assert_eq!(kinds(&items), ["downloaded source good@1", "uninstalled package worse@1"]);
    let freed = workspace.clean(&items).unwrap();
    assert!(freed >= 8, "{freed}");
    assert!(!workspace.packages_dir().join("worse").exists());
    assert!(plan::executable(&workspace.package("good", "1").install.join("bin/good")).is_ok());

    // The cleared source is downloaded again only if a rebuild needs it.
    let (packages, ids) = resolve(defs(), &[("good", "1")]);
    let plan = builder.plan(packages, &ids);
    assert_eq!(plan.to_build().count(), 0);
    assert_eq!(plan.steps[0].source, SourceStatus::Missing);
}

#[test]
fn clean_safeguards() {
    let home = temp("clean-guards");
    let mut builder = Builder::new(&home);
    sync(&mut builder, vec![def("app", "1").bin("bin/app")], &[("app", "1")]).2.unwrap();
    let workspace = builder.workspace().clone();
    let paths = workspace.package("app", "1");

    let item = |kind, path: PathBuf, package: bool| CleanItem {
        kind,
        path,
        package: package.then(|| ("app".to_string(), "1".to_string())),
        bytes: 0,
    };
    for bad in [
        item(CleanKind::BuildDirectory, paths.install.clone(), true),
        item(CleanKind::BuildLog, paths.metadata.clone(), true),
        item(CleanKind::UninstalledPackage, paths.root.clone(), true),
        item(CleanKind::TemporaryFile, home.join("state"), false),
        item(CleanKind::BuildDirectory, PathBuf::from("/etc/passwd"), true),
    ] {
        let err = workspace.clean(std::slice::from_ref(&bad)).unwrap_err();
        assert!(matches!(err, BuildError::Clean { .. }), "{err}");
    }
    assert_eq!(run_bin(&builder, "app"), "app 1");

    // A linked version without a record is still in use: never removed.
    std::fs::write(&paths.metadata, "{}").unwrap();
    let items = workspace.clean_plan(CleanOptions { sources: true, cache: false }).unwrap();
    assert!(items.iter().all(|i| i.kind != CleanKind::UninstalledPackage), "{items:?}");
    let err = workspace.clean(&[item(CleanKind::UninstalledPackage, paths.root.clone(), true)]).unwrap_err();
    assert!(err.to_string().ends_with("an executable link points into it"), "{err}");

    // A workspace that does not exist has nothing to clean.
    assert!(Workspace::new(home.join("missing")).clean_plan(CleanOptions { sources: true, cache: false }).unwrap().is_empty());
}

#[test]
fn exported_libraries_are_found_by_dependents() {
    let home = temp("libraries");
    let mut builder = Builder::new(&home);
    let defs = vec![
        // A tiny zlib-like library: a header and a shared object.
        def("greet", "1.0")
            .cmd("mkdir -p $NUX_PREFIX/include $NUX_PREFIX/lib")
            .cmd(r#"printf 'const char *greet(void);\n' > $NUX_PREFIX/include/greet.h"#)
            .cmd(r#"printf 'const char *greet(void) { return "hello from libgreet"; }\n' > greet.c"#)
            .cmd("cc -shared -fPIC -o $NUX_PREFIX/lib/libgreet.so greet.c")
            .lib("greet"),
        // Its dependent passes no -I or -L of its own.
        def("app", "1.0")
            .dep("greet", "1.0")
            .cmd(r#"printf '#include <stdio.h>\n#include <greet.h>\nint main(void) { puts(greet()); return 0; }\n' > main.c"#)
            .cmd("mkdir -p $NUX_PREFIX/bin && cc main.c $LDFLAGS -lgreet -o $NUX_PREFIX/bin/app")
            .cmd("test \"$NUX_DEP_GREET\" = \"$(dirname $(dirname $(dirname $NUX_PREFIX)))/greet/1.0/install\"")
            .bin_only("bin/app"),
    ];
    let (_, _, result) = sync(&mut builder, defs, &[("app", "1.0")]);
    result.unwrap();
    // The rpath lets the installed program find the library on its own.
    assert_eq!(run_bin(&builder, "app"), "hello from libgreet");
}

#[test]
fn missing_exported_library_fails_the_build() {
    let home = temp("missing-library");
    let mut builder = Builder::new(&home);
    let defs = vec![def("zlib", "1.3").cmd("mkdir -p $NUX_PREFIX/include $NUX_PREFIX/lib").lib("z")];
    let err = sync(&mut builder, defs, &[("zlib", "1.3")]).2.unwrap_err().to_string();
    assert_eq!(err, "zlib 1.3 exports library `z`, but neither `libz.so` nor `libz.a` is in `lib`");
    assert!(!builder.workspace().package("zlib", "1.3").install.exists());

    // Versioned shared objects and static archives both count.
    for file in ["libz.so.1.3", "libz.a"] {
        let defs = vec![def("zlib", "1.3").cmd(&format!("mkdir -p $NUX_PREFIX/include $NUX_PREFIX/lib && touch $NUX_PREFIX/lib/{file}")).lib("z")];
        sync(&mut builder, defs, &[("zlib", "1.3")]).2.unwrap();
    }
    let defs = vec![def("zlib", "1.3").cmd("mkdir -p $NUX_PREFIX/lib && touch $NUX_PREFIX/lib/libz.a").lib("z")];
    let err = sync(&mut builder, defs, &[("zlib", "1.3")]).2.unwrap_err().to_string();
    assert!(err.ends_with("but its header directory `include` does not exist"), "{err}");
}

#[test]
fn host_provided_versions_are_never_built() {
    let home = temp("host");
    let host_gcc = home.join("usr/bin/gcc");
    std::fs::create_dir_all(host_gcc.parent().unwrap()).unwrap();
    std::fs::write(&host_gcc, "#!/bin/sh\necho host gcc\n").unwrap();
    std::fs::set_permissions(&host_gcc, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let mut builder = Builder::new(home.join("nux"));
    let defs = || {
        vec![
            def("gcc", "14.2.0")
                .host(&host_gcc)
                .source(SourceKind::Url("file:///never/downloaded".into()), None)
                .cmd("false")
                .bin("bin/gcc"),
            def("app", "1").dep("gcc", "latest").cmd("test -z \"$NUX_DEP_GCC\""),
        ]
    };
    let (plan, events, result) = sync(&mut builder, defs(), &[("app", "1")]);
    result.unwrap();
    assert_eq!(names(plan.to_build()), ["app@1"]);
    assert_eq!(plan.downloads(), 0);
    assert!(plan.links.is_empty(), "host executables are not linked");
    assert_eq!(events, ["already-built gcc@14.2.0", "build app@1", "built app@1"]);
    assert!(!builder.workspace().package("gcc", "14.2.0").root.exists());

    // If the host loses it, the version is no longer satisfied (its
    // dependents' definitions did not change, so they stay installed).
    std::fs::remove_file(&host_gcc).unwrap();
    let (packages, ids) = resolve(defs(), &[("app", "1")]);
    let plan = builder.plan(packages, &ids);
    assert_eq!(names(plan.to_build()), ["gcc@14.2.0"]);
}

#[test]
fn one_process_at_a_time_per_workspace() {
    let home = temp("lock");
    let workspace = Workspace::new(home.join("nux"));
    let held = workspace.lock(false, |_| unreachable!()).unwrap();

    // A second holder (another open file, as another process would have).
    let err = workspace.lock(false, |_| unreachable!()).unwrap_err();
    let BuildError::Busy { pid, .. } = &err else { panic!("{err}") };
    assert_eq!(*pid, Some(std::process::id()));
    assert!(err.to_string().ends_with(&format!("is in use by another nux process (pid {})", std::process::id())));

    // Other workspaces are independent.
    let other = Workspace::new(home.join("other"));
    let _other = other.lock(false, |_| unreachable!()).unwrap();

    // Waiting blocks until the holder lets go.
    let waited = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        let waiter = scope.spawn(|| workspace.lock(true, |_| waited.store(true, Ordering::SeqCst)));
        while !waited.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!waiter.is_finished());
        drop(held);
        waiter.join().unwrap().unwrap();
    });
}

#[test]
fn the_lock_is_released_when_its_holder_dies() {
    let home = temp("lock-crash");
    let workspace = Workspace::new(&home);
    // A separate process holds the same OS lock, then is killed.
    std::fs::create_dir_all(&home).unwrap();
    // `-o`: only `flock` itself holds the lock, not the `sleep` it runs.
    let mut holder = Command::new("flock")
        .arg("-o")
        .arg(home.join(".lock"))
        .args(["sleep", "30"])
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while workspace.lock(false, |_| {}).is_ok() {
        assert!(std::time::Instant::now() < deadline, "flock never took the lock");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(matches!(workspace.lock(false, |_| {}), Err(BuildError::Busy { .. })));
    holder.kill().unwrap();
    holder.wait().unwrap();
    workspace.lock(false, |_| unreachable!()).unwrap();
}

fn git_head(repo: &Path) -> String {
    let out = Command::new("git").arg("-C").arg(repo).args(["rev-parse", "HEAD"]).output().unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn pinned_git_revisions_are_checked_out_exactly() {
    let home = temp("pin-git");
    let repo = home.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    std::fs::write(repo.join("v.txt"), "one").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    let first = git_head(&repo);
    std::fs::write(repo.join("v.txt"), "two").unwrap();
    git(&repo, &["commit", "-q", "-am", "two"]);
    let second = git_head(&repo);

    let url = format!("file://{}", repo.display());
    let defs = || vec![def("pkg", "1").source(SourceKind::Git(url.clone()), None).cmd("cp v.txt $NUX_PREFIX/")];
    let installed = |builder: &Builder| {
        std::fs::read_to_string(builder.workspace().package("pkg", "1").install.join("v.txt")).unwrap()
    };

    // Unpinned: the latest commit, recorded.
    let mut builder = Builder::new(home.join("nux"));
    sync(&mut builder, defs(), &[("pkg", "1")]).2.unwrap();
    assert_eq!(installed(&builder), "two");
    let identity = builder.workspace().source_identity("pkg", "1").unwrap();
    assert_eq!(identity, SourceIdentity { revision: Some(second.clone()), sha256: None });

    // Pinned to the same commit: nothing to do.
    let pin = |revision: &str| HashMap::from([(("pkg".to_string(), "1".to_string()), SourceIdentity { revision: Some(revision.into()), sha256: None })]);
    let mut builder = Builder::new(home.join("nux")).with_pins(pin(&second));
    let (packages, ids) = resolve(defs(), &[("pkg", "1")]);
    assert!(builder.plan(packages, &ids).is_up_to_date());

    // Pinned to an older commit: downloaded and rebuilt from it.
    builder = Builder::new(home.join("nux")).with_pins(pin(&first));
    let (plan, _, result) = sync(&mut builder, defs(), &[("pkg", "1")]);
    result.unwrap();
    assert_eq!(plan.downloads(), 1);
    assert_eq!(installed(&builder), "one");
    assert_eq!(builder.workspace().source_identity("pkg", "1").unwrap().revision, Some(first));

    // A revision the repository does not have.
    builder = Builder::new(home.join("nux")).with_pins(pin("0123456789abcdef0123456789abcdef01234567"));
    let err = sync(&mut builder, defs(), &[("pkg", "1")]).2.unwrap_err().to_string();
    assert!(err.contains("the locked revision 0123456789abcdef0123456789abcdef01234567 is not in"), "{err}");
}

#[test]
fn pinned_checksums_are_verified() {
    let home = temp("pin-url");
    let file = home.join("data.txt");
    std::fs::write(&file, "payload").unwrap();
    let url = SourceKind::Url(format!("file://{}", file.display()));
    let defs = || vec![def("data", "1").source(url.clone(), None).cmd("cp $NUX_SOURCE_PATH $NUX_PREFIX/")];
    let sha = "239f59ed55e737c77147cf55ad0c1b030b6d7ee748a7426952f9b852d5a935e5";

    let mut builder = Builder::new(home.join("nux"));
    sync(&mut builder, defs(), &[("data", "1")]).2.unwrap();
    assert_eq!(builder.workspace().source_identity("data", "1").unwrap().sha256.as_deref(), Some(sha));

    let pin = |sha: &str| HashMap::from([(("data".to_string(), "1".to_string()), SourceIdentity { revision: None, sha256: Some(sha.into()) })]);
    let wrong = "0".repeat(64);
    builder = Builder::new(home.join("other")).with_pins(pin(&wrong));
    let err = sync(&mut builder, defs(), &[("data", "1")]).2.unwrap_err().to_string();
    assert!(err.contains(&format!("checksum mismatch for `file://{}`: Nux.lock expects sha256 {wrong}, the download has {sha}", file.display())), "{err}");
    assert!(!builder.workspace().package("data", "1").source.exists());

    builder = Builder::new(home.join("other")).with_pins(pin(sha));
    sync(&mut builder, defs(), &[("data", "1")]).2.unwrap();
}

/// `name@version` of each step, group by group.
fn group_names(plan: &BuildPlan) -> Vec<Vec<String>> {
    plan.build_groups().into_iter().map(|group| names(group.into_iter())).collect()
}

#[test]
fn build_groups_never_hold_a_dependency_and_its_dependent() {
    let home = temp("groups");
    let builder = Builder::new(home.join("nux"));
    // A diamond (app -> a, b -> lib), plus a package needing nothing.
    let defs = || {
        vec![
            def("lib", "1").cmd("true"),
            def("a", "1").dep("lib", "1").cmd("true"),
            def("b", "1").dep("lib", "1").cmd("true"),
            def("app", "1").dep("a", "1").dep("b", "1").cmd("true"),
            def("solo", "1").cmd("true"),
        ]
    };
    let requests = [("app", "1"), ("solo", "1")];
    let (packages, ids) = resolve(defs(), &requests);
    let plan = builder.plan(packages, &ids);
    let groups = group_names(&plan);
    assert_eq!(groups.len(), 3, "{groups:?}");
    let mut first = groups[0].clone();
    first.sort();
    assert_eq!(first, ["lib@1", "solo@1"]);
    assert_eq!(groups[1], ["a@1", "b@1"]);
    assert_eq!(groups[2], ["app@1"]);

    // An installed dependency holds nothing back.
    builder.build(&builder.plan(resolve(defs(), &[("lib", "1")]).0, &[]), &Silent).unwrap();
    let (packages, ids) = resolve(defs(), &requests);
    let groups = group_names(&builder.plan(packages, &ids));
    assert_eq!(groups.len(), 2, "{groups:?}");
    assert_eq!(groups[1], ["app@1"]);
}

/// A package whose build takes `seconds`, and records when it ran and the
/// jobs it was given in `$NUX_PREFIX/run`.
fn slow(name: &'static str, seconds: &str) -> Def {
    def(name, "1").cmd(&format!(
        "start=$(date +%s.%N); sleep {seconds}; echo \"$start $(date +%s.%N) $NUX_JOBS\" > $NUX_PREFIX/run"
    ))
}

/// When each package's build ran: (start, end, jobs).
fn run_of(builder: &Builder, name: &str) -> (f64, f64, usize) {
    let text = std::fs::read_to_string(builder.workspace().package(name, "1").install.join("run")).unwrap();
    let parts: Vec<&str> = text.split_whitespace().collect();
    (parts[0].parse().unwrap(), parts[1].parse().unwrap(), parts[2].parse().unwrap())
}

#[test]
fn independent_packages_are_built_at_the_same_time() {
    let defs = || vec![slow("p1", "0.5"), slow("p2", "0.5"), slow("p3", "0.5"), slow("p4", "0.5")];
    let requests = [("p1", "1"), ("p2", "1"), ("p3", "1"), ("p4", "1")];
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());

    let builder = Builder::new(temp("parallel").join("nux"));
    let (packages, ids) = resolve(defs(), &requests);
    let started = std::time::Instant::now();
    assert_eq!(builder.build(&builder.plan(packages, &ids), &Silent).unwrap(), 4);
    // One after the other would take 2s.
    assert!(started.elapsed() < std::time::Duration::from_millis(1500), "{:?}", started.elapsed());
    // Each build may use every CPU.
    assert_eq!(run_of(&builder, "p1").2, cpus);

    let builder = Builder::new(temp("sequential").join("nux")).with_parallel_builds(1);
    let (packages, ids) = resolve(defs(), &requests);
    builder.build(&builder.plan(packages, &ids), &Silent).unwrap();
    let mut runs: Vec<_> = ["p1", "p2", "p3", "p4"].iter().map(|n| run_of(&builder, n)).collect();
    runs.sort_by(|a, b| a.0.total_cmp(&b.0));
    for pair in runs.windows(2) {
        assert!(pair[1].0 >= pair[0].1, "one at a time: {runs:?}");
    }
    assert_eq!(runs[0].2, cpus);
}

#[test]
fn dependents_wait_for_their_dependencies() {
    let builder = Builder::new(temp("waits").join("nux"));
    let defs = vec![
        slow("dep", "0.3"),
        slow("other", "0.1"),
        def("top", "1").dep("dep", "1").cmd("cp $NUX_DEP_DEP/run $NUX_PREFIX/copied && date +%s.%N > $NUX_PREFIX/started"),
    ];
    let (packages, ids) = resolve(defs, &[("top", "1"), ("other", "1")]);
    builder.build(&builder.plan(packages, &ids), &Silent).unwrap();
    let started: f64 = std::fs::read_to_string(builder.workspace().package("top", "1").install.join("started"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(started >= run_of(&builder, "dep").1);
}

#[test]
fn a_failure_stops_new_builds_and_keeps_the_others() {
    let builder = Builder::new(temp("parallel-failure").join("nux")).with_parallel_builds(2);
    let defs = vec![
        slow("aslow", "0.5"),
        def("broken", "1").cmd("exit 4"),
        def("later", "1").cmd("true"),
        def("top", "1").dep("aslow", "1").dep("broken", "1").cmd("true"),
    ];
    let (packages, ids) = resolve(defs, &[("top", "1"), ("later", "1")]);
    let plan = builder.plan(packages, &ids);
    assert_eq!(group_names(&plan)[0], ["aslow@1", "broken@1", "later@1"]);
    let recorder = Recorder::default();
    let err = builder.build(&plan, &recorder).unwrap_err();
    assert!(err.to_string().starts_with("failed to build broken 1: `exit 4`"), "{err}");
    let events = recorder.events();
    // With two at a time, `aslow` ran alongside `broken` and finished;
    // nothing started after the failure.
    assert!(events.contains(&"built aslow@1".to_string()), "{events:?}");
    assert!(!events.iter().any(|e| e.ends_with("later@1") || e.ends_with("top@1")), "{events:?}");
    assert!(builder.workspace().package("aslow", "1").install.join("run").is_file());
}

#[test]
fn named_links_let_versions_live_side_by_side() {
    let home = temp("named-links");
    let mut builder = Builder::new(home.join("nux"));
    let defs = || {
        vec![
            def("lua", "5.4").bin("bin/lua").named("lua5.4"),
            def("lua", "5.3").bin("bin/lua").named("lua5.3"),
            def("app", "1").dep("lua", "5.3").cmd("true"),
        ]
    };
    let (_, events, result) = sync(&mut builder, defs(), &[("lua", "5.4"), ("app", "1")]);
    result.unwrap_or_else(|e| panic!("{e} {events:?}"));
    // Both versions are linked, each under its own name.
    assert_eq!(run_bin(&builder, "lua5.4"), "lua 5.4");
    assert_eq!(run_bin(&builder, "lua5.3"), "lua 5.3");
    assert!(builder.bin_path().join("lua").symlink_metadata().is_err());
}
