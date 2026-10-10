//! The parent side: turning a [`Sandbox`] into the steps the child runs.
//! Everything that needs memory (paths, file contents, which paths exist)
//! is done here, before forking.

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use crate::sys::Op;
use crate::{Access, Sandbox};

/// Device nodes kept in the sandbox's `/dev`.
const DEVICES: &[&str] = &["null", "zero", "full", "random", "urandom", "tty"];

#[derive(Debug)]
pub(crate) struct Plan {
    pub ops: Vec<Op>,
    /// How many mount copies are held at once.
    pub slots: usize,
}

impl Plan {
    /// The steps to run `sandbox` with `cwd` as the working directory.
    pub fn new(sandbox: &Sandbox, cwd: &Path) -> io::Result<Self> {
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        let mut namespaces = libc::CLONE_NEWUSER
            | libc::CLONE_NEWNS
            | libc::CLONE_NEWPID
            | libc::CLONE_NEWIPC
            | libc::CLONE_NEWUTS
            | libc::CLONE_NEWCGROUP;
        if !sandbox.network {
            namespaces |= libc::CLONE_NEWNET;
        }

        // Real directories only: `/var/run` is usually a link to `/run`.
        let hidden: Vec<PathBuf> = dedup(sandbox.hidden.iter().map(|p| normalize(p)))
            .into_iter()
            .filter(|p| p.symlink_metadata().is_ok_and(|m| m.is_dir()))
            .collect();
        let is_hidden = |path: &Path| hidden.iter().any(|h| path.starts_with(h));

        let mut exposed: Vec<(PathBuf, Access)> = Vec::new();
        for (path, access) in &sandbox.exposed {
            let path = normalize(path);
            if !path.exists() {
                continue;
            }
            match exposed.iter_mut().find(|(p, _)| *p == path) {
                Some((_, existing)) if access == &Access::ReadWrite => *existing = Access::ReadWrite,
                Some(_) => {}
                None => exposed.push((path, *access)),
            }
        }
        // The resolver configuration often lives in `/run`.
        if sandbox.network
            && let Ok(resolv) = std::fs::canonicalize("/etc/resolv.conf")
            && let Some(dir) = resolv.parent().filter(|d| is_hidden(d))
        {
            exposed.push((dir.to_path_buf(), Access::ReadOnly));
        }
        let cwd = normalize(cwd);
        if is_hidden(&cwd) && !exposed.iter().any(|(path, _)| cwd.starts_with(path)) {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("the working directory `{}` is hidden in the sandbox; expose it", cwd.display()),
            ));
        }
        // Parents before children, so a writable directory can sit inside a
        // read-only one.
        exposed.sort_by_key(|(path, _)| path.components().count());

        let mut ops = vec![
            Op::DieWithParent,
            Op::Unshare(namespaces),
            Op::WriteFile { path: c("/proc/self/setgroups")?, contents: b"deny".to_vec() },
            Op::WriteFile { path: c("/proc/self/uid_map")?, contents: format!("{uid} {uid} 1\n").into_bytes() },
            Op::WriteFile { path: c("/proc/self/gid_map")?, contents: format!("{gid} {gid} 1\n").into_bytes() },
            // The child is the first process of the new PID namespace.
            Op::Fork,
            Op::DieWithParent,
            Op::MakePrivate,
        ];

        // Copies of what shows through, taken before the host is hidden.
        let mut slots = 0;
        let mut attach = Vec::new();
        for (path, access) in &exposed {
            ops.push(Op::Clone { source: c(path)?, slot: slots, read_only: *access == Access::ReadOnly });
            attach.push((slots, path.clone()));
            slots += 1;
        }
        let mut devices = Vec::new();
        for device in DEVICES {
            let path = Path::new("/dev").join(device);
            if path.exists() {
                ops.push(Op::Clone { source: c(&path)?, slot: slots, read_only: false });
                devices.push((slots, path));
                slots += 1;
            }
        }

        ops.push(Op::RootReadOnly);
        for dir in &hidden {
            let mode = if dir.starts_with("/tmp") || dir.starts_with("/var/tmp") { "mode=1777" } else { "mode=0755" };
            ops.push(Op::Tmpfs { target: c(dir)?, options: c(mode)? });
        }

        ops.push(Op::Tmpfs { target: c("/dev")?, options: c("mode=0755")? });
        for (slot, path) in devices {
            ops.push(Op::File(c(&path)?));
            ops.push(Op::Attach { slot, target: c(&path)? });
        }
        ops.push(Op::Dir(c("/dev/pts")?));
        ops.push(Op::Devpts);
        ops.push(Op::Symlink { target: c("pts/ptmx")?, link: c("/dev/ptmx")? });
        ops.push(Op::Dir(c("/dev/shm")?));
        ops.push(Op::Tmpfs { target: c("/dev/shm")?, options: c("mode=1777")? });
        for (link, target) in
            [("fd", "/proc/self/fd"), ("stdin", "/proc/self/fd/0"), ("stdout", "/proc/self/fd/1"), ("stderr", "/proc/self/fd/2")]
        {
            ops.push(Op::Symlink { target: c(target)?, link: c(Path::new("/dev").join(link))? });
        }
        ops.push(Op::Proc);

        // Mount points inside hidden directories have to be created first.
        for (slot, path) in attach {
            if let Some(root) = hidden.iter().find(|h| path.starts_with(h)) {
                for dir in between(root, path.parent().unwrap_or(root)) {
                    ops.push(Op::Dir(c(dir)?));
                }
                ops.push(if path.is_dir() { Op::Dir(c(&path)?) } else { Op::File(c(&path)?) });
            }
            ops.push(Op::Attach { slot, target: c(&path)? });
        }
        for dir in &sandbox.directories {
            let dir = normalize(dir);
            if let Some(root) = hidden.iter().find(|h| dir.starts_with(h)) {
                for dir in between(root, &dir) {
                    ops.push(Op::Dir(c(dir)?));
                }
            }
        }

        ops.push(Op::Hostname(c(&sandbox.hostname)?));
        if !sandbox.network {
            ops.push(Op::LoopbackUp);
        }
        // The child runs the command; this process stays as its init.
        ops.push(Op::Fork);
        ops.push(Op::NoNewPrivileges);
        ops.push(Op::Chdir(c(cwd)?));
        Ok(Self { ops, slots })
    }
}

fn c(path: impl AsRef<Path>) -> io::Result<CString> {
    CString::new(path.as_ref().as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("`{}` contains a NUL byte", path.as_ref().display())))
}

/// `path` made absolute, without `.` and `..`; symlinks are kept.
fn normalize(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut normal = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            other => normal.push(other),
        }
    }
    normal
}

fn dedup(paths: impl Iterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut unique: Vec<PathBuf> = Vec::new();
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    unique
}

/// The directories strictly below `root`, down to and including `to`.
fn between(root: &Path, to: &Path) -> Vec<PathBuf> {
    let Ok(rest) = to.strip_prefix(root) else { return Vec::new() };
    let mut dir = root.to_path_buf();
    rest.components()
        .map(|component| {
            dir.push(component);
            dir.clone()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_normalized_and_split() {
        assert_eq!(normalize(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
        assert_eq!(between(Path::new("/home"), Path::new("/home/me/.cargo")), [PathBuf::from("/home/me"), PathBuf::from("/home/me/.cargo")]);
        assert!(between(Path::new("/home"), Path::new("/home")).is_empty());
        assert!(between(Path::new("/home"), Path::new("/usr")).is_empty());
    }
}
