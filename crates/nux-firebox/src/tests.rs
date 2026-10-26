//! These run real commands in real sandboxes. Where unprivileged user
//! namespaces are unavailable, they only check that this is reported.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use super::*;

/// A fresh directory under `/tmp`, which sandboxes hide.
fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("nux-firebox-tests-{}", std::process::id()))
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs `script` with `sh` in `sandbox`, from `cwd`: its exit code and output.
fn run(sandbox: &Sandbox, cwd: &Path, script: &str) -> (i32, String) {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", script])
        .current_dir(cwd)
        .env("LC_ALL", "C");
    let session = sandbox.apply(&mut command).unwrap();
    let output = command.output().unwrap();
    session.finish().unwrap();
    let text = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    (output.status.code().unwrap(), text)
}

fn supported() -> bool {
    match Sandbox::available() {
        Ok(()) => true,
        Err(reason) => {
            eprintln!("skipping: sandboxes are not available here: {reason}");
            false
        }
    }
}

#[test]
fn the_host_is_read_only_except_what_is_exposed() {
    if !supported() {
        return;
    }
    let dir = temp("rw");
    let (build, sources, secret) = (dir.join("build"), dir.join("sources"), dir.join("secret"));
    for d in [&build, &sources, &secret] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(sources.join("main.c"), "int main;").unwrap();
    std::fs::write(secret.join("key"), "hunter2").unwrap();
    let mut sandbox = Sandbox::new();
    sandbox
        .expose(&build, Access::ReadWrite)
        .expose(&sources, Access::ReadOnly);

    let (code, out) = run(
        &sandbox,
        &build,
        "echo built > out && cat ../sources/main.c && pwd",
    );
    assert_eq!(
        (code, out.as_str()),
        (0, format!("int main;{}\n", build.display()).as_str())
    );
    assert_eq!(
        std::fs::read_to_string(build.join("out")).unwrap(),
        "built\n"
    );

    let script = "touch ../sources/x; touch /usr/x; test -e ../secret/key || echo hidden";
    let (code, out) = run(&sandbox, &build, script);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("'../sources/x': Read-only file system"),
        "{out}"
    );
    assert!(out.contains("'/usr/x': Read-only file system"), "{out}");
    assert!(out.starts_with("hidden\n"), "{out}");
    assert!(!sources.join("x").exists());
}

#[test]
fn hidden_directories_are_empty_and_writable() {
    if !supported() {
        return;
    }
    let dir = temp("hidden");
    let home = dir.join("home/me");
    std::fs::create_dir_all(dir.join("visible")).unwrap();
    let mut sandbox = Sandbox::new();
    sandbox
        .expose(dir.join("visible"), Access::ReadOnly)
        .directory(&home)
        .expose("/nonexistent", Access::ReadOnly);
    // `/tmp` only shows the exposed directory; `home` is empty and writable.
    let script = format!(
        "ls -A /tmp | wc -l; \
     n=$(ls -A /run | wc -l); echo $n; \
     [ $n -eq 0 ] || ls -A /run | sed 's/^/run: /'; \
     echo cache > {home}/c && cat {home}/c; touch /tmp/new && echo tmp ok",
        home = home.display()
    );
    let (code, out) = run(&sandbox, Path::new("/"), &script);
    assert_eq!((code, out.as_str()), (0, "1\n0\ncache\ntmp ok\n"), "{out}");
    assert!(!home.exists(), "nothing reaches the host");
}

#[test]
fn processes_devices_and_identity_are_private() {
    if !supported() {
        return;
    }
    let sandbox = Sandbox::new();
    let script = "echo $$; ls /proc | grep -c '^[0-9]'; id -u; ls /dev | tr '\\n' ' '; echo; cat /proc/sys/kernel/hostname; echo x > /dev/null && echo devnull";
    let (code, out) = run(&sandbox, Path::new("/"), script);
    let uid = unsafe { libc::getuid() };
    assert_eq!(code, 0, "{out}");
    let lines: Vec<_> = out.lines().collect();
    // `sh` runs under the sandbox's init (pid 1), alongside `ls` and `grep`.
    assert_eq!(lines[0], "2", "{out}");
    assert!(lines[1].parse::<u32>().unwrap() <= 4, "{out}");
    assert_eq!(lines[2], uid.to_string(), "the user keeps its id");
    for device in [
        "null", "zero", "urandom", "pts", "ptmx", "shm", "fd", "stdout",
    ] {
        assert!(
            lines[3].split(' ').any(|d| d == device),
            "{device} missing: {out}"
        );
    }
    assert!(
        !lines[3].contains("sda") && !lines[3].contains("kvm") && !lines[3].contains("nvme"),
        "{out}"
    );
    assert_eq!(&lines[4..], ["firebox", "devnull"]);
}

#[test]
fn exit_status_and_leftover_processes() {
    if !supported() {
        return;
    }
    let sandbox = Sandbox::new();
    assert_eq!(run(&sandbox, Path::new("/"), "exit 3").0, 3);
    assert_eq!(run(&sandbox, Path::new("/"), "kill -TERM $$").0, 128 + 15);
    // A background process does not outlive the command, nor keep it waiting.
    let started = Instant::now();
    let (code, _) = run(
        &sandbox,
        Path::new("/"),
        "sleep 30 > /dev/null 2>&1 & echo started",
    );
    assert_eq!(code, 0);
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn the_network_can_be_cut_off() {
    if !supported() {
        return;
    }
    let mut sandbox = Sandbox::new();
    sandbox.network(false);
    let (code, out) = run(
        &sandbox,
        Path::new("/"),
        "tail -n +3 /proc/net/dev | cut -d: -f1 | tr -d ' '",
    );
    assert_eq!((code, out.as_str()), (0, "lo\n"));
}

#[test]
fn a_hidden_working_directory_is_refused() {
    let dir = temp("cwd");
    let mut command = Command::new("/bin/true");
    command.current_dir(&dir);
    let err = Sandbox::new().apply(&mut command).err().unwrap();
    assert_eq!(
        err.to_string(),
        format!(
            "the working directory `{}` is hidden in the sandbox; expose it",
            dir.display()
        )
    );
}

#[test]
fn session_variables_are_removed() {
    if !supported() {
        return;
    }
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "echo ${SSH_AUTH_SOCK:-none} $KEPT"])
        .current_dir("/")
        .env("SSH_AUTH_SOCK", "/run/user/1/ssh")
        .env("KEPT", "yes");
    let session = Sandbox::new().apply(&mut command).unwrap();
    let output = command.output().unwrap();
    session.finish().unwrap();
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "none yes\n");
}

#[test]
fn setup_failures_name_the_step() {
    if !supported() {
        return;
    }
    let mut sandbox = Sandbox::new();
    sandbox.hostname("x".repeat(100));
    let mut command = Command::new("/bin/true");
    command.current_dir("/");
    let session = sandbox.apply(&mut command).unwrap();
    let status = command.status().unwrap();
    let err = session.finish().unwrap_err().to_string();
    assert_eq!(status.code(), Some(127));
    assert_eq!(
        err,
        "cannot create the sandbox: failed to set the hostname: Invalid argument (os error 22)"
    );
}

#[test]
fn programs_on_the_path_stay_available() {
    if !supported() {
        return;
    }
    let dir = temp("programs");
    let prefix = dir.join("home/me/.local");
    std::fs::create_dir_all(prefix.join("bin")).unwrap();
    std::fs::create_dir_all(prefix.join("lib")).unwrap();
    std::fs::create_dir_all(prefix.join("share/keyrings")).unwrap();
    std::fs::write(prefix.join("lib/greeting"), "hello").unwrap();
    let tool = prefix.join("bin/tool");
    std::fs::write(
        &tool,
        "#!/bin/sh\ncat \"$(dirname \"$0\")/../lib/greeting\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&tool, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let path = std::env::join_paths([
        prefix.join("bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ])
    .unwrap();
    let mut sandbox = Sandbox::new();
    sandbox.expose_programs(&path);
    let script = format!(
        "PATH={}; tool; test -e {}/share/keyrings || echo ' no keyrings'",
        path.to_string_lossy(),
        prefix.display()
    );
    let (code, out) = run(&sandbox, Path::new("/"), &script);
    assert_eq!((code, out.as_str()), (0, "hello no keyrings\n"));
}
