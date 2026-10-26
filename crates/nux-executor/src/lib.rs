use std::{collections::HashMap, path::PathBuf};

pub use nux_firebox::{Access, Sandbox};

/// What a program printed and how it exited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    /// The exit code; `128 + signal` when a signal ended the program.
    pub status: i32,
}

impl Output {
    pub fn success(&self) -> bool {
        self.status == 0
    }
}

pub struct Executor {
    env: HashMap<String, String>,
    cwd: PathBuf,
    /// Commands run inside it when set.
    sandbox: Option<Sandbox>,
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor {
    pub fn new() -> Self {
        Self {
            env: std::env::vars().collect(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            sandbox: None,
        }
    }

    /// Runs every later command inside `sandbox`, or directly with `None`.
    pub fn set_sandbox(&mut self, sandbox: Option<Sandbox>) {
        self.sandbox = sandbox;
    }

    /// Runs `command` to completion inside the sandbox, if any. Failing to
    /// set the sandbox up is an error, like failing to start the command.
    fn sandboxed<T>(
        &self,
        command: &mut std::process::Command,
        run: impl FnOnce(&mut std::process::Command) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let Some(sandbox) = &self.sandbox else {
            return run(command);
        };
        let session = sandbox.apply(command)?;
        let result = run(command);
        session.finish()?;
        result
    }

    pub fn set_env(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.env.insert(key.into(), value.into());
    }

    pub fn set_cwd(&mut self, cwd: impl Into<PathBuf>) {
        self.cwd = cwd.into();
    }

    pub fn get_env(&self, key: &str) -> Option<&String> {
        self.env.get(key)
    }

    pub fn get_cwd(&self) -> &PathBuf {
        &self.cwd
    }

    /// Runs `command` with both stdout and stderr appended to `log`.
    pub fn execute_logged(&self, command: &str, log: &std::fs::File) -> Result<(), String> {
        let stdout = log
            .try_clone()
            .map_err(|e| format!("Failed to open log: {}", e))?;
        let stderr = log
            .try_clone()
            .map_err(|e| format!("Failed to open log: {}", e))?;
        let mut process = std::process::Command::new("sh");
        process
            .arg("-c")
            .arg(command)
            .envs(&self.env)
            .current_dir(&self.cwd)
            .stdin(std::process::Stdio::null())
            .stdout(stdout)
            .stderr(stderr);
        let status = self
            .sandboxed(&mut process, |p| p.status())
            .map_err(|e| format!("Failed to execute command: {}", e))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("Command exited with {}", status))
        }
    }

    /// Runs `program` with `args` directly, without a shell: arguments are
    /// passed as they are, and `program` is looked up on the `PATH` unless it
    /// contains a `/`. Only failing to start the program is an error; a
    /// non-zero exit is reported in [`Output::status`].
    pub fn run(&self, program: &str, args: &[String]) -> std::io::Result<Output> {
        use std::os::unix::process::ExitStatusExt;
        let mut process = std::process::Command::new(program);
        process
            .args(args)
            .env_clear()
            .envs(&self.env)
            .current_dir(&self.cwd)
            .stdin(std::process::Stdio::null());
        let output = self.sandboxed(&mut process, |p| p.output())?;
        let status = output
            .status
            .code()
            .or_else(|| output.status.signal().map(|signal| 128 + signal))
            .unwrap_or(-1);
        Ok(Output {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            status,
        })
    }

    pub fn execute(&self, command: &str) -> Result<String, String> {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd.envs(&self.env);
        cmd.current_dir(&self.cwd);
        self.sandboxed(&mut cmd, |c| c.output())
            .map_err(|e| format!("Failed to execute command: {}", e))
            .and_then(|output| {
                if output.status.success() {
                    String::from_utf8(output.stdout)
                        .map_err(|e| format!("Failed to parse output as UTF-8: {}", e))
                } else {
                    Err(format!(
                        "Command exited with status {}: {}",
                        output.status,
                        String::from_utf8_lossy(&output.stderr)
                    ))
                }
            })
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn runs_programs_without_a_shell() {
        let executor = Executor::new();
        let output = executor
            .run("printf", &args(&["%s|", "a b", "$HOME", "*"]))
            .unwrap();
        // No word splitting, expansion or globbing.
        assert_eq!(output.stdout, "a b|$HOME|*|");
        assert!(output.success());

        let output = executor
            .run("sh", &args(&["-c", "echo oops >&2; exit 3"]))
            .unwrap();
        assert_eq!(
            (output.stderr.as_str(), output.status, output.success()),
            ("oops\n", 3, false)
        );

        let output = executor.run("sh", &args(&["-c", "kill -TERM $$"])).unwrap();
        assert_eq!(output.status, 128 + 15);
    }

    #[test]
    fn uses_its_environment_and_directory() {
        let mut executor = Executor::new();
        executor.set_env("NUX_TEST_VALUE", "42");
        executor.set_cwd("/");
        let output = executor
            .run("sh", &args(&["-c", "echo $NUX_TEST_VALUE $(pwd)"]))
            .unwrap();
        assert_eq!(output.stdout, "42 /\n");
    }

    #[test]
    fn sandboxed_commands_cannot_write_the_host() {
        if nux_firebox::Sandbox::available().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("nux-executor-sandbox-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut sandbox = Sandbox::new();
        sandbox.expose(&dir, Access::ReadWrite);
        let mut executor = Executor::new();
        executor.set_cwd(&dir);
        executor.set_env("LC_ALL", "C");
        executor.set_sandbox(Some(sandbox));
        let output = executor
            .run("sh", &args(&["-c", "echo ok > out; touch /usr/nux-test"]))
            .unwrap();
        assert_eq!(output.status, 1);
        assert!(
            output.stderr.contains("Read-only file system"),
            "{}",
            output.stderr
        );
        assert_eq!(std::fs::read_to_string(dir.join("out")).unwrap(), "ok\n");
        assert_eq!(executor.execute("echo $$").unwrap(), "2\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failing_to_start_is_an_error() {
        let err = Executor::new()
            .run("definitely-not-a-program", &[])
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
        let err = Executor::new().run("/", &[]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    }
}
