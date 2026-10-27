//! `nux init`: create an environment that syncs out of the box.

use std::io::Write;
use std::path::{Path, PathBuf};

use nux_log::{Style, Term};

use crate::environment::{Environment, FILE_NAME};
use crate::repository;
use crate::ui::write_header;

/// A starter definition that needs no download and builds anywhere.
const HELLO: &str = r#"// A package definition: how to build `hello` and the executable it provides.
program hello {
    version("1.0.0");

    // Each line runs in its own shell; $NUX_PREFIX is the install directory.
    bash {
        mkdir -p $NUX_PREFIX/bin
        printf '#!/bin/sh\necho "Hello from Nux!"\n' > $NUX_PREFIX/bin/hello
        chmod +x $NUX_PREFIX/bin/hello
    }

    // Linked as $NUX_HOME/bin/hello after a successful build.
    set_bin("bin/hello");
}
"#;

fn manifest(name: &str) -> String {
    format!(
        r#"[environment]
name = {}

# Packages to build and the version to select: "latest", "1.0.0", "^1", ">=1, <2"...
[packages]
hello = "latest"

# Directories of .nux package definitions, searched in order.
[[repository]]
path = "./repository"
"#,
        toml::Value::String(name.to_string())
    )
}

/// Creates `Nux.toml` and a starter repository in `path` (default `.`).
/// Returns the created environment file.
pub fn run(path: Option<&Path>, out: &mut impl Write, term: Term) -> Result<PathBuf, String> {
    let w = |e: std::io::Error| format!("cannot write output: {e}");
    let dir = path.unwrap_or(Path::new("."));
    let manifest_path = dir.join(FILE_NAME);
    if manifest_path.exists() {
        return Err(format!(
            "`{}` already exists; not overwriting it",
            manifest_path.display()
        ));
    }
    std::fs::create_dir_all(dir.join("repository"))
        .map_err(|e| format!("cannot create `{}`: {e}", dir.join("repository").display()))?;

    let name = std::fs::canonicalize(dir)
        .ok()
        .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "default".into());

    let mut created = vec![FILE_NAME.to_string()];
    create_new(&manifest_path, &manifest(&name))?;
    // An existing repository is kept as it is.
    let hello = dir.join("repository").join("hello.nux");
    if !hello.exists() {
        create_new(&hello, HELLO)?;
        created.push("repository/hello.nux".into());
    }

    // What was written must load exactly as `nux sync` will load it.
    let env = Environment::load(&manifest_path)?;
    repository::load(
        &env.repositories,
        &nux_eval::Host::empty(),
        repository::DEFAULT_JOBS,
    )?;

    write_header(out, term).map_err(w)?;
    writeln!(
        out,
        "Created environment {} in `{}`:",
        term.paint(Style::Emphasis, &env.name),
        dir.display()
    )
    .map_err(w)?;
    for file in created {
        writeln!(out, "  {file}").map_err(w)?;
    }
    let sync = if path.is_none() {
        "nux sync".to_string()
    } else {
        format!("nux sync -e {}", dir.display())
    };
    writeln!(
        out,
        "\nTo build it, run:\n\n  {}",
        term.paint(Style::Emphasis, sync)
    )
    .map_err(w)?;
    Ok(manifest_path)
}

fn create_new(path: &Path, contents: &str) -> Result<(), String> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(contents.as_bytes()))
        .map_err(|e| format!("cannot create `{}`: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("nux-init-tests-{}", std::process::id()))
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn creates_a_valid_environment() {
        let dir = temp("my env");
        let mut out = Vec::new();
        let manifest = run(Some(&dir), &mut out, Term::PLAIN).unwrap();
        let env = Environment::load(&manifest).unwrap();
        assert_eq!(env.name, "my env");
        assert_eq!(
            env.packages,
            [nux_resolver::Request::new("hello", "latest")]
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!(
                "Nux v0.2.1\n\nCreated environment my env in `{0}`:\n  Nux.toml\n  repository/hello.nux\n\nTo build it, run:\n\n  nux sync -e {0}\n",
                dir.display()
            )
        );

        // Never overwrites an environment.
        std::fs::write(&manifest, "# mine").unwrap();
        let err = run(Some(&dir), &mut Vec::new(), Term::PLAIN).unwrap_err();
        assert_eq!(
            err,
            format!(
                "`{}` already exists; not overwriting it",
                manifest.display()
            )
        );
        assert_eq!(std::fs::read_to_string(&manifest).unwrap(), "# mine");
    }

    #[test]
    fn keeps_an_existing_repository() {
        let dir = temp("existing");
        std::fs::create_dir_all(dir.join("repository")).unwrap();
        std::fs::write(
            dir.join("repository/hello.nux"),
            r#"program hello { version("9"); }"#,
        )
        .unwrap();
        let mut out = Vec::new();
        run(Some(&dir), &mut out, Term::PLAIN).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains(":\n  Nux.toml\n\n"), "{out}");
        assert_eq!(
            std::fs::read_to_string(dir.join("repository/hello.nux")).unwrap(),
            r#"program hello { version("9"); }"#
        );
    }

    #[test]
    fn names_with_quotes_stay_valid_toml() {
        let dir = temp("it's \"quoted\"");
        let manifest = run(Some(&dir), &mut Vec::new(), Term::PLAIN).unwrap();
        assert_eq!(
            Environment::load(&manifest).unwrap().name,
            "it's \"quoted\""
        );
    }
}
