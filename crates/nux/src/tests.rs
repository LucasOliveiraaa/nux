//! End-to-end tests of the commands against temporary projects and
//! workspaces.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use nux_build::Workspace;
use nux_eval::Host;
use nux_log::Term;

use crate::commands::sync::{Io, Options, Outcome, run};
use crate::commands::{
    clean as clean_command, init, install, purge as purge_command, remove, reset as reset_command,
    status as status_command,
};
use crate::config::Config;
use crate::environment::Selection;

/// A clonable in-memory writer, standing in for stdout.
#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Project {
    dir: PathBuf,
}

impl Project {
    fn new(name: &str, packages: &str, definitions: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir()
            .join(format!("nux-sync-tests-{}", std::process::id()))
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("repository")).unwrap();
        let project = Self { dir };
        project.write_env(packages);
        for (name, source) in definitions {
            project.define(name, source);
        }
        project
    }

    fn write_env(&self, packages: &str) {
        let toml = format!(
            "[environment]\nname = \"test\"\n\n[packages]\n{packages}\n\n[[repository]]\npath = \"./repository\"\n"
        );
        std::fs::write(self.dir.join("Nux.toml"), toml).unwrap();
    }

    fn define(&self, name: &str, source: &str) {
        std::fs::write(
            self.dir.join("repository").join(format!("{name}.nux")),
            source,
        )
        .unwrap();
    }

    fn home(&self) -> PathBuf {
        self.dir.join("home")
    }

    /// Runs `nux sync -e <project>` answering `input`; returns the outcome
    /// and the output with durations masked.
    fn sync(&self, yes: bool, input: &str) -> (Result<Outcome, String>, String) {
        self.sync_with(
            Options {
                yes,
                ..Options::default()
            },
            input,
        )
    }

    fn sync_with(&self, options: Options, input: &str) -> (Result<Outcome, String>, String) {
        let out = Buffer::default();
        let options = Options {
            environment: at(&self.dir),
            ..options
        };
        let io = Io {
            term: Term::PLAIN,
            out: out.clone(),
            input: input.as_bytes(),
            workspace: Workspace::new(self.home()),
            path: Some(self.home().join("bin").into_os_string()),
            host: Host::empty(),
            config: Config::default(),
        };
        let result = run(&options, io);
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        (result, mask_durations(&text))
    }
}

/// `-e <dir>`.
fn at(dir: &Path) -> Selection {
    Selection {
        path: Some(dir.to_path_buf()),
        global: false,
    }
}

fn mask_durations(text: &str) -> String {
    text.lines()
        .map(|line| match line.rsplit_once(" in ") {
            Some((head, tail))
                if tail.ends_with('s') && tail.starts_with(|c: char| c.is_ascii_digit()) =>
            {
                format!("{head} in <time>")
            }
            _ => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// A definition whose build installs an executable printing `name version`.
fn tool(name: &str, version: &str, extra: &str) -> String {
    format!(
        r#"program {name} {{
    version("{version}");
    {extra}
    bash {{
        mkdir -p $NUX_PREFIX/bin
        printf '#!/bin/sh\necho {name} {version}\n' > $NUX_PREFIX/bin/{name}
        chmod +x $NUX_PREFIX/bin/{name}
    }}
    set_bin("bin/{name}");
}}"#
    )
}

const UP_TO_DATE: &str = "Nux v0.2.0

Environment: test

Resolving dependencies...
Checking installed packages...

Environment is already up to date.
";

#[test]
fn sync_builds_then_reports_up_to_date() {
    let archive =
        std::env::temp_dir().join(format!("nux-sync-tests-{}-lib.txt", std::process::id()));
    std::fs::write(&archive, "payload").unwrap();
    let project = Project::new(
        "full",
        "app = \"1.0\"",
        &[
            (
                "lib",
                &format!(
                    r#"program lib {{ version("2.0"); source(url = "file://{}"); bash {{ cp $NUX_SOURCE_PATH $NUX_PREFIX/ }} }}"#,
                    archive.display()
                ),
            ),
            ("app", &tool("app", "1.0", "add_dependency(lib);")),
            (
                "unrelated",
                r#"program unrelated { version("1"); bash { false } }"#,
            ),
        ],
    );

    let (result, output) = project.sync(false, "y\n");
    assert_eq!(result, Ok(Outcome::Synced), "{output}");
    // The prompt leaves the cursor after `] `; the rate varies between runs.
    let output = output.replace("[Y/n] \n", "[Y/n]\n");
    let output: Vec<_> = output
        .lines()
        .map(|l| {
            if l.starts_with("  Downloading") {
                l.split_inclusive('%').next().unwrap()
            } else {
                l
            }
        })
        .collect();
    assert_eq!(
        output.join("\n") + "\n",
        format!(
            "Nux v0.1.0

Environment: test

Resolving dependencies...
Checking installed packages...

The environment requests the following packages:
  app 1.0

The following packages will be built:
  lib 2.0
  app 1.0

2 packages to build, 1 source to download.

Continue? [Y/n]
No Nux workspace found, creating one...

  Downloading lib [==============================] 100%

  Finished downloading 1 source in <time>

  Updated `{lock}`.

  Building lib... Done!
  Building app... Done!

  Finished building 2 packages in <time>

  Updating environment...
  Updating executable links...

Nux finished syncing the environment.
",
            lock = project.dir.join("Nux.lock").display()
        )
    );
    let link = project.home().join("bin/app");
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        Path::new("../packages/app/1.0/install/bin/app")
    );
    assert!(
        project
            .home()
            .join("packages/lib/2.0/install")
            .join(archive.file_name().unwrap())
            .is_file()
    );

    let (result, output) = project.sync(false, "");
    assert_eq!(result, Ok(Outcome::UpToDate));
    assert_eq!(output, UP_TO_DATE);
}

#[test]
fn only_missing_packages_are_built_after_a_change() {
    let project = Project::new(
        "incremental",
        "a = \"1\"",
        &[("a", &tool("a", "1", "")), ("b", &tool("b", "1", ""))],
    );
    assert_eq!(project.sync(true, "").0, Ok(Outcome::Synced));

    project.write_env("a = \"1\"\nb = \"1\"");
    let (result, output) = project.sync(true, "");
    assert_eq!(result, Ok(Outcome::Synced));
    assert!(output.contains("The following packages will be built:\n  b 1\n\n1 package to build, 0 sources to download.\n"), "{output}");
    assert!(
        output.contains("  Building a... Already built\n  Building b... Done!\n"),
        "{output}"
    );
    assert!(!output.contains("Continue?"));
    assert!(!output.contains("No Nux workspace found"));
    assert!(!output.contains("Downloading"));
}

#[test]
fn declining_changes_nothing() {
    let project = Project::new("decline", "a = \"1\"", &[("a", &tool("a", "1", ""))]);
    let (result, output) = project.sync(false, "maybe\nn\n");
    assert_eq!(result, Ok(Outcome::Cancelled));
    assert!(output.ends_with("Continue? [Y/n] Please answer `y` or `n`.\n\nContinue? [Y/n] \nSync cancelled; nothing was changed.\n"), "{output}");
    // Only the workspace lock file may exist: no packages, links or state.
    assert!(!project.home().join("packages").exists() && !project.home().join("bin").exists());
    assert!(!project.dir.join("Nux.lock").exists());

    let (result, _) = project.sync(false, "");
    assert_eq!(
        result,
        Err("no answer to the confirmation prompt; pass `--yes` to sync without asking".into())
    );
}

#[test]
fn build_failure_leaves_environment_unsynced() {
    let project = Project::new(
        "failure",
        "top = \"1\"",
        &[
            ("dep", &tool("dep", "1", "")),
            (
                "top",
                r#"program top { version("1"); add_dependency(dep); bash { echo broken >&2 } bash { exit 1 } }"#,
            ),
        ],
    );
    let (result, output) = project.sync(true, "");
    let err = result.unwrap_err();
    assert!(err.starts_with("failed to build top 1: `exit 1` exited with exit status: 1\n$ echo broken >&2\nbroken\n$ exit 1\nfull log: "), "{err}");
    assert!(
        output.contains("  Building dep... Done!\n  Building top... Failed\n"),
        "{output}"
    );
    assert!(!output.contains("Finished building"));
    assert!(!output.contains("finished syncing"));
    assert!(!project.home().join("state/environments/test.json").exists());

    // Fixing the definition rebuilds only what failed.
    project.define(
        "top",
        r#"program top { version("1"); add_dependency(dep); bash { true } }"#,
    );
    let (result, output) = project.sync(true, "");
    assert_eq!(result, Ok(Outcome::Synced));
    assert!(
        output.contains("The following packages will be built:\n  top 1\n"),
        "{output}"
    );
    assert_eq!(project.sync(true, "").1, UP_TO_DATE);
}

#[test]
fn moved_links_are_updated_without_building() {
    let project = Project::new(
        "links",
        "py = \"3\"",
        &[(
            "py",
            "program py { version(\"2\"); bash { mkdir -p $NUX_PREFIX/bin && printf '#!/bin/sh\\n' > $NUX_PREFIX/bin/py && chmod +x $NUX_PREFIX/bin/py } set_bin(\"bin/py\"); version(\"3\"); bash { mkdir -p $NUX_PREFIX/bin && printf '#!/bin/sh\\n' > $NUX_PREFIX/bin/py && chmod +x $NUX_PREFIX/bin/py } set_bin(\"bin/py\"); }",
        )],
    );
    project.sync(true, "").0.unwrap();
    project.write_env("py = \"2\"");
    project.sync(true, "").0.unwrap();
    project.write_env("py = \"3\"");

    let (result, output) = project.sync(false, "");
    assert_eq!(result, Ok(Outcome::Synced), "{output}");
    assert!(
        !output.contains("Continue?") && !output.contains("Building"),
        "{output}"
    );
    assert!(output.contains("  Updating executable links...\n"));
    assert_eq!(
        std::fs::read_link(project.home().join("bin/py")).unwrap(),
        Path::new("../packages/py/3/install/bin/py")
    );
}

#[test]
fn link_conflicts_are_reported_before_building() {
    let project = Project::new("conflict", "a = \"1\"", &[("a", &tool("a", "1", ""))]);
    std::fs::create_dir_all(project.home().join("bin")).unwrap();
    std::fs::write(project.home().join("bin/a"), "mine").unwrap();
    let (result, output) = project.sync(true, "");
    let err = result.unwrap_err();
    assert!(
        err.starts_with("cannot link `a`: `")
            && err.ends_with("bin/a` already exists and it is not a link created by nux"),
        "{err}"
    );
    assert!(!output.contains("Building"));
    assert!(!project.home().join("packages/a").exists());
}

#[test]
fn resolution_errors_are_useful() {
    let project = Project::new(
        "missing",
        "app = \"1\"",
        &[(
            "app",
            r#"program app { version("1"); add_dependency(ghost); }"#,
        )],
    );
    let err = project.sync(true, "").0.unwrap_err();
    assert_eq!(
        err,
        format!(
            "in dependencies of `app` 1: package `ghost` is not defined (searched: `{}`)",
            project.dir.join("./repository").display()
        )
    );

    project.write_env("app = \">=2\"");
    let err = project.sync(true, "").0.unwrap_err();
    assert_eq!(err, "no version of `app` satisfies `>=2` (available: 1)");

    project.define(
        "app",
        r#"program app { version("1"); add_dependency(app, "1"); }"#,
    );
    project.write_env("app = \"1\"");
    assert_eq!(
        project.sync(true, "").0.unwrap_err(),
        "dependency cycle: app 1 -> app 1"
    );
}

#[test]
fn path_hint_only_when_bin_is_not_on_path() {
    let project = Project::new("path", "a = \"1\"", &[("a", &tool("a", "1", ""))]);
    let out = Buffer::default();
    let io = Io {
        term: Term::PLAIN,
        out: out.clone(),
        input: "".as_bytes(),
        workspace: Workspace::new(project.home()),
        path: Some("/usr/bin".into()),
        host: Host::empty(),
        config: Config::default(),
    };
    run(
        &Options {
            environment: at(&project.dir),
            yes: true,
            ..Options::default()
        },
        io,
    )
    .unwrap();
    let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
    assert!(
        text.contains(
            "to your PATH.\nAdd the following line to your shell's rc file:\n\n  export PATH=\""
        ),
        "{text}"
    );
}

fn status(project: &Project) -> (Result<status_command::Outcome, String>, String) {
    let mut out = Vec::new();
    let result = status_command::run(
        &at(&project.dir),
        &Config::default(),
        &mut out,
        Term::PLAIN,
        &Workspace::new(project.home()),
        &Host::empty(),
    );
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn init_creates_an_environment_that_syncs() {
    let dir = std::env::temp_dir()
        .join(format!("nux-sync-tests-{}", std::process::id()))
        .join("init");
    let _ = std::fs::remove_dir_all(&dir);
    init::run(Some(&dir), &mut Vec::new(), Term::PLAIN).unwrap();
    let project = Project { dir };

    let (result, output) = project.sync(true, "");
    assert_eq!(result, Ok(Outcome::Synced), "{output}");
    let hello = std::process::Command::new(project.home().join("bin/hello"))
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(hello.stdout).unwrap(),
        "Hello from Nux!\n"
    );
    assert_eq!(project.sync(true, "").0, Ok(Outcome::UpToDate));
}

#[test]
fn status_reports_requests_installs_and_pending_work() {
    let project = Project::new(
        "status",
        "app = \"^1\"",
        &[
            ("app", &tool("app", "1.0", "add_dependency(lib);")),
            ("lib", r#"program lib { version("1"); bash { true } }"#),
        ],
    );
    let (result, output) = status(&project);
    assert_eq!(result, Ok(status_command::Outcome::OutOfDate));
    assert_eq!(
        output,
        format!(
            "Nux v0.1.0

Environment: test ({env})
Workspace:   {home} (not created yet)

Requested packages:
  app ^1 -> 1.0 not built

Dependencies:
  lib 1 not built

Executable links:
  app -> app 1.0 not linked

Lock file:   Nux.lock needs updating: it does not exist yet

Environment is not synchronized: 2 packages to build, 0 sources to download, 1 link to update, Nux.lock to update.
Run `nux sync` to update it.
",
            env = project.dir.join("Nux.toml").display(),
            home = project.home().display()
        )
    );
    assert!(!project.home().exists(), "status must not write anything");

    project.sync(true, "").0.unwrap();
    // A newer version appears: status shows the old one is still installed.
    let with_newer = tool("app", "1.0", "add_dependency(lib);").replace(
        "program app {",
        "program app {\n    version(\"1.1\"); bash { true }",
    );
    project.define("app", &with_newer);
    // Nux.lock keeps the locked 1.0 while it still matches `^1`...
    let (result, output) = status(&project);
    assert_eq!(result, Ok(status_command::Outcome::UpToDate), "{output}");
    assert!(output.contains("  app ^1 -> 1.0 installed\n"), "{output}");
    // ...and without it, the newest version is selected.
    let lock = std::fs::read_to_string(project.dir.join("Nux.lock")).unwrap();
    std::fs::remove_file(project.dir.join("Nux.lock")).unwrap();
    let (result, output) = status(&project);
    assert_eq!(result, Ok(status_command::Outcome::OutOfDate));
    assert!(
        output.contains("  app ^1 -> 1.1 not built (also installed: 1.0)\n"),
        "{output}"
    );
    std::fs::write(project.dir.join("Nux.lock"), lock).unwrap();

    project.define("app", &tool("app", "1.0", "add_dependency(lib);"));
    let (result, output) = status(&project);
    assert_eq!(result, Ok(status_command::Outcome::UpToDate));
    assert!(
        output.contains("  app ^1 -> 1.0 installed\n")
            && output.ends_with("\nEnvironment is up to date.\n"),
        "{output}"
    );
    assert!(output.contains("  app -> app 1.0 linked\n"), "{output}");

    project.define(
        "app",
        r#"program app { version("1.0"); add_dependency(ghost); }"#,
    );
    let (result, output) = status(&project);
    assert!(
        result
            .unwrap_err()
            .contains("package `ghost` is not defined")
    );
    assert!(
        output.ends_with("Requested packages:\n  app ^1\n"),
        "{output}"
    );
}

/// The items `clean` lists, without their sizes (which depend on logs).
fn listed_items(output: &str) -> Vec<String> {
    output
        .lines()
        .skip_while(|l| !l.starts_with("The following will be removed:"))
        .skip(1)
        .take_while(|l| !l.is_empty())
        .map(|l| l.rsplitn(3, ' ').nth(2).unwrap_or(l).trim_end().to_string())
        .collect()
}

fn clean(
    project: &Project,
    options: clean_command::Options,
    input: &str,
) -> (Result<clean_command::Outcome, String>, String) {
    let mut out = Vec::new();
    let result = clean_command::run(
        options,
        &mut out,
        &mut input.as_bytes(),
        Term::PLAIN,
        &Workspace::new(project.home()),
    );
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn clean_removes_leftovers_and_keeps_the_environment_synced() {
    use clean_command::{Options as CleanOptions, Outcome as Cleaned};
    let project = Project::new(
        "clean",
        "app = \"1.0\"\nbroken = \"1\"",
        &[
            ("app", &tool("app", "1.0", "")),
            (
                "broken",
                r#"program broken { version("1"); bash { false } }"#,
            ),
        ],
    );
    assert_eq!(
        clean(&project, CleanOptions::default(), "").0,
        Ok(Cleaned::NothingToClean)
    );
    project.sync(true, "").0.unwrap_err();

    let (result, output) = clean(
        &project,
        CleanOptions {
            dry_run: true,
            ..Default::default()
        },
        "",
    );
    assert_eq!(result, Ok(Cleaned::DryRun));
    assert!(
        listed_items(&output)
            == [
                "  build log       app 1.0",
                "  build directory broken 1",
                "  build log       broken 1"
            ],
        "{output}"
    );
    assert!(output.contains("\n3 items, "), "{output}");
    assert!(output.ends_with("Dry run: nothing was removed.\n"));
    assert_eq!(
        clean(&project, CleanOptions::default(), "n\n").0,
        Ok(Cleaned::Cancelled)
    );
    assert!(project.home().join("packages/broken/1/build").exists());

    let (result, output) = clean(&project, CleanOptions::default(), "y\n");
    assert_eq!(result, Ok(Cleaned::Cleaned));
    assert!(output.contains("  Removed 3 items, freed "), "{output}");
    assert_eq!(
        clean(&project, CleanOptions::default(), "").0,
        Ok(Cleaned::NothingToClean)
    );

    // Removing everything removable keeps installations usable and synced.
    project.write_env("app = \"1.0\"");
    project.sync(true, "").0.unwrap();
    let (result, output) = clean(
        &project,
        CleanOptions {
            yes: true,
            sources: true,
            ..Default::default()
        },
        "",
    );
    assert_eq!(result, Ok(Cleaned::Cleaned), "{output}");
    assert!(output.contains("uninstalled package broken 1"), "{output}");
    assert!(!project.home().join("packages/broken").exists());
    assert_eq!(project.sync(true, "").1, UP_TO_DATE);
}

/// A host whose only executable is a `gcc` reporting `version`.
fn fake_host(project: &Project, version: &str) -> Host {
    use std::os::unix::fs::PermissionsExt;
    let dir = project.dir.join("host-bin");
    std::fs::create_dir_all(&dir).unwrap();
    let gcc = dir.join("gcc");
    std::fs::write(
        &gcc,
        format!("#!/bin/sh\necho 'gcc (Debian {version}-1) {version}'\n"),
    )
    .unwrap();
    std::fs::set_permissions(&gcc, std::fs::Permissions::from_mode(0o755)).unwrap();
    Host::new(vec![dir], vec![project.home()])
}

const GCC_RECIPE: &str = r#"program gcc {
    let host = host_version("gcc");
    if host {
        version(host);
        use_host("gcc");
    }
    if host != "14.2.0" {
        version("14.2.0");
        source(url = "https://example.invalid/gcc-14.2.0.tar.xz");
        bash { false }
        set_bin("bin/gcc");
    }
}"#;

#[test]
fn host_compilers_satisfy_dependencies_without_building() {
    let project = Project::new(
        "host",
        "app = \"1\"",
        &[
            ("gcc", GCC_RECIPE),
            ("app", &tool("app", "1", "add_dependency(gcc);")),
        ],
    );
    let run_with = |host: Host| {
        let out = Buffer::default();
        let io = Io {
            term: Term::PLAIN,
            out: out.clone(),
            input: "".as_bytes(),
            workspace: Workspace::new(project.home()),
            path: Some(project.home().join("bin").into_os_string()),
            host,
            config: Config::default(),
        };
        let result = run(
            &Options {
                environment: at(&project.dir),
                yes: true,
                ..Options::default()
            },
            io,
        );
        (
            result,
            mask_durations(&String::from_utf8(out.0.lock().unwrap().clone()).unwrap()),
        )
    };

    let (result, output) = run_with(fake_host(&project, "12.2.0"));
    assert_eq!(result, Ok(Outcome::Synced), "{output}");
    let gcc = project.dir.join("host-bin/gcc");
    assert!(
        output.contains(&format!(
            "The following packages are provided by the host:\n  gcc 12.2.0 {}\n\nThe following packages will be built:\n  app 1\n",
            gcc.display()
        )),
        "{output}"
    );
    assert!(
        output.contains("  Building gcc... Provided by the host\n  Building app... Done!\n"),
        "{output}"
    );
    assert!(!project.home().join("packages/gcc").exists());
    assert!(!project.home().join("bin/gcc").exists());

    let mut out = Vec::new();
    status_command::run(
        &at(&project.dir),
        &Config::default(),
        &mut out,
        Term::PLAIN,
        &Workspace::new(project.home()),
        &fake_host(&project, "12.2.0"),
    )
    .unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains(&format!(
            "  gcc 12.2.0 provided by the host ({})\n",
            gcc.display()
        )),
        "{out}"
    );
    assert!(out.ends_with("Environment is up to date.\n"), "{out}");

    // Without a host compiler the recipe's own build is required (and here, fails).
    let (result, output) = run_with(Host::empty());
    assert!(
        result
            .unwrap_err()
            .starts_with("failed to download gcc 14.2.0"),
        "{output}"
    );
}

fn git_repo(dir: &Path, file_contents: &str) -> String {
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    if !dir.join(".git").exists() {
        std::fs::create_dir_all(dir).unwrap();
        git(&["init", "-q"]);
    }
    std::fs::write(dir.join("file.txt"), file_contents).unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", file_contents]);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn sync_writes_a_lock_that_pins_versions_and_sources() {
    let project = Project::new("lockfile", "app = \"^1\"", &[]);
    let repo = project.dir.join("upstream");
    let first = git_repo(&repo, "one");
    let archive = project.dir.join("data.txt");
    std::fs::write(&archive, "payload").unwrap();
    project.define("data", &format!(
        r#"program data {{ version("1.0"); source(url = "file://{}"); bash {{ cp $NUX_SOURCE_PATH $NUX_PREFIX/ }} }}"#,
        archive.display()
    ));
    let app = |versions: &[&str]| {
        versions
            .iter()
            .map(|v| format!(
                r#"version("{v}"); source(git = "file://{}"); add_dependency(data); bash {{ cp file.txt $NUX_PREFIX/ }}"#,
                repo.display()
            ))
            .collect::<Vec<_>>()
            .join("\n")
    };
    project.define("app", &format!("program app {{ {} }}", app(&["1.0"])));
    let installed = || {
        std::fs::read_to_string(project.home().join("packages/app/1.0/install/file.txt")).unwrap()
    };

    // `--locked` needs an existing lock.
    let err = project
        .sync_with(
            Options {
                yes: true,
                locked: true,
                ..Options::default()
            },
            "",
        )
        .0
        .unwrap_err();
    assert!(
        err.ends_with("Nux.lock` does not exist; run `nux sync` without `--locked` to create it"),
        "{err}"
    );

    project.sync(true, "").0.unwrap();
    let lock_path = project.dir.join("Nux.lock");
    let lock = std::fs::read_to_string(&lock_path).unwrap();
    assert_eq!(
        lock,
        format!(
            r#"# Generated by `nux sync`: the exact versions and sources this environment
# resolved to. Commit it to reproduce them; do not edit it by hand.
version = 1

[[package]]
name = "app"
version = "1.0"
requested = true
source = "git+file://{repo}"
revision = "{first}"
dependencies = [
    "data 1.0",
]

[[package]]
name = "data"
version = "1.0"
source = "url+file://{archive}"
checksum = "sha256:239f59ed55e737c77147cf55ad0c1b030b6d7ee748a7426952f9b852d5a935e5"
"#,
            repo = repo.display(),
            archive = archive.display()
        )
    );
    assert_eq!(installed(), "one");

    // Upstream moves and a newer version appears: the lock keeps both.
    git_repo(&repo, "two");
    project.define(
        "app",
        &format!("program app {{ {} }}", app(&["1.0", "1.1"])),
    );
    assert_eq!(project.sync(true, "").1, UP_TO_DATE);
    assert_eq!(std::fs::read_to_string(&lock_path).unwrap(), lock);

    // A fresh workspace reproduces the locked revision, not the new one.
    std::fs::remove_dir_all(project.home()).unwrap();
    let (result, output) = project.sync_with(
        Options {
            yes: true,
            locked: true,
            ..Options::default()
        },
        "",
    );
    assert_eq!(result, Ok(Outcome::Synced), "{output}");
    assert_eq!(installed(), "one");
    assert!(
        !output.contains("Updated `"),
        "--locked never writes: {output}"
    );
    assert_eq!(std::fs::read_to_string(&lock_path).unwrap(), lock);

    // Requiring something else would change the lock: refused with --locked.
    project.write_env("app = \"1.1\"");
    let err = project
        .sync_with(
            Options {
                yes: true,
                locked: true,
                ..Options::default()
            },
            "",
        )
        .0
        .unwrap_err();
    assert!(err.ends_with("Nux.lock` is out of date: `app` would change from 1.0 to 1.1; run `nux sync` without `--locked` to update it"), "{err}");
    assert!(!project.home().join("packages/app/1.1").exists());
    assert_eq!(std::fs::read_to_string(&lock_path).unwrap(), lock);

    // Without --locked the lock follows, atomically replaced.
    project.sync(true, "").0.unwrap();
    let updated = std::fs::read_to_string(&lock_path).unwrap();
    assert!(
        updated.contains("name = \"app\"\nversion = \"1.1\"\nrequested = true\n"),
        "{updated}"
    );
    assert!(!project.dir.join("Nux.tmp").exists() && !project.dir.join("Nux.lock.tmp").exists());

    // A tampered download no longer matches the locked checksum.
    std::fs::write(&archive, "tampered").unwrap();
    std::fs::remove_dir_all(project.home()).unwrap();
    let err = project.sync(true, "").0.unwrap_err();
    assert!(
        err.contains("checksum mismatch for `file://")
            && err.contains("Nux.lock expects sha256 239f59ed"),
        "{err}"
    );
}

#[test]
fn a_busy_workspace_is_reported_or_waited_for() {
    let project = Project::new("busy", "a = \"1\"", &[("a", &tool("a", "1", ""))]);
    let workspace = Workspace::new(project.home());
    let held = workspace.lock(false, |_| {}).unwrap();

    let err = project.sync(true, "").0.unwrap_err();
    assert_eq!(
        err,
        format!(
            "the Nux workspace `{}` is in use by another nux process (pid {}); pass `--wait` to wait for it to finish",
            project.home().display(),
            std::process::id()
        )
    );
    assert!(!project.home().join("packages").exists());
    std::fs::create_dir_all(project.home().join("packages")).unwrap();
    let err = clean(&project, clean_command::Options::default(), "")
        .0
        .unwrap_err();
    assert!(err.contains("is in use by another nux process"), "{err}");

    // With --wait, sync proceeds once the other process is done.
    std::thread::scope(|scope| {
        let waiter = scope.spawn(|| {
            project.sync_with(
                Options {
                    yes: true,
                    wait: true,
                    ..Options::default()
                },
                "",
            )
        });
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!waiter.is_finished());
        drop(held);
        let (result, output) = waiter.join().unwrap();
        assert_eq!(result, Ok(Outcome::Synced), "{output}");
        assert!(
            output.contains(&format!(
                "Waiting for another nux process (pid {}) to finish with the workspace...",
                std::process::id()
            )),
            "{output}"
        );
    });
}

#[test]
fn recipes_can_run_programs_and_return_early() {
    let project = Project::new(
        "exec",
        "probe = \"latest\"",
        &[(
            "probe",
            r#"program probe {
                let result = exec("sh", ["-c", "echo 2.5.0"]);
                if !result.success { return; }
                version(trim(result.stdout));
                bash { mkdir -p $NUX_PREFIX }
                return;
                version("9.9.9");
            }"#,
        )],
    );
    let (result, output) = project.sync(true, "");
    assert_eq!(result, Ok(Outcome::Synced), "{output}");
    assert!(
        output.contains("The following packages will be built:\n  probe 2.5.0\n"),
        "{output}"
    );

    // A program that cannot start is an evaluation error with a location.
    project.define(
        "probe",
        r#"program probe { let r = exec("no-such-program-nux"); version("1"); }"#,
    );
    let err = project.sync(true, "").0.unwrap_err();
    assert!(
        err.starts_with("cannot run `no-such-program-nux`: "),
        "{err}"
    );
    assert!(err.contains("probe.nux:1:25"), "{err}");
}

fn have(tool: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {tool}"))
        .output()
        .is_ok_and(|o| o.status.success())
}

fn write_files(dir: &Path, files: &[(&str, &str)]) {
    use std::os::unix::fs::PermissionsExt;
    for (name, contents) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        if *name == "configure" {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

#[test]
fn build_system_helpers_build_real_projects() {
    let project = Project::new("build-systems", "", &[]);
    let src = project.dir.join("src");

    // A static library built with `make`, found by a CMake project through
    // the CMAKE_PREFIX_PATH Nux exports.
    write_files(
        &src.join("greet"),
        &[
            ("greet.h", "const char *greet(void);\n"),
            (
                "greet.c",
                "const char *greet(void) { return \"hello from make\"; }\n",
            ),
            (
                "Makefile",
                "PREFIX ?= /usr/local\nall: libgreet.a\nlibgreet.a: greet.c\n\tcc -c -fPIC greet.c -o greet.o && ar rcs libgreet.a greet.o\ninstall: all\n\tmkdir -p $(PREFIX)/lib $(PREFIX)/include\n\tcp libgreet.a $(PREFIX)/lib/ && cp greet.h $(PREFIX)/include/\n",
            ),
        ],
    );
    project.define("greet", r#"program greet { version("1.0"); source(local = "src/greet"); make(); export_lib("greet"); }"#);
    write_files(
        &src.join("app"),
        &[
            (
                "CMakeLists.txt",
                "cmake_minimum_required(VERSION 3.18)\nproject(app C)\nfind_library(GREET greet REQUIRED NO_DEFAULT_PATH PATHS ENV CMAKE_PREFIX_PATH PATH_SUFFIXES lib)\nfind_path(GREET_INCLUDE greet.h REQUIRED)\nadd_executable(app main.c)\ntarget_include_directories(app PRIVATE ${GREET_INCLUDE})\ntarget_link_libraries(app ${GREET})\ninstall(TARGETS app DESTINATION bin)\n",
            ),
            (
                "main.c",
                "#include <stdio.h>\n#include <greet.h>\nint main(void) { puts(greet()); return 0; }\n",
            ),
        ],
    );
    project.define("app", r#"program app { version("1.0"); source(local = "src/app"); add_dependency(greet); cmake(build_type = "Debug"); set_bin("bin/app"); }"#);

    // Autotools without autoconf: a hand-written configure script.
    write_files(
        &src.join("conf"),
        &[
            (
                "configure",
                "#!/bin/sh\nfor arg; do case $arg in --prefix=*) prefix=${arg#--prefix=};; esac; done\necho \"PREFIX=$prefix\" > config.mk\n",
            ),
            (
                "Makefile",
                "include config.mk\nall:\n\tprintf '#!/bin/sh\\necho %s\\n' \"$(GREETING)\" > conf && chmod +x conf\ninstall: all\n\tmkdir -p $(PREFIX)/bin && cp conf $(PREFIX)/bin/conf\n",
            ),
        ],
    );
    project.define("conf", r#"program conf { version("1.0"); source(local = "src/conf"); autotools(env = ["GREETING=hello from autotools"]); set_bin("bin/conf"); }"#);

    let mut expected = vec![("app", "hello from make"), ("conf", "hello from autotools")];
    if have("cargo") {
        write_files(
            &src.join("rusty"),
            &[
                (
                    "Cargo.toml",
                    "[package]\nname = \"rusty\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
                ),
                (
                    "src/main.rs",
                    "fn main() { println!(\"hello from cargo\"); }\n",
                ),
            ],
        );
        project.define("rusty", r#"program rusty { version("0.1.0"); source(local = "src/rusty"); cargo(env = ["CARGO_NET_OFFLINE=true"]); set_bin("bin/rusty"); }"#);
        expected.push(("rusty", "hello from cargo"));
    }
    if have("go") {
        write_files(
            &src.join("gotool"),
            &[
                ("go.mod", "module example.com/gotool\n\ngo 1.21\n"),
                (
                    "main.go",
                    "package main\n\nimport \"fmt\"\n\nfunc main() { fmt.Println(\"hello from go\") }\n",
                ),
            ],
        );
        project.define("gotool", r#"program gotool { version("1.0"); source(local = "src/gotool"); go(env = ["GOTOOLCHAIN=local", "GOPROXY=off"]); set_bin("bin/gotool"); }"#);
        expected.push(("gotool", "hello from go"));
    }
    let requests: Vec<_> = expected
        .iter()
        .map(|(name, _)| format!("{name} = \"latest\""))
        .collect();
    project.write_env(&requests.join("\n"));

    let (result, output) = project.sync(true, "");
    assert_eq!(result, Ok(Outcome::Synced), "{output}");
    for (name, greeting) in expected {
        let out = std::process::Command::new(project.home().join("bin").join(name))
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8(out.stdout).unwrap().trim(),
            greeting,
            "{name}"
        );
    }
    // The build log shows each generated command before its output.
    let log = std::fs::read_to_string(project.home().join("packages/app/1.0/build.log")).unwrap();
    assert!(log.starts_with("$ cmake -S . -B build \"-DCMAKE_INSTALL_PREFIX=$NUX_PREFIX\" -DCMAKE_BUILD_TYPE=Debug\n"), "{log}");
    assert!(log.contains("\n$ cmake --install build\n"), "{log}");
}

impl Project {
    fn io(&self, out: &Buffer, input: &'static str, config: Config) -> Io<Buffer, &'static [u8]> {
        Io {
            term: Term::PLAIN,
            out: out.clone(),
            input: input.as_bytes(),
            workspace: Workspace::new(self.home()),
            path: Some(self.home().join("bin").into_os_string()),
            host: Host::empty(),
            config,
        }
    }

    /// `nux install <packages> -e <project>`.
    fn install(&self, packages: &[&str], input: &'static str) -> (Result<Outcome, String>, String) {
        self.install_in(at(&self.dir), Config::default(), packages, input)
    }

    fn install_in(
        &self,
        environment: Selection,
        config: Config,
        packages: &[&str],
        input: &'static str,
    ) -> (Result<Outcome, String>, String) {
        let packages = packages
            .iter()
            .map(|p| match p.split_once('@') {
                Some((name, requirement)) => (name.to_string(), Some(requirement.to_string())),
                None => (p.to_string(), None),
            })
            .collect();
        let out = Buffer::default();
        let options = install::Options {
            packages,
            environment,
            yes: input.is_empty(),
            wait: false,
        };
        let result = install::run(&options, self.io(&out, input, config));
        (
            result,
            mask_durations(&String::from_utf8(out.0.lock().unwrap().clone()).unwrap()),
        )
    }

    /// `nux remove <packages> -e <project> -y`.
    fn remove(&self, packages: &[&str]) -> (Result<Outcome, String>, String) {
        let out = Buffer::default();
        let packages = packages.iter().map(|p| p.to_string()).collect();
        let options = remove::Options {
            packages,
            environment: at(&self.dir),
            yes: true,
            wait: false,
        };
        let result = remove::run(&options, self.io(&out, "", Config::default()));
        (
            result,
            mask_durations(&String::from_utf8(out.0.lock().unwrap().clone()).unwrap()),
        )
    }

    fn purge(
        &self,
        options: purge_command::Options,
    ) -> (Result<purge_command::Outcome, String>, String) {
        let mut out = Vec::new();
        let result = purge_command::run(
            options,
            &mut out,
            &mut "".as_bytes(),
            Term::PLAIN,
            &Workspace::new(self.home()),
        );
        (result, String::from_utf8(out).unwrap())
    }

    fn manifest(&self) -> String {
        std::fs::read_to_string(self.dir.join("Nux.toml")).unwrap()
    }
}

#[test]
fn install_requests_packages_and_syncs() {
    let project = Project::new(
        "install",
        "a = \"1\"  # keep this comment",
        &[
            ("a", &tool("a", "1", "")),
            ("b", &tool("b", "1", "add_dependency(lib);")),
            ("lib", r#"program lib { version("1"); bash { true } }"#),
        ],
    );
    project.sync(true, "").0.unwrap();

    // Declining, or a package that does not resolve, leaves Nux.toml alone.
    let before = project.manifest();
    assert_eq!(project.install(&["b"], "n\n").0, Ok(Outcome::Cancelled));
    assert_eq!(project.manifest(), before);
    let err = project.install(&["ghost"], "").0.unwrap_err();
    assert!(err.starts_with("package `ghost` is not defined"), "{err}");
    assert_eq!(project.manifest(), before);

    let (result, output) = project.install(&["b"], "");
    assert_eq!(result, Ok(Outcome::Synced), "{output}");
    assert!(
        output.starts_with(&format!(
            "Nux v0.1.0\n\nEnvironment: test\n\nRequesting in `{}`:\n  b latest\n\nResolving dependencies...\n",
            project.dir.join("Nux.toml").display()
        )),
        "{output}"
    );
    assert!(
        output.contains(&format!(
            "\n  Updated `{}`.\n",
            project.dir.join("Nux.toml").display()
        )),
        "{output}"
    );
    assert_eq!(
        project.manifest(),
        before.replace(
            "# keep this comment\n",
            "# keep this comment\nb = \"latest\"\n"
        )
    );
    assert_eq!(
        std::process::Command::new(project.home().join("bin/b"))
            .output()
            .unwrap()
            .stdout,
        b"b 1\n"
    );

    // Already requested: nothing to write, nothing to do.
    let (result, output) = project.install(&["b"], "");
    assert_eq!(result, Ok(Outcome::UpToDate));
    assert!(output.contains("Already requested in `"), "{output}");
    assert!(
        output.contains("\n  b latest (already requested)\n"),
        "{output}"
    );

    // A new requirement replaces the old one.
    let (result, output) = project.install(&["a@^1"], "");
    assert_eq!(result, Ok(Outcome::UpToDate), "{output}");
    assert!(output.contains("\n  a 1 -> ^1\n"), "{output}");
    assert!(
        project
            .manifest()
            .contains("a = \"^1\"  # keep this comment\n"),
        "{}",
        project.manifest()
    );
}

#[test]
fn remove_unlinks_and_purge_removes_what_is_unused() {
    use purge_command::{Options as PurgeOptions, Outcome as Purged};
    let project = Project::new(
        "remove",
        "a = \"1\"\nb = \"1\"",
        &[
            ("a", &tool("a", "1", "add_dependency(shared);")),
            (
                "b",
                &tool("b", "1", "add_dependency(lib); add_dependency(shared);"),
            ),
            ("lib", r#"program lib { version("1"); bash { true } }"#),
            (
                "shared",
                r#"program shared { version("1"); bash { true } }"#,
            ),
        ],
    );
    project.sync(true, "").0.unwrap();
    assert_eq!(
        project.purge(PurgeOptions::default()).0,
        Ok(Purged::NothingToPurge)
    );

    assert_eq!(
        project.remove(&["ghost"]).0.unwrap_err(),
        format!(
            "`ghost` is not requested by environment `test` ({})",
            project.dir.join("Nux.toml").display()
        )
    );
    let (result, output) = project.remove(&["b"]);
    assert_eq!(result, Ok(Outcome::Synced), "{output}");
    assert!(output.contains("Removing from `"), "{output}");
    assert!(output.contains("\n  Removed link: b\n"), "{output}");
    assert!(
        output.ends_with(
            "\nb 1, lib 1 are no longer used by any environment; run `nux purge` to remove them.\n"
        ),
        "{output}"
    );
    assert!(!project.manifest().contains("b = "));
    assert!(project.home().join("bin/b").symlink_metadata().is_err());
    assert!(
        project.home().join("packages/b/1/install").is_dir(),
        "remove keeps installations"
    );
    assert_eq!(status(&project).0, Ok(status_command::Outcome::UpToDate));

    let (result, output) = project.purge(PurgeOptions {
        dry_run: true,
        ..Default::default()
    });
    assert_eq!(result, Ok(Purged::DryRun));
    assert_eq!(
        listed_items(&output),
        ["  unused package b 1", "  unused package lib 1"],
        "{output}"
    );
    assert!(
        output.contains("Keeping what these environments use: test\n"),
        "{output}"
    );
    let (result, output) = project.purge(PurgeOptions {
        yes: true,
        ..Default::default()
    });
    assert_eq!(result, Ok(Purged::Purged), "{output}");
    assert!(
        !project.home().join("packages/b").exists()
            && !project.home().join("packages/lib").exists()
    );
    assert!(project.home().join("packages/shared/1/install").is_dir());
    assert_eq!(project.sync(true, "").1, UP_TO_DATE);

    // Editing Nux.toml by hand: status and sync notice the link left behind.
    project.write_env("");
    let (result, output) = status(&project);
    assert_eq!(result, Ok(status_command::Outcome::OutOfDate));
    assert!(
        output.contains("  a -> a 1 to remove, no longer used\n"),
        "{output}"
    );
    assert!(
        output.contains(
            "\nUnused:      a 1, shared 1 (after `nux sync`, `nux purge` removes them)\n"
        ),
        "{output}"
    );
    assert!(output.contains("1 link to remove"), "{output}");
    let (result, output) = project.sync(true, "");
    assert_eq!(result, Ok(Outcome::Synced), "{output}");
    assert!(project.home().join("bin/a").symlink_metadata().is_err());
}

#[test]
fn purge_keeps_what_other_environments_use_and_forgets_deleted_ones() {
    use purge_command::{Options as PurgeOptions, Outcome as Purged};
    let definitions = [("a", tool("a", "1", "")), ("b", tool("b", "1", ""))];
    let definitions: Vec<_> = definitions.iter().map(|(n, d)| (*n, d.as_str())).collect();
    let first = Project::new("purge-first", "a = \"1\"\nb = \"1\"", &definitions);
    first.sync(true, "").0.unwrap();
    // A second environment, sharing the first one's workspace.
    let second = Project::new("purge-second", "b = \"1\"", &definitions);
    std::fs::write(
        second.dir.join("Nux.toml"),
        second.manifest().replace("\"test\"", "\"second\""),
    )
    .unwrap();
    let home = first.home();
    let io = Io {
        workspace: Workspace::new(&home),
        ..second.io(&Buffer::default(), "", Config::default())
    };
    run(
        &Options {
            environment: at(&second.dir),
            yes: true,
            ..Options::default()
        },
        io,
    )
    .unwrap();

    // `b` leaves the first environment, but the second still uses it.
    first.remove(&["b"]).0.unwrap();
    assert!(first.home().join("bin/b").symlink_metadata().is_ok());
    assert_eq!(
        first.purge(PurgeOptions::default()).0,
        Ok(Purged::NothingToPurge)
    );

    // Once the second environment is deleted, its record is stale.
    std::fs::remove_dir_all(&second.dir).unwrap();
    let (result, output) = first.purge(PurgeOptions {
        yes: true,
        ..Default::default()
    });
    assert_eq!(result, Ok(Purged::Purged), "{output}");
    assert_eq!(
        listed_items(&output),
        [
            "  executable link bin/b",
            "  unused package  b 1",
            "  stale record    state/environments/second.json"
        ],
        "{output}"
    );
    assert!(!home.join("packages/b").exists() && !home.join("bin/b").exists());
    assert!(!home.join("state/environments/second.json").exists());
    assert!(home.join("packages/a/1/install").is_dir());
}

#[test]
fn the_global_environment_is_created_selected_and_used_as_a_fallback() {
    let project = Project::new(
        "global",
        "",
        &[("a", &tool("a", "1", "")), ("b", &tool("b", "1", ""))],
    );
    std::fs::remove_file(project.dir.join("Nux.toml")).unwrap();
    let config_dir = project.dir.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let global = Selection {
        path: None,
        global: true,
    };

    // Without repositories, there is nothing to install from.
    let err = project
        .install_in(
            global.clone(),
            Config::load(&config_dir).unwrap(),
            &["a"],
            "",
        )
        .0
        .unwrap_err();
    assert!(
        err.contains("no repositories configured; add a `[[repository]]` with a `path` to `"),
        "{err}"
    );

    std::fs::write(
        config_dir.join("config.toml"),
        "[[repository]]\npath = \"../repository\"\n",
    )
    .unwrap();
    let config = Config::load(&config_dir).unwrap();
    let (result, output) = project.install_in(global.clone(), config.clone(), &["a"], "");
    assert_eq!(result, Ok(Outcome::Synced), "{output}");
    let manifest = config_dir.join("Nux.toml");
    assert!(
        output.contains(&format!(
            "Environment: global (global)\n\nCreating `{}`, requesting:\n  a latest\n",
            manifest.display()
        )),
        "{output}"
    );
    let env = crate::environment::Environment::load_with(&manifest, true, &config).unwrap();
    assert_eq!(env.packages, [nux_resolver::Request::new("a", "latest")]);
    assert_eq!(env.repositories, [config_dir.join("../repository")]);
    assert!(config_dir.join("Nux.lock").is_file());

    // An environment's own repositories come first, then the configured ones.
    std::fs::create_dir_all(project.dir.join("local")).unwrap();
    let own = "[packages]\nb = \"1\"\n\n[[repository]]\npath = \"local\"\n";
    let env = crate::environment::Environment::from_text(
        own,
        &project.dir.join("Nux.toml"),
        false,
        &config,
    )
    .unwrap();
    assert_eq!(
        env.repositories,
        [project.dir.join("local"), config_dir.join("../repository")]
    );

    // `-e` wins when it exists; without `-e` and `./Nux.toml`, the global one is used.
    std::fs::write(project.dir.join("Nux.toml"), own).unwrap();
    assert_eq!(
        at(&project.dir).locate(&config),
        Ok((project.dir.join("Nux.toml"), false))
    );
    assert_eq!(global.locate(&config), Ok((manifest.clone(), true)));
    assert!(
        !std::path::Path::new("Nux.toml").exists(),
        "tests run outside of an environment"
    );
    assert_eq!(
        Selection::default().locate(&config),
        Ok((manifest.clone(), true))
    );
    assert_eq!(
        Selection::default().locate(&Config::default()),
        Ok((PathBuf::from("Nux.toml"), false))
    );
    let err = crate::environment::Environment::select(&Selection::default(), &Config::default())
        .unwrap_err();
    assert!(
        err.starts_with("no `Nux.toml` in the current directory and no global environment"),
        "{err}"
    );

    let mut out = Vec::new();
    let result = status_command::run(
        &global,
        &config,
        &mut out,
        Term::PLAIN,
        &Workspace::new(project.home()),
        &Host::empty(),
    );
    let out = String::from_utf8(out).unwrap();
    assert_eq!(result, Ok(status_command::Outcome::UpToDate), "{out}");
    assert!(
        out.contains(&format!(
            "Environment: global ({}, global)\n",
            manifest.display()
        )),
        "{out}"
    );
}

#[test]
fn builds_run_in_a_sandbox_unless_it_is_off() {
    if nux_executor::Sandbox::available().is_err() {
        return;
    }
    let project = Project::new("sandbox", "probe = \"1\"", &[]);
    let outside = project.dir.join("outside");
    // Writes its results where it may, and tries to write outside.
    project.define(
        "probe",
        &format!(
            r#"program probe {{
    version("1");
    bash {{
        mkdir -p $NUX_PREFIX/bin
        echo "pid $$" > $NUX_PREFIX/report
        echo escaped > {outside}
        (echo cache > $HOME/.probe-cache && echo "home writable" >> $NUX_PREFIX/report) 2>/dev/null
        (touch $NUX_HOME/state/x) 2>/dev/null || echo "workspace read-only" >> $NUX_PREFIX/report
    }}
}}"#,
            outside = outside.display()
        ),
    );
    project.sync(true, "").0.unwrap();
    let report =
        std::fs::read_to_string(project.home().join("packages/probe/1/install/report")).unwrap();
    assert_eq!(report, "pid 2\nhome writable\nworkspace read-only\n");
    // It wrote to the sandbox's own `/tmp`, which is thrown away.
    assert!(!outside.exists());

    // With `[sandbox] mode = "off"`, the same build reaches the host.
    project.define(
        "probe",
        &std::fs::read_to_string(project.dir.join("repository/probe.nux"))
            .unwrap()
            .replace("version(\"1\");", "version(\"1\"); bash { true }"),
    );
    let config_dir = project.dir.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "[sandbox]\nmode = \"off\"\n",
    )
    .unwrap();
    let out = Buffer::default();
    let io = project.io(&out, "", Config::load(&config_dir).unwrap());
    run(
        &Options {
            environment: at(&project.dir),
            yes: true,
            ..Options::default()
        },
        io,
    )
    .unwrap();
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "escaped\n");
}

#[test]
fn locked_sources_are_rebuilt_from_the_cache_without_the_network() {
    use clean_command::{Options as CleanOptions, Outcome as Cleaned};
    let project = Project::new("download-cache", "app = \"1\"\ndata = \"1\"", &[]);
    let repo = project.dir.join("upstream");
    let first = git_repo(&repo, "one");
    git_repo(&repo, "two");
    // The lock pins the first commit, which is no longer the tip: a mirror
    // must fetch exactly it.
    let archive = project.dir.join("data.txt");
    std::fs::write(&archive, "payload").unwrap();
    project.define("data", &format!(
        r#"program data {{ version("1"); source(url = "file://{}"); bash {{ cp $NUX_SOURCE_PATH $NUX_PREFIX/ }} }}"#,
        archive.display()
    ));
    project.define("app", &format!(
        r#"program app {{ version("1"); source(git = "file://{}"); bash {{ cp file.txt $NUX_PREFIX/ }} }}"#,
        repo.display()
    ));
    project.sync(true, "").0.unwrap();
    let lock = std::fs::read_to_string(project.dir.join("Nux.lock")).unwrap();
    let tip = lock
        .split("revision = \"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string();
    std::fs::write(project.dir.join("Nux.lock"), lock.replace(&tip, &first)).unwrap();
    let (result, output) = project.sync(true, "");
    assert_eq!(result, Ok(Outcome::Synced), "{output}");
    let installed =
        || std::fs::read_to_string(project.home().join("packages/app/1/install/file.txt")).unwrap();
    assert_eq!(installed(), "one");

    // Everything installed goes, and so do the upstream repository and file.
    for path in [
        project.home().join("packages"),
        project.home().join("bin"),
        repo.clone(),
    ] {
        std::fs::remove_dir_all(path).unwrap();
    }
    std::fs::remove_file(&archive).unwrap();
    let (result, output) = project.sync(true, "");
    assert_eq!(result, Ok(Outcome::Synced), "{output}");
    assert!(
        output.contains("  Downloading app  From the cache\n")
            && output.contains("  Downloading data From the cache\n"),
        "{output}"
    );
    assert!(!output.contains("Finished downloading"), "{output}");
    assert_eq!(installed(), "one");
    assert_eq!(
        std::fs::read_to_string(project.home().join("packages/data/1/install/data.txt")).unwrap(),
        "payload"
    );

    // A damaged file in the cache is not used, and removed.
    let sha = lock
        .split("checksum = \"sha256:")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let cached = project.home().join("cache/downloads/sha256").join(sha);
    std::fs::remove_dir_all(project.home().join("packages/data")).unwrap();
    std::fs::write(&cached, "tampered").unwrap();
    let err = project.sync(true, "").0.unwrap_err();
    assert!(err.starts_with("failed to download data 1:"), "{err}");
    assert!(!cached.exists());

    // `clean --cache` empties it.
    let mut out = Vec::new();
    let options = CleanOptions {
        yes: true,
        cache: true,
        ..Default::default()
    };
    let result = clean_command::run(
        options,
        &mut out,
        &mut "".as_bytes(),
        Term::PLAIN,
        &Workspace::new(project.home()),
    );
    let out = String::from_utf8(out).unwrap();
    assert_eq!(result, Ok(Cleaned::Cleaned), "{out}");
    assert!(
        listed_items(&out).contains(&"  download cache cache/git".to_string()),
        "{out}"
    );
    assert!(
        !project.home().join("cache/git").exists()
            && !project.home().join("cache/downloads").exists()
    );
}

#[test]
fn builds_can_share_tool_caches() {
    if nux_executor::Sandbox::available().is_err() {
        return;
    }
    let project = Project::new("shared-caches", "probe = \"1\"", &[]);
    project.define(
        "probe",
        r#"program probe { version("1"); bash {
        mkdir -p $NUX_PREFIX
        echo "${CARGO_HOME:-unset} ${GOMODCACHE:-unset}" > $NUX_PREFIX/report
        (touch $CARGO_HOME/written) 2>/dev/null || true
    } }"#,
    );
    let config_dir = project.dir.join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let report =
        || std::fs::read_to_string(project.home().join("packages/probe/1/install/report")).unwrap();

    // By default, builds keep the environment's settings (and start from
    // empty caches in their sandbox).
    project.sync(true, "").0.unwrap();
    assert!(!report().contains("/cache/"), "{}", report());

    std::fs::write(
        config_dir.join("config.toml"),
        "[sandbox]\nshared_caches = true\n",
    )
    .unwrap();
    // Changed, so that it is built again.
    project.define(
        "probe",
        &std::fs::read_to_string(project.dir.join("repository/probe.nux"))
            .unwrap()
            .replace("mkdir", "true; mkdir"),
    );
    let out = Buffer::default();
    run(
        &Options {
            environment: at(&project.dir),
            yes: true,
            ..Options::default()
        },
        project.io(&out, "", Config::load(&config_dir).unwrap()),
    )
    .unwrap();
    let cache = project.home().join("cache");
    assert_eq!(
        report(),
        format!(
            "{} {}\n",
            cache.join("cargo").display(),
            cache.join("go/mod").display()
        )
    );
    assert!(
        cache.join("cargo/written").is_file(),
        "the shared cache is writable"
    );
}

fn reset(
    home: &Path,
    options: reset_command::Options,
    input: &str,
) -> (Result<reset_command::Outcome, String>, String) {
    let mut out = Vec::new();
    let result = reset_command::run(
        options,
        &mut out,
        &mut input.as_bytes(),
        Term::PLAIN,
        &Workspace::new(home),
    );
    (result, String::from_utf8(out).unwrap())
}

#[test]
fn reset_removes_what_nux_created_and_nothing_else() {
    use reset_command::{Options as ResetOptions, Outcome as Reset};
    let archive =
        std::env::temp_dir().join(format!("nux-sync-tests-{}-reset.txt", std::process::id()));
    std::fs::write(&archive, "payload").unwrap();
    let project = Project::new(
        "reset",
        "app = \"1\"",
        &[(
            "app",
            &tool(
                "app",
                "1",
                &format!(r#"source(url = "file://{}");"#, archive.display()),
            ),
        )],
    );
    let home = project.home();
    assert_eq!(
        reset(&home, ResetOptions::default(), "").0,
        Ok(Reset::NothingToReset)
    );
    project.sync(true, "").0.unwrap();
    // Files Nux did not create, which must survive.
    std::fs::write(home.join("notes.txt"), "mine").unwrap();
    std::fs::write(home.join("bin/my-script"), "mine").unwrap();
    // A read-only directory, as builds and Go leave behind.
    let readonly = home.join("packages/app/1/install/readonly");
    std::fs::create_dir_all(&readonly).unwrap();
    std::fs::set_permissions(
        &readonly,
        std::os::unix::fs::PermissionsExt::from_mode(0o555),
    )
    .unwrap();

    let (result, output) = reset(
        &home,
        ResetOptions {
            dry_run: true,
            ..Default::default()
        },
        "",
    );
    assert_eq!(result, Ok(Reset::DryRun));
    assert_eq!(
        listed_items(&output),
        [
            "  executable links    bin",
            "  packages            packages",
            "  environment records state",
            "  download cache      cache"
        ],
        "{output}"
    );
    assert!(
        output.contains("\nKept, since Nux did not create them:\n  bin/my-script\n  notes.txt\n"),
        "{output}"
    );
    assert_eq!(
        reset(&home, ResetOptions::default(), "n\n").0,
        Ok(Reset::Cancelled)
    );
    assert!(home.join("bin/app").symlink_metadata().is_ok());

    // Keeping the cache: the rebuild needs no download.
    let (result, output) = reset(
        &home,
        ResetOptions {
            yes: true,
            keep_cache: true,
            ..Default::default()
        },
        "",
    );
    assert_eq!(result, Ok(Reset::Reset), "{output}");
    for gone in ["packages", "state", "bin/app"] {
        assert!(home.join(gone).symlink_metadata().is_err(), "{gone}");
    }
    for kept in ["notes.txt", "bin/my-script", "cache/downloads"] {
        assert!(home.join(kept).exists(), "{kept}");
    }
    assert_eq!(status(&project).0, Ok(status_command::Outcome::OutOfDate));
    std::fs::remove_file(&archive).unwrap();
    let (result, output) = project.sync(true, "");
    assert_eq!(result, Ok(Outcome::Synced), "{output}");
    assert!(output.contains("From the cache"), "{output}");

    // Without `--keep-cache`, the cache goes too.
    reset(
        &home,
        ResetOptions {
            yes: true,
            ..Default::default()
        },
        "",
    )
    .0
    .unwrap();
    assert!(!home.join("cache").exists() && home.join("notes.txt").exists());
    assert_eq!(
        reset(&home, ResetOptions::default(), "").0,
        Ok(Reset::NothingToReset)
    );
}

#[test]
fn reset_leaves_a_directory_that_is_not_a_workspace_alone() {
    let dir = std::env::temp_dir()
        .join(format!("nux-sync-tests-{}", std::process::id()))
        .join("not-a-workspace");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    std::fs::write(dir.join("bin/tool"), "mine").unwrap();
    std::os::unix::fs::symlink("/usr/bin/env", dir.join("bin/env")).unwrap();
    std::fs::write(dir.join("document.txt"), "mine").unwrap();
    let (result, output) = reset(
        &dir,
        reset_command::Options {
            yes: true,
            ..Default::default()
        },
        "",
    );
    assert_eq!(
        result,
        Ok(reset_command::Outcome::NothingToReset),
        "{output}"
    );
    assert!(
        output.contains(
            "Kept, since Nux did not create them:\n  bin/env\n  bin/tool\n  document.txt\n"
        ),
        "{output}"
    );
    assert!(
        dir.join("bin/tool").exists()
            && dir.join("bin/env").symlink_metadata().is_ok()
            && dir.join("document.txt").exists()
    );
}

#[test]
fn renaming_a_link_replaces_the_old_one() {
    let project = Project::new(
        "rename-link",
        "tool = \"1\"",
        &[("tool", &tool("tool", "1", ""))],
    );
    project.sync(true, "").0.unwrap();
    assert!(project.home().join("bin/tool").symlink_metadata().is_ok());

    // The recipe now names the link; nothing needs building.
    project.define(
        "tool",
        &tool("tool", "1", "").replace(
            r#"set_bin("bin/tool");"#,
            r#"set_bin("bin/tool", name = "tool1");"#,
        ),
    );
    let (result, output) = status(&project);
    assert_eq!(result, Ok(status_command::Outcome::OutOfDate), "{output}");
    assert!(
        output.contains("Executable links:\n  tool  -> tool 1 to remove, no longer used\n  tool1 -> tool 1 not linked\n"),
        "{output}"
    );
    let (result, output) = project.sync(true, "");
    assert_eq!(result, Ok(Outcome::Synced), "{output}");
    assert!(
        !output.contains("Building tool... Done"),
        "no rebuild: {output}"
    );
    assert!(output.contains("  Removed link: tool\n"), "{output}");
    let run = std::process::Command::new(project.home().join("bin/tool1"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(run.stdout).unwrap(), "tool 1\n");
    assert!(project.home().join("bin/tool").symlink_metadata().is_err());
    assert_eq!(project.sync(true, "").1, UP_TO_DATE);
}
