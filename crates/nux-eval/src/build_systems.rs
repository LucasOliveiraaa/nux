//! Commands for common build systems: what `cmake(...)`, `meson(...)`,
//! `make(...)`, `autotools(...)`, `cargo(...)` and `go(...)` add to a version.
//!
//! The commands are ordinary build commands, as if written in a `bash`
//! block: they run in the build directory with the dependency environment
//! (`CMAKE_PREFIX_PATH`, `PKG_CONFIG_PATH`, `LDFLAGS`...) and log to
//! `build.log`.

/// Options every helper accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Common {
    /// Extra arguments passed to the configure (or only) step.
    pub options: Vec<String>,
    /// The source directory, relative to the build directory.
    pub dir: String,
    /// Where to install.
    pub prefix: String,
    /// Parallel jobs: a number, or `$NUX_JOBS`.
    pub jobs: String,
    /// `NAME=value` assignments for every command.
    pub env: Vec<(String, String)>,
}

impl Default for Common {
    fn default() -> Self {
        Self {
            options: Vec::new(),
            dir: ".".into(),
            prefix: "$NUX_PREFIX".into(),
            jobs: "$NUX_JOBS".into(),
            env: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BuildSystem {
    Cmake { build_type: String, build_dir: String, generator: Option<String> },
    Meson { build_type: String, build_dir: String },
    Make { targets: Vec<String>, install: Option<String>, prefix_var: String },
    Autotools { reconf: bool },
    Cargo { features: Vec<String>, locked: bool },
    Go { packages: Vec<String>, ldflags: Option<String> },
}

impl BuildSystem {
    pub fn commands(&self, common: &Common) -> Vec<String> {
        let Common { options, dir, prefix, jobs, env } = common;
        let (q, qs) = (quote, |items: &[String]| items.iter().map(|i| quote(i)).collect::<Vec<_>>());
        match self {
            BuildSystem::Cmake { build_type, build_dir, generator } => {
                let mut configure = vec!["cmake".into(), "-S".into(), q(dir), "-B".into(), q(build_dir)];
                if let Some(generator) = generator {
                    configure.extend(["-G".into(), q(generator)]);
                }
                configure.push(q(&format!("-DCMAKE_INSTALL_PREFIX={prefix}")));
                configure.push(q(&format!("-DCMAKE_BUILD_TYPE={build_type}")));
                configure.extend(qs(options));
                vec![
                    line(env, None, configure),
                    line(env, None, vec!["cmake".into(), "--build".into(), q(build_dir), "--parallel".into(), jobs.clone()]),
                    line(env, None, vec!["cmake".into(), "--install".into(), q(build_dir)]),
                ]
            }
            BuildSystem::Meson { build_type, build_dir } => {
                let mut setup = vec!["meson".into(), "setup".into(), q(build_dir), q(dir)];
                setup.push(q(&format!("--prefix={prefix}")));
                setup.push(q(&format!("--buildtype={build_type}")));
                setup.extend(qs(options));
                vec![
                    line(env, None, setup),
                    line(env, None, vec!["meson".into(), "compile".into(), "-C".into(), q(build_dir), "-j".into(), jobs.clone()]),
                    line(env, None, vec!["meson".into(), "install".into(), "-C".into(), q(build_dir)]),
                ]
            }
            BuildSystem::Make { targets, install, prefix_var } => {
                let base = |extra: Vec<String>| {
                    let mut words = vec!["make".into(), "-C".into(), q(dir)];
                    words.extend(extra);
                    words.push(q(&format!("{prefix_var}={prefix}")));
                    words.extend(qs(options));
                    words
                };
                let mut build = base(vec!["-j".into(), jobs.clone()]);
                build.extend(qs(targets));
                let mut commands = vec![line(env, None, build)];
                if let Some(install) = install {
                    let mut words = base(Vec::new());
                    words.push(q(install));
                    commands.push(line(env, None, words));
                }
                commands
            }
            BuildSystem::Autotools { reconf } => {
                let mut commands = Vec::new();
                if *reconf {
                    commands.push(line(env, Some(dir), vec!["autoreconf".into(), "-fi".into()]));
                }
                let mut configure = vec!["./configure".into(), q(&format!("--prefix={prefix}"))];
                configure.extend(qs(options));
                commands.push(line(env, Some(dir), configure));
                commands.push(line(env, None, vec!["make".into(), "-C".into(), q(dir), "-j".into(), jobs.clone()]));
                commands.push(line(env, None, vec!["make".into(), "-C".into(), q(dir), "install".into()]));
                commands
            }
            BuildSystem::Cargo { features, locked } => {
                // Keep build artifacts in the build directory, not the source.
                let mut env = vec![("CARGO_TARGET_DIR".to_string(), "$NUX_BUILD/target".to_string())];
                env.extend(common.env.iter().cloned());
                let mut words: Vec<String> = vec!["cargo".into(), "install".into(), "--path".into(), q(dir)];
                words.extend(["--root".into(), q(prefix), "--jobs".into(), jobs.clone()]);
                if *locked {
                    words.push("--locked".into());
                }
                if !features.is_empty() {
                    words.extend(["--features".into(), q(&features.join(","))]);
                }
                words.extend(qs(options));
                vec![line(&env, None, words)]
            }
            BuildSystem::Go { packages, ldflags } => {
                // Module and build caches stay in the build directory.
                let mut env = vec![
                    ("GOBIN".to_string(), format!("{prefix}/bin")),
                    ("GOPATH".to_string(), "$NUX_BUILD/.go".to_string()),
                    ("GOCACHE".to_string(), "$NUX_BUILD/.cache/go-build".to_string()),
                ];
                env.extend(common.env.iter().cloned());
                let mut words: Vec<String> = vec!["go".into(), "install".into(), "-p".into(), jobs.clone()];
                if let Some(ldflags) = ldflags {
                    words.extend(["-ldflags".into(), q(ldflags)]);
                }
                words.extend(qs(options));
                words.extend(qs(packages));
                vec![line(&env, Some(dir), words)]
            }
        }
    }
}

/// `cd <dir> && NAME=value ... <words>`.
fn line(env: &[(String, String)], dir: Option<&str>, words: Vec<String>) -> String {
    let mut parts = Vec::new();
    if let Some(dir) = dir.filter(|d| *d != ".") {
        parts.push(format!("cd {} &&", quote(dir)));
    }
    parts.extend(env.iter().map(|(name, value)| format!("{name}={}", quote(value))));
    parts.extend(words);
    parts.join(" ")
}

/// Quotes `word` for `sh` so it stays one word. Plain words are kept as they
/// are; anything else is double-quoted, which keeps `$NUX_PREFIX` and other
/// variables expanding.
pub(crate) fn quote(word: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "_./:=+,@%-".contains(c);
    if !word.is_empty() && word.chars().all(plain) {
        return word.to_string();
    }
    let mut quoted = String::from("\"");
    for c in word.chars() {
        if matches!(c, '"' | '\\' | '`') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

/// Splits `NAME=value`, checking that `NAME` is a valid variable name.
pub(crate) fn assignment(text: &str) -> Option<(String, String)> {
    let (name, value) = text.split_once('=')?;
    let valid = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid.then(|| (name.to_string(), value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn quoting() {
        assert_eq!(quote("-DFOO=ON"), "-DFOO=ON");
        assert_eq!(quote("$NUX_PREFIX"), r#""$NUX_PREFIX""#);
        assert_eq!(quote("a b"), r#""a b""#);
        assert_eq!(quote(r#"say "hi" `x` \"#), r#""say \"hi\" \`x\` \\""#);
        assert_eq!(quote(""), r#""""#);
        assert_eq!(assignment("CFLAGS=-O2 -g"), Some(("CFLAGS".into(), "-O2 -g".into())));
        assert_eq!(assignment("1X=a"), None);
        assert_eq!(assignment("novalue"), None);
    }

    #[test]
    fn cmake_and_meson() {
        let common = Common { options: strings(&["-DBUILD_TESTING=OFF", "-DNAME=a b"]), ..Common::default() };
        let cmake = BuildSystem::Cmake { build_type: "Release".into(), build_dir: "build".into(), generator: Some("Ninja".into()) };
        assert_eq!(
            cmake.commands(&common),
            [
                r#"cmake -S . -B build -G Ninja "-DCMAKE_INSTALL_PREFIX=$NUX_PREFIX" -DCMAKE_BUILD_TYPE=Release -DBUILD_TESTING=OFF "-DNAME=a b""#,
                "cmake --build build --parallel $NUX_JOBS",
                "cmake --install build",
            ]
        );
        let meson = BuildSystem::Meson { build_type: "debug".into(), build_dir: "out".into() };
        let common = Common { dir: "src".into(), jobs: "2".into(), ..Common::default() };
        assert_eq!(
            meson.commands(&common),
            [
                r#"meson setup out src "--prefix=$NUX_PREFIX" --buildtype=debug"#,
                "meson compile -C out -j 2",
                "meson install -C out",
            ]
        );
    }

    #[test]
    fn make_and_autotools() {
        let make = BuildSystem::Make { targets: strings(&["all"]), install: Some("install".into()), prefix_var: "PREFIX".into() };
        let common = Common { env: vec![("CFLAGS".into(), "-O2 -g".into())], options: strings(&["V=1"]), ..Common::default() };
        assert_eq!(
            make.commands(&common),
            [
                r#"CFLAGS="-O2 -g" make -C . -j $NUX_JOBS "PREFIX=$NUX_PREFIX" V=1 all"#,
                r#"CFLAGS="-O2 -g" make -C . "PREFIX=$NUX_PREFIX" V=1 install"#,
            ]
        );
        let no_install = BuildSystem::Make { targets: vec![], install: None, prefix_var: "DESTDIR".into() };
        assert_eq!(no_install.commands(&Common::default()), [r#"make -C . -j $NUX_JOBS "DESTDIR=$NUX_PREFIX""#]);

        let autotools = BuildSystem::Autotools { reconf: true };
        let common = Common { dir: "src dir".into(), options: strings(&["--disable-nls"]), ..Common::default() };
        assert_eq!(
            autotools.commands(&common),
            [
                r#"cd "src dir" && autoreconf -fi"#,
                r#"cd "src dir" && ./configure "--prefix=$NUX_PREFIX" --disable-nls"#,
                r#"make -C "src dir" -j $NUX_JOBS"#,
                r#"make -C "src dir" install"#,
            ]
        );
    }

    #[test]
    fn cargo_and_go() {
        let cargo = BuildSystem::Cargo { features: strings(&["a", "b"]), locked: true };
        let common = Common { env: vec![("RUSTFLAGS".into(), "-C target-cpu=native".into())], ..Common::default() };
        assert_eq!(
            cargo.commands(&common),
            [r#"CARGO_TARGET_DIR="$NUX_BUILD/target" RUSTFLAGS="-C target-cpu=native" cargo install --path . --root "$NUX_PREFIX" --jobs $NUX_JOBS --locked --features a,b"#]
        );
        let go = BuildSystem::Go { packages: strings(&["./cmd/tool"]), ldflags: Some("-s -w".into()) };
        let common = Common { dir: "src".into(), ..Common::default() };
        assert_eq!(
            go.commands(&common),
            [r#"cd src && GOBIN="$NUX_PREFIX/bin" GOPATH="$NUX_BUILD/.go" GOCACHE="$NUX_BUILD/.cache/go-build" go install -p $NUX_JOBS -ldflags "-s -w" ./cmd/tool"#]
        );
    }
}
