//! One module per `nux` subcommand. Each writes to the output it is given,
//! so tests can run commands end to end and check what they print.

use std::io::Write;

use nux_build::{BuildError, Workspace, WorkspaceLock};

pub mod clean;
pub mod init;
pub mod install;
pub mod purge;
pub mod remove;
pub mod reset;
pub mod status;
pub mod sync;

/// Takes the workspace lock for a command that changes the workspace.
/// Errors are ready to print; a busy workspace suggests `--wait`.
pub fn lock_workspace(workspace: &Workspace, wait: bool, out: &mut impl Write) -> Result<WorkspaceLock, String> {
    workspace
        .lock(wait, |pid| {
            let holder = pid.map_or(String::new(), |pid| format!(" (pid {pid})"));
            let _ = writeln!(out, "Waiting for another nux process{holder} to finish with the workspace...");
            let _ = out.flush();
        })
        .map_err(|e| match e {
            BuildError::Busy { .. } => format!("{e}; pass `--wait` to wait for it to finish"),
            e => e.to_string(),
        })
}
