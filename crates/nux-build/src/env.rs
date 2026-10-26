//! What build commands can see of their dependencies.
//!
//! Every dependency, direct or not, contributes:
//! - its `set_bin` directory to `PATH` (links in `bin/` only exist once the
//!   whole sync succeeded);
//! - for each `export_lib`, its header directory to `CPATH` and `CPPFLAGS`,
//!   its library directory to `LIBRARY_PATH`, `LD_LIBRARY_PATH` and
//!   `LDFLAGS` (with an rpath, so installed executables find the library at
//!   run time), and its `pkgconfig` directories to `PKG_CONFIG_PATH`;
//! - `NUX_DEP_<NAME>`: its install prefix (`NUX_DEP_LIBSTDCXX` for
//!   `libstdc++`), and the prefix in `CMAKE_PREFIX_PATH`, so CMake's
//!   `find_package` finds it.
//!
//! Versions the host provides contribute nothing: the host's compilers and
//! libraries already find them.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use nux_resolver::PackageVersionId;

use crate::{BuildPlan, Step};

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct DependencyExports {
    pub bin: Vec<PathBuf>,
    pub include: Vec<PathBuf>,
    pub lib: Vec<PathBuf>,
    pub pkg_config: Vec<PathBuf>,
    pub prefixes: Vec<(String, PathBuf)>,
}

impl DependencyExports {
    /// Collects the exports of `step`'s dependencies: direct ones first, so
    /// they win over transitive ones in search paths.
    pub fn of(plan: &BuildPlan, step: &Step) -> Self {
        let by_id: HashMap<PackageVersionId, &Step> =
            plan.steps.iter().map(|s| (s.package.id, s)).collect();
        let mut exports = Self::default();
        let mut seen = HashSet::new();
        let mut queue: VecDeque<PackageVersionId> =
            step.package.dependencies.iter().map(|d| d.id).collect();

        while let Some(id) = queue.pop_front() {
            if !seen.insert(id) {
                continue;
            }
            let Some(dep) = by_id.get(&id) else { continue };
            queue.extend(dep.package.dependencies.iter().map(|d| d.id));
            if dep.package.host.is_some() {
                continue;
            }

            let prefix = &dep.paths.install;
            if let Some(bin) = &dep.package.bin
                && let Some(dir) = prefix.join(bin).parent()
            {
                push_unique(&mut exports.bin, dir.to_path_buf());
            }
            for library in &dep.package.libraries {
                if let Some(include) = &library.include {
                    push_unique(&mut exports.include, prefix.join(include));
                }
                let lib = prefix.join(&library.lib);
                push_unique(&mut exports.pkg_config, lib.join("pkgconfig"));
                push_unique(&mut exports.lib, lib);
            }
            if !dep.package.libraries.is_empty() {
                push_unique(&mut exports.pkg_config, prefix.join("share/pkgconfig"));
            }
            exports
                .prefixes
                .push((variable_name(&dep.package.name), prefix.clone()));
        }
        exports.pkg_config.retain(|dir| dir.is_dir());
        exports
    }

    /// The variables to set, each extending its value in `base` (the
    /// process environment).
    pub fn vars(&self, base: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
        let join = |dirs: &[PathBuf]| {
            dirs.iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
        };
        let extend = |name: &str, mut items: Vec<String>, separator: &str| {
            items.extend(base(name).filter(|v| !v.is_empty()));
            (name.to_string(), items.join(separator))
        };

        let mut vars = Vec::new();
        if !self.include.is_empty() {
            vars.push(extend("CPATH", join(&self.include), ":"));
            let flags = self
                .include
                .iter()
                .map(|d| format!("-I{}", d.display()))
                .collect();
            vars.push(extend("CPPFLAGS", flags, " "));
        }
        if !self.lib.is_empty() {
            vars.push(extend("LIBRARY_PATH", join(&self.lib), ":"));
            vars.push(extend("LD_LIBRARY_PATH", join(&self.lib), ":"));
            let flags = self
                .lib
                .iter()
                .map(|d| format!("-L{0} -Wl,-rpath,{0}", d.display()))
                .collect();
            vars.push(extend("LDFLAGS", flags, " "));
        }
        if !self.pkg_config.is_empty() {
            vars.push(extend("PKG_CONFIG_PATH", join(&self.pkg_config), ":"));
        }
        if !self.prefixes.is_empty() {
            let prefixes = self
                .prefixes
                .iter()
                .map(|(_, p)| p.clone())
                .collect::<Vec<_>>();
            vars.push(extend("CMAKE_PREFIX_PATH", join(&prefixes), ":"));
        }
        for (name, prefix) in &self.prefixes {
            vars.push((name.clone(), prefix.display().to_string()));
        }
        vars
    }
}

fn push_unique(list: &mut Vec<PathBuf>, path: PathBuf) {
    if !list.contains(&path) {
        list.push(path);
    }
}

/// `NUX_DEP_<NAME>`: uppercase, `+` as `X`, anything else non-alphanumeric
/// as `_`.
pub(crate) fn variable_name(package: &str) -> String {
    let name: String = package
        .chars()
        .map(|c| match c {
            '+' => 'X',
            c if c.is_ascii_alphanumeric() => c.to_ascii_uppercase(),
            _ => '_',
        })
        .collect();
    format!("NUX_DEP_{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_names() {
        assert_eq!(variable_name("zlib"), "NUX_DEP_ZLIB");
        assert_eq!(variable_name("libstdc++"), "NUX_DEP_LIBSTDCXX");
        assert_eq!(variable_name("gtk-4.0"), "NUX_DEP_GTK_4_0");
    }

    #[test]
    fn vars_extend_the_base_environment() {
        let exports = DependencyExports {
            bin: vec![],
            include: vec!["/p/z/include".into()],
            lib: vec!["/p/z/lib".into()],
            pkg_config: vec![],
            prefixes: vec![("NUX_DEP_ZLIB".into(), "/p/z".into())],
        };
        let base = |name: &str| match name {
            "CPATH" => Some("/usr/local/include".into()),
            "LDFLAGS" => Some("-O2".into()),
            "LD_LIBRARY_PATH" => Some(String::new()),
            _ => None,
        };
        assert_eq!(
            exports.vars(base),
            [
                ("CPATH".into(), "/p/z/include:/usr/local/include".into()),
                ("CPPFLAGS".into(), "-I/p/z/include".into()),
                ("LIBRARY_PATH".into(), "/p/z/lib".into()),
                ("LD_LIBRARY_PATH".into(), "/p/z/lib".into()),
                (
                    "LDFLAGS".into(),
                    "-L/p/z/lib -Wl,-rpath,/p/z/lib -O2".into()
                ),
                ("CMAKE_PREFIX_PATH".into(), "/p/z".into()),
                ("NUX_DEP_ZLIB".into(), "/p/z".into()),
            ]
        );
        assert!(DependencyExports::default().vars(base).is_empty());
    }
}
