//! The child side: entering the sandbox between `fork` and `exec`.
//!
//! This runs in a child forked from a possibly multithreaded process, so it
//! must not allocate, lock or panic: everything is prepared by the parent
//! (see [`crate::plan`]) and only raw system calls are made here.
//!
//! The command's child forks twice:
//!
//! ```text
//! nux ── supervisor ── init (pid 1) ── command
//! ```
//!
//! The supervisor creates the namespaces and waits; `init`, the first
//! process of the new PID namespace, sets up the mounts, reaps orphans and
//! exits with the command's status, which kills whatever the command left
//! running. Both die with their parent.

use std::ffi::{CStr, CString, c_int, c_long, c_uint};
use std::io;

// The mount API (linux/mount.h), not in every libc version.
const OPEN_TREE_CLONE: c_uint = 1;
const OPEN_TREE_CLOEXEC: c_uint = libc::O_CLOEXEC as c_uint;
const MOVE_MOUNT_F_EMPTY_PATH: c_uint = 4;
const AT_RECURSIVE: c_uint = 0x8000;
const MOUNT_ATTR_RDONLY: u64 = 0x1;
const MOUNT_ATTR_NOSUID: u64 = 0x2;

#[repr(C)]
struct MountAttr {
    attr_set: u64,
    attr_clr: u64,
    propagation: u64,
    userns_fd: u64,
}

/// One step of entering the sandbox.
#[derive(Debug)]
pub(crate) enum Op {
    /// Die with the parent.
    DieWithParent,
    Unshare(c_int),
    WriteFile {
        path: CString,
        contents: Vec<u8>,
    },
    /// The parent waits for the child and exits with its status; the child
    /// goes on with the next steps.
    Fork,
    /// Stops mounts from propagating back to the host.
    MakePrivate,
    /// Takes a copy of `source` (and what is mounted under it) into `slot`,
    /// before the host is hidden.
    Clone {
        source: CString,
        slot: usize,
        read_only: bool,
    },
    /// Makes every mount read-only and ignores setuid bits.
    RootReadOnly,
    Tmpfs {
        target: CString,
        options: CString,
    },
    Dir(CString),
    File(CString),
    /// Mounts the copy in `slot` on `target`.
    Attach {
        slot: usize,
        target: CString,
    },
    Proc,
    Devpts,
    Symlink {
        target: CString,
        link: CString,
    },
    Hostname(CString),
    LoopbackUp,
    NoNewPrivileges,
    Chdir(CString),
}

impl Op {
    pub fn describe(&self) -> String {
        let show = |path: &CStr| path.to_string_lossy().into_owned();
        match self {
            Op::DieWithParent => "set the parent-death signal".into(),
            Op::Unshare(_) => {
                "create the namespaces (are unprivileged user namespaces enabled?)".into()
            }
            Op::WriteFile { path, .. } => format!("write `{}`", show(path)),
            Op::Fork => "fork".into(),
            Op::MakePrivate => "make the mounts private".into(),
            Op::Clone { source, .. } => format!("copy the mount of `{}`", show(source)),
            Op::RootReadOnly => "make the host read-only".into(),
            Op::Tmpfs { target, .. } => format!("mount a tmpfs on `{}`", show(target)),
            Op::Dir(path) => format!("create `{}`", show(path)),
            Op::File(path) => format!("create `{}`", show(path)),
            Op::Attach { target, .. } => format!("mount `{}`", show(target)),
            Op::Proc => "mount `/proc`".into(),
            Op::Devpts => "mount `/dev/pts`".into(),
            Op::Symlink { link, .. } => format!("create `{}`", show(link)),
            Op::Hostname(_) => "set the hostname".into(),
            Op::LoopbackUp => "bring up the loopback interface".into(),
            Op::NoNewPrivileges => "set no_new_privs".into(),
            Op::Chdir(path) => format!("change to `{}`", show(path)),
        }
    }
}

/// Runs `ops` in the forked child. Returns in the process that goes on to
/// exec the command; the others exit. A failing step is reported on
/// `report` as its index and `errno`, then the process exits with 127.
pub(crate) fn enter(ops: &[Op], slots: &mut [c_int], report: c_int) -> io::Result<()> {
    for (index, op) in ops.iter().enumerate() {
        // SAFETY: each step only makes system calls on data prepared by the
        // parent; nothing allocates.
        if let Err(errno) = unsafe { run(op, slots, report) } {
            fail(report, index, errno);
        }
    }
    Ok(())
}

fn fail(report: c_int, index: usize, errno: c_int) -> ! {
    let mut message = [0u8; 8];
    message[..4].copy_from_slice(&(index as u32).to_ne_bytes());
    message[4..].copy_from_slice(&errno.to_ne_bytes());
    // SAFETY: writing a stack buffer, then exiting without running atexit
    // handlers that belong to the parent.
    unsafe {
        libc::write(report, message.as_ptr().cast(), message.len());
        libc::_exit(127)
    }
}

fn errno() -> c_int {
    io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EIO)
}

fn check(result: c_long) -> Result<c_long, c_int> {
    if result < 0 { Err(errno()) } else { Ok(result) }
}

unsafe fn run(op: &Op, slots: &mut [c_int], report: c_int) -> Result<(), c_int> {
    unsafe {
        match op {
            Op::DieWithParent => {
                check(libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) as c_long)?;
            }
            Op::Unshare(flags) => {
                check(libc::unshare(*flags) as c_long)?;
            }
            Op::WriteFile { path, contents } => {
                let fd =
                    check(libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC) as c_long)?
                        as c_int;
                let written = libc::write(fd, contents.as_ptr().cast(), contents.len());
                let error = errno();
                libc::close(fd);
                if written != contents.len() as isize {
                    return Err(error);
                }
            }
            Op::Fork => {
                let pid = check(libc::fork() as c_long)? as libc::pid_t;
                if pid > 0 {
                    supervise(pid, report);
                }
            }
            Op::MakePrivate => {
                let flags = libc::MS_REC | libc::MS_PRIVATE;
                check(libc::mount(
                    std::ptr::null(),
                    c"/".as_ptr(),
                    std::ptr::null(),
                    flags,
                    std::ptr::null(),
                ) as c_long)?;
            }
            Op::Clone {
                source,
                slot,
                read_only,
            } => {
                let flags = OPEN_TREE_CLONE | OPEN_TREE_CLOEXEC | AT_RECURSIVE;
                let fd = check(libc::syscall(
                    libc::SYS_open_tree,
                    libc::AT_FDCWD,
                    source.as_ptr(),
                    flags,
                ))? as c_int;
                slots[*slot] = fd;
                let read_only = if *read_only { MOUNT_ATTR_RDONLY } else { 0 };
                set_attributes(
                    fd,
                    c"",
                    libc::AT_EMPTY_PATH as c_uint,
                    read_only | MOUNT_ATTR_NOSUID,
                    0,
                )?;
            }
            Op::RootReadOnly => {
                set_attributes(
                    libc::AT_FDCWD,
                    c"/",
                    0,
                    MOUNT_ATTR_RDONLY | MOUNT_ATTR_NOSUID,
                    0,
                )?;
            }
            Op::Tmpfs { target, options } => {
                let flags = libc::MS_NOSUID | libc::MS_NODEV;
                check(libc::mount(
                    c"tmpfs".as_ptr(),
                    target.as_ptr(),
                    c"tmpfs".as_ptr(),
                    flags,
                    options.as_ptr().cast(),
                ) as c_long)?;
            }
            Op::Dir(path) => {
                if libc::mkdir(path.as_ptr(), 0o755) != 0 && errno() != libc::EEXIST {
                    return Err(errno());
                }
            }
            Op::File(path) => {
                let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW;
                let fd = check(libc::open(path.as_ptr(), flags, 0o644 as c_uint) as c_long)?;
                libc::close(fd as c_int);
            }
            Op::Attach { slot, target } => {
                let fd = slots[*slot];
                let flags = MOVE_MOUNT_F_EMPTY_PATH;
                check(libc::syscall(
                    libc::SYS_move_mount,
                    fd,
                    c"".as_ptr(),
                    libc::AT_FDCWD,
                    target.as_ptr(),
                    flags,
                ))?;
                libc::close(fd);
                slots[*slot] = -1;
            }
            Op::Proc => {
                let flags = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
                check(libc::mount(
                    c"proc".as_ptr(),
                    c"/proc".as_ptr(),
                    c"proc".as_ptr(),
                    flags,
                    std::ptr::null(),
                ) as c_long)?;
            }
            Op::Devpts => {
                let flags = libc::MS_NOSUID | libc::MS_NOEXEC;
                let options = c"newinstance,ptmxmode=0666,mode=0620";
                check(libc::mount(
                    c"devpts".as_ptr(),
                    c"/dev/pts".as_ptr(),
                    c"devpts".as_ptr(),
                    flags,
                    options.as_ptr().cast(),
                ) as c_long)?;
            }
            Op::Symlink { target, link } => {
                check(libc::symlink(target.as_ptr(), link.as_ptr()) as c_long)?;
            }
            Op::Hostname(name) => {
                check(libc::sethostname(name.as_ptr(), name.as_bytes().len()) as c_long)?;
            }
            Op::LoopbackUp => {
                let socket = check(libc::socket(
                    libc::AF_INET,
                    libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
                    0,
                ) as c_long)? as c_int;
                let mut request: libc::ifreq = std::mem::zeroed();
                for (to, from) in request.ifr_name.iter_mut().zip(b"lo") {
                    *to = *from as libc::c_char;
                }
                request.ifr_ifru.ifru_flags = (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
                let result = libc::ioctl(socket, libc::SIOCSIFFLAGS, &request);
                let error = errno();
                libc::close(socket);
                if result != 0 {
                    return Err(error);
                }
            }
            Op::NoNewPrivileges => {
                check(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) as c_long)?;
            }
            Op::Chdir(path) => {
                check(libc::chdir(path.as_ptr()) as c_long)?;
            }
        }
    }
    Ok(())
}

/// `mount_setattr`, recursively.
unsafe fn set_attributes(
    dirfd: c_int,
    path: &CStr,
    flags: c_uint,
    set: u64,
    clear: u64,
) -> Result<(), c_int> {
    let attributes = MountAttr {
        attr_set: set,
        attr_clr: clear,
        propagation: 0,
        userns_fd: 0,
    };
    let size = std::mem::size_of::<MountAttr>();
    // SAFETY: `attributes` outlives the call and `size` is its size.
    check(unsafe {
        libc::syscall(
            libc::SYS_mount_setattr,
            dirfd,
            path.as_ptr(),
            flags | AT_RECURSIVE,
            &attributes,
            size,
        )
    })?;
    Ok(())
}

/// The parent side of [`Op::Fork`]: keeps only `report` open (so that the
/// command's exec closes the last copy of the pipe `std` watches), reaps
/// every child, and exits with `child`'s status once it ends.
fn supervise(child: libc::pid_t, report: c_int) -> ! {
    // SAFETY: closing descriptors and waiting are plain system calls.
    unsafe {
        if report > 3 {
            libc::syscall(libc::SYS_close_range, 3, report - 1, 0);
        }
        libc::syscall(libc::SYS_close_range, report.max(2) + 1, c_uint::MAX, 0);
        loop {
            let mut status = 0;
            let pid = libc::waitpid(-1, &mut status, 0);
            if pid == child {
                let code = if libc::WIFSIGNALED(status) {
                    128 + libc::WTERMSIG(status)
                } else {
                    libc::WEXITSTATUS(status)
                };
                libc::_exit(code);
            }
            if pid < 0 && errno() != libc::EINTR {
                libc::_exit(127);
            }
        }
    }
}
