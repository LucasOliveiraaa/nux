//! Runs commands in a sandbox, so that what a recipe's build commands do
//! cannot reach beyond what the build needs.
//!
//! A sandboxed command runs in its own user, mount, PID, IPC, UTS and cgroup
//! namespaces (and network namespace, with [`Sandbox::network`] off), set up
//! by the process itself: no setuid helper and no root are needed, only
//! unprivileged user namespaces (Linux 5.12 or later). Inside:
//!
//! - **The host is read-only.** The whole filesystem is visible, so
//!   compilers, headers and libraries work as usual, but nothing can be
//!   changed except the paths exposed with [`Access::ReadWrite`]. Setuid bits
//!   are ignored and `no_new_privs` is set.
//! - **Personal files are hidden.** `/home`, `/root`, `/tmp`, `/var/tmp`,
//!   `/run`, `/mnt`, `/media` and `/srv` are replaced by empty, writable,
//!   in-memory directories. Only what is exposed with [`Sandbox::expose`]
//!   shows through, and session variables pointing at hidden sockets
//!   (`SSH_AUTH_SOCK`, `DBUS_SESSION_BUS_ADDRESS`...) are removed.
//! - **Devices are minimal.** `/dev` only has `null`, `zero`, `full`,
//!   `random`, `urandom`, `tty`, a private `pts` and `shm`.
//! - **Processes are private.** The command is alone in its `/proc`, under a
//!   small init process; when the command exits, everything it left running
//!   is killed. If the calling process dies, the sandbox dies with it.
//! - **The network** is shared with the host by default, since builds may
//!   download dependencies. Without it, only a loopback interface exists.
//!
//! ```no_run
//! use std::process::Command;
//! use nux_firebox::{Access, Sandbox};
//!
//! let mut sandbox = Sandbox::new();
//! sandbox.expose("/home/me/project/build", Access::ReadWrite);
//! let mut command = Command::new("make");
//! command.current_dir("/home/me/project/build");
//! let session = sandbox.apply(&mut command)?;
//! let status = command.status()?;
//! session.finish()?;
//! # Ok::<(), std::io::Error>(())
//! ```

use std::ffi::{OsStr, c_int};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};

mod plan;
mod sys;

/// Directories replaced by empty ones inside the sandbox.
pub const HIDDEN: &[&str] = &["/home", "/root", "/tmp", "/var/tmp", "/run", "/mnt", "/media", "/srv"];

/// Environment variables removed from sandboxed commands: they point at
/// sockets and sessions the sandbox hides.
pub const SESSION_VARIABLES: &[&str] = &[
    "SSH_AUTH_SOCK",
    "SSH_AGENT_PID",
    "GPG_AGENT_INFO",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_RUNTIME_DIR",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
];

/// How an exposed path can be used inside the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    ReadWrite,
}

/// What a sandbox lets through; see the [crate documentation](crate).
#[derive(Debug, Clone)]
pub struct Sandbox {
    hidden: Vec<PathBuf>,
    exposed: Vec<(PathBuf, Access)>,
    directories: Vec<PathBuf>,
    network: bool,
    hostname: String,
}

impl Default for Sandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Sandbox {
    /// A sandbox hiding [`HIDDEN`], with the host's network.
    pub fn new() -> Self {
        Self {
            hidden: HIDDEN.iter().map(PathBuf::from).collect(),
            exposed: Vec::new(),
            directories: Vec::new(),
            network: true,
            hostname: "firebox".into(),
        }
    }

    /// Also replaces `path` by an empty directory.
    pub fn hide(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.hidden.push(path.into());
        self
    }

    /// Shows the host's `path` at the same place, even inside a hidden
    /// directory. Anything else is read-only or hidden. Paths that do not
    /// exist are ignored.
    pub fn expose(&mut self, path: impl Into<PathBuf>, access: Access) -> &mut Self {
        self.exposed.push((path.into(), access));
        self
    }

    /// Creates `path` as an empty writable directory if it is in a hidden
    /// directory, such as `$HOME`, so tools can keep their caches there for
    /// the length of the command.
    pub fn directory(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.directories.push(path.into());
        self
    }

    /// Whether the command shares the host's network (the default).
    pub fn network(&mut self, enabled: bool) -> &mut Self {
        self.network = enabled;
        self
    }

    pub fn hostname(&mut self, name: impl Into<String>) -> &mut Self {
        self.hostname = name.into();
        self
    }

    /// Exposes, read-only, the programs on `path` (a `$PATH` value) that
    /// would otherwise be hidden, such as `~/.local/bin` or `~/.cargo/bin`,
    /// along with what they need to run:
    ///
    /// - for a `bin` directory, the `lib`, `lib64` and `libexec` next to it;
    /// - for a `shims` directory (pyenv, rbenv...), the whole directory
    ///   holding it;
    /// - when a directory has `rustup`, the toolchains in `$RUSTUP_HOME`
    ///   (`~/.rustup` by default).
    pub fn expose_programs(&mut self, path: &OsStr) -> &mut Self {
        let hidden: Vec<PathBuf> = std::env::split_paths(path).filter(|d| d.is_absolute() && self.is_hidden(d)).collect();
        for dir in hidden {
            let parent = dir.parent().map(Path::to_path_buf);
            match (dir.file_name().and_then(OsStr::to_str), parent) {
                (Some("bin"), Some(prefix)) => {
                    for sibling in ["lib", "lib64", "libexec"] {
                        self.expose(prefix.join(sibling), Access::ReadOnly);
                    }
                }
                (Some("shims"), Some(prefix)) => {
                    self.expose(prefix, Access::ReadOnly);
                }
                _ => {}
            }
            if dir.join("rustup").is_file() {
                let rustup = std::env::var_os("RUSTUP_HOME")
                    .map(PathBuf::from)
                    .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".rustup")));
                if let Some(rustup) = rustup {
                    self.expose(rustup, Access::ReadOnly);
                }
            }
            self.expose(dir, Access::ReadOnly);
        }
        self
    }

    fn is_hidden(&self, path: &Path) -> bool {
        self.hidden.iter().any(|hidden| path.starts_with(hidden))
    }

    /// Makes `command` run inside the sandbox when it is spawned, and
    /// removes the [`SESSION_VARIABLES`] from its environment. The
    /// returned [`Session`] reports whether the sandbox could be set up:
    /// call [`Session::finish`] once the command has exited.
    ///
    /// A command that exits because of a signal is reported as exiting with
    /// `128 + signal`.
    pub fn apply(&self, command: &mut Command) -> io::Result<Session> {
        let cwd = match command.get_current_dir() {
            Some(dir) => dir.to_path_buf(),
            None => std::env::current_dir()?,
        };
        let plan = Arc::new(plan::Plan::new(self, &cwd)?);
        let (read, write) = pipe()?;
        for variable in SESSION_VARIABLES {
            command.env_remove(variable);
        }
        let report = write.as_raw_fd();
        let child_plan = Arc::clone(&plan);
        let mut slots = vec![-1 as c_int; plan.slots].into_boxed_slice();
        // SAFETY: `sys::enter` only makes system calls on data prepared
        // here; it neither allocates nor takes locks.
        unsafe {
            command.pre_exec(move || sys::enter(&child_plan.ops, &mut slots, report));
        }
        Ok(Session { plan, read, write: Some(write) })
    }

    /// Checks once whether sandboxes can be created here, by running `true`
    /// in one. The answer is kept for the life of the process.
    pub fn available() -> Result<(), String> {
        static AVAILABLE: OnceLock<Result<(), String>> = OnceLock::new();
        AVAILABLE
            .get_or_init(|| {
                let mut command = Command::new("/bin/sh");
                command.args(["-c", "exit 0"]).current_dir("/");
                let session = Sandbox::new().apply(&mut command).map_err(|e| e.to_string())?;
                let status = command.status().map_err(|e| e.to_string());
                session.finish().map_err(|e| e.to_string())?;
                match status? {
                    status if status.success() => Ok(()),
                    status => Err(format!("a test command {status}")),
                }
            })
            .clone()
    }
}

/// A command set up by [`Sandbox::apply`].
pub struct Session {
    plan: Arc<plan::Plan>,
    read: OwnedFd,
    write: Option<OwnedFd>,
}

impl Session {
    /// Once the command has exited (or failed to start): an error if the
    /// sandbox could not be set up, saying which step failed and why.
    pub fn finish(mut self) -> io::Result<()> {
        drop(self.write.take());
        let mut message = Vec::new();
        std::fs::File::from(self.read.try_clone()?).read_to_end(&mut message)?;
        let Ok(message) = <[u8; 8]>::try_from(message.as_slice()) else {
            return Ok(());
        };
        let index = u32::from_ne_bytes(message[..4].try_into().unwrap()) as usize;
        let errno = c_int::from_ne_bytes(message[4..].try_into().unwrap());
        let step = self.plan.ops.get(index).map_or_else(|| "set it up".into(), sys::Op::describe);
        Err(io::Error::other(format!(
            "cannot create the sandbox: failed to {step}: {}",
            io::Error::from_raw_os_error(errno)
        )))
    }
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as c_int; 2];
    // SAFETY: `fds` has room for both ends.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just created and are owned here.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

#[cfg(test)]
mod tests;
