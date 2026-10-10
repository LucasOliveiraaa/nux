//! Comparing a resolved build order with what the workspace already has.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use nux_eval::SourceKind;
use nux_resolver::{PackageVersionId, ResolvedPackage, Version};

use crate::state::{Fingerprint, Metadata, SourceIdentity, source_fingerprint};
use crate::{PackagePaths, Workspace};

/// Where a step's source comes from, as of planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceStatus {
    /// The definition has no `source(...)`.
    None,
    /// A `source(local = ...)` path; nothing to download.
    Local(PathBuf),
    /// A valid download is in the workspace.
    Cached,
    /// Must be downloaded before building.
    Missing,
}

#[derive(Debug, Clone)]
pub struct Step {
    pub package: ResolvedPackage,
    /// Requested by the environment rather than pulled in as a dependency.
    pub requested: bool,
    pub paths: PackagePaths,
    /// Whether a valid installation of this exact definition exists. Always
    /// true for a version the host provides, while the host still has it.
    pub installed: bool,
    pub source: SourceStatus,
    /// The source identity this version must have (from `Nux.lock`).
    pub pin: Option<SourceIdentity>,
    pub(crate) fingerprint: String,
    pub(crate) source_fingerprint: Option<String>,
}

impl Step {
    /// The host executable that provides this version, if any.
    pub fn host(&self) -> Option<&Path> {
        self.package.host.as_ref().map(|h| h.path.as_path())
    }

    pub fn needs_build(&self) -> bool {
        !self.installed
    }

    pub fn needs_download(&self) -> bool {
        self.needs_build() && self.source == SourceStatus::Missing
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkStatus {
    Current,
    Missing,
    /// A link managed by nux that points at another package or version.
    Outdated,
    /// Something nux does not manage is in the way.
    Conflict(String),
}

/// A link `$NUX_HOME/bin/<name>` to an installed executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub name: String,
    pub package: String,
    pub version: String,
    /// The path the link holds, relative to the bin directory.
    pub target: PathBuf,
    pub status: LinkStatus,
}

#[derive(Debug, Clone)]
pub struct BuildPlan {
    /// Every required version, dependencies first.
    pub steps: Vec<Step>,
    pub links: Vec<Link>,
    /// Non-fatal notes, such as two packages providing the same command.
    pub warnings: Vec<String>,
}

impl BuildPlan {
    pub fn to_build(&self) -> impl Iterator<Item = &Step> {
        self.steps.iter().filter(|s| s.needs_build())
    }

    pub fn downloads(&self) -> usize {
        self.steps.iter().filter(|s| s.needs_download()).count()
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &Link> {
        self.links.iter().filter(|l| matches!(l.status, LinkStatus::Conflict(_)))
    }

    /// The steps to build, in groups that are built one after another.
    /// Every dependency a step needs built is in an earlier group, so the
    /// steps of a group do not depend on each other and can be built at
    /// the same time. Installed and host-provided dependencies do not
    /// delay anything. Within a group, steps keep their plan order.
    pub fn build_groups(&self) -> Vec<Vec<&Step>> {
        let mut group_of: HashMap<PackageVersionId, usize> = HashMap::new();
        let mut groups: Vec<Vec<&Step>> = Vec::new();
        // Steps are in build order, so dependencies come first.
        for step in self.to_build() {
            let group = step
                .package
                .dependencies
                .iter()
                .filter_map(|dependency| group_of.get(&dependency.id))
                .map(|group| group + 1)
                .max()
                .unwrap_or(0);
            group_of.insert(step.package.id, group);
            if groups.len() <= group {
                groups.resize_with(group + 1, Vec::new);
            }
            groups[group].push(step);
        }
        groups
    }

    /// Nothing to download, build or link.
    pub fn is_up_to_date(&self) -> bool {
        self.to_build().next().is_none() && self.links.iter().all(|l| l.status == LinkStatus::Current)
    }
}

pub(crate) fn plan(
    workspace: &Workspace,
    local_root: &Path,
    pins: &HashMap<(String, String), SourceIdentity>,
    packages: Vec<ResolvedPackage>,
    requested: &[PackageVersionId],
) -> BuildPlan {
    let mut fingerprints: HashMap<PackageVersionId, String> = HashMap::new();
    let steps: Vec<Step> = packages
        .into_iter()
        .map(|package| {
            let paths = workspace.package(&package.name, &package.version);
            let source_fingerprint = package.source.as_ref().map(source_fingerprint);

            // Dependencies come first in a build order, so theirs are known.
            let mut fingerprint = Fingerprint::new();
            fingerprint
                .add(&package.name)
                .add(&package.version)
                .add(source_fingerprint.as_deref().unwrap_or(""))
                .add(package.bin.as_deref().unwrap_or(""));
            for library in &package.libraries {
                fingerprint.add(&library.name).add(&library.lib).add(library.include.as_deref().unwrap_or(""));
            }
            // A different host executable means different dependents.
            if let Some(host) = &package.host {
                fingerprint.add("host").add(&host.path.to_string_lossy());
            }
            for command in &package.commands {
                fingerprint.add(command);
            }
            for dependency in &package.dependencies {
                fingerprint
                    .add(&dependency.name)
                    .add(&dependency.version)
                    .add(fingerprints.get(&dependency.id).map_or("", String::as_str));
            }
            let fingerprint = fingerprint.hex();
            fingerprints.insert(package.id, fingerprint.clone());

            let metadata = Metadata::read(&paths.metadata);
            let pin = pins.get(&(package.name.clone(), package.version.clone())).cloned();
            let pinned = |identity: &SourceIdentity| pin.as_ref().is_none_or(|pin| identity.satisfies(pin));
            let installed = match &package.host {
                Some(host) => executable(&host.path).is_ok(),
                None => metadata.installed.as_ref().is_some_and(|record| {
                    record.fingerprint == fingerprint
                        && paths.install.is_dir()
                        && package.bin.as_ref().is_none_or(|bin| executable(&paths.install.join(bin)).is_ok())
                        && package.libraries.iter().all(|lib| library(&paths.install, lib).is_ok())
                        && pinned(&record.identity)
                }),
            };
            let source = match &package.source {
                _ if package.host.is_some() => SourceStatus::None,
                None => SourceStatus::None,
                Some(source) => match &source.kind {
                    SourceKind::Local(path) => SourceStatus::Local(local_root.join(path)),
                    _ if source_cached(&metadata, &paths, source_fingerprint.as_deref())
                        && metadata.source.as_ref().is_some_and(|s| pinned(&s.identity)) =>
                    {
                        SourceStatus::Cached
                    }
                    _ => SourceStatus::Missing,
                },
            };

            Step {
                requested: requested.contains(&package.id),
                package,
                paths,
                installed,
                source,
                pin,
                fingerprint,
                source_fingerprint,
            }
        })
        .collect();

    let (links, warnings) = plan_links(workspace, &steps);
    BuildPlan { steps, links, warnings }
}

fn source_cached(metadata: &Metadata, paths: &PackagePaths, fingerprint: Option<&str>) -> bool {
    let Some(record) = &metadata.source else {
        return false;
    };
    if Some(record.fingerprint.as_str()) != fingerprint {
        return false;
    }
    match &record.file {
        Some(file) => paths.source.join(file).is_file(),
        None => paths.source.join(".git").exists(),
    }
}

/// Checks that `path` is an executable file.
pub(crate) fn executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Err(_) => Err("does not exist".into()),
        Ok(m) if !m.is_file() => Err("is not a file".into()),
        Ok(m) if m.permissions().mode() & 0o111 == 0 => Err("is not executable".into()),
        Ok(_) => Ok(()),
    }
}

/// Checks that `library` was installed under `prefix`: its header
/// directory, and `lib<name>.so` (possibly versioned) or `lib<name>.a`.
pub(crate) fn library(prefix: &Path, library: &nux_eval::Library) -> Result<(), String> {
    if let Some(include) = &library.include
        && !prefix.join(include).is_dir()
    {
        return Err(format!("its header directory `{include}` does not exist"));
    }
    let dir = prefix.join(&library.lib);
    let (shared, archive) = (format!("lib{}.so", library.name), format!("lib{}.a", library.name));
    let found = std::fs::read_dir(&dir).is_ok_and(|entries| {
        entries.filter_map(|e| e.ok()).any(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            file == archive || file == shared || file.starts_with(&format!("{shared}."))
        })
    });
    if found {
        Ok(())
    } else {
        Err(format!("neither `{shared}` nor `{archive}` is in `{}`", library.lib))
    }
}

/// One link per command name. Requested packages win over dependencies,
/// then higher versions over lower ones, matching how versions are selected.
fn plan_links(workspace: &Workspace, steps: &[Step]) -> (Vec<Link>, Vec<String>) {
    let mut candidates: BTreeMap<String, Vec<&Step>> = BTreeMap::new();
    // Host executables are already on the PATH.
    for step in steps.iter().filter(|s| s.package.host.is_none()) {
        let Some(bin) = &step.package.bin else { continue };
        // The name `set_bin` gave, or the executable's file name.
        let name = match &step.package.bin_name {
            Some(name) => name.clone(),
            None => match Path::new(bin).file_name() {
                Some(name) => name.to_string_lossy().into_owned(),
                None => continue,
            },
        };
        candidates.entry(name).or_default().push(step);
    }

    let version = |s: &Step| Version::parse(&s.package.version).ok();
    let mut links = Vec::new();
    let mut warnings = Vec::new();
    for (name, mut steps) in candidates {
        steps.sort_by(|a, b| {
            b.requested
                .cmp(&a.requested)
                .then_with(|| version(b).cmp(&version(a)))
                .then_with(|| a.package.name.cmp(&b.package.name))
        });
        let winner = steps[0];
        if let Some(rival) = steps[1..]
            .iter()
            .find(|s| s.requested == winner.requested && s.package.name != winner.package.name)
        {
            warnings.push(format!(
                "`{}` and `{}` both provide `{name}`; linking `{}`",
                winner.package.name, rival.package.name, winner.package.name
            ));
        }

        let target = Path::new("..")
            .join("packages")
            .join(&winner.package.name)
            .join(&winner.package.version)
            .join("install")
            .join(winner.package.bin.as_ref().unwrap());
        let status = link_status(&workspace.bin_dir().join(&name), &target);
        links.push(Link {
            name,
            package: winner.package.name.clone(),
            version: winner.package.version.clone(),
            target,
            status,
        });
    }
    (links, warnings)
}

fn link_status(path: &Path, target: &Path) -> LinkStatus {
    match std::fs::read_link(path) {
        Ok(existing) if existing == target => LinkStatus::Current,
        Ok(existing) if existing.starts_with(Path::new("..").join("packages")) => LinkStatus::Outdated,
        Ok(existing) => LinkStatus::Conflict(format!("it links to `{}`, which nux does not manage", existing.display())),
        Err(_) if path.symlink_metadata().is_ok() => {
            LinkStatus::Conflict("it is not a link created by nux".into())
        }
        Err(_) => LinkStatus::Missing,
    }
}
