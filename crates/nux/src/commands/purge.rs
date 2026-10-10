//! `nux purge`: remove the package versions no environment uses, the links
//! into them, and the records of environments that no longer exist. The
//! safeguards live in `nux-build`.

use std::io::{BufRead, Write};

use nux_build::Workspace;
use nux_log::{Style, Term};

use crate::commands::lock_workspace;
use crate::ui::{confirm, count, write_header, write_removals};

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub yes: bool,
    pub dry_run: bool,
    /// Wait for another process using the workspace instead of failing.
    pub wait: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Purged,
    NothingToPurge,
    DryRun,
    Cancelled,
}

pub fn run(
    options: Options,
    out: &mut impl Write,
    input: &mut impl BufRead,
    term: Term,
    workspace: &Workspace,
) -> Result<Outcome, String> {
    let w = |e: std::io::Error| format!("cannot write output: {e}");
    write_header(out, term).map_err(w)?;
    if !workspace.exists() {
        writeln!(out, "No Nux workspace found at `{}`; nothing to purge.", workspace.home().display()).map_err(w)?;
        return Ok(Outcome::NothingToPurge);
    }
    writeln!(out, "Workspace: {}", workspace.home().display()).map_err(w)?;

    // Nothing may be synced while deciding what is unused.
    let _workspace_lock = lock_workspace(workspace, options.wait, out)?;
    let records = workspace.environment_records().map_err(|e| e.to_string())?;
    let kept: Vec<_> = records.iter().filter(|r| !r.is_stale()).map(|r| r.name.as_str()).collect();
    if kept.is_empty() {
        writeln!(out, "No environment has been synced in this workspace.\n").map_err(w)?;
    } else {
        writeln!(out, "Keeping what these environments use: {}\n", kept.join(", ")).map_err(w)?;
    }

    let items = workspace.purge_plan().map_err(|e| e.to_string())?;
    if items.is_empty() {
        writeln!(out, "{}", term.paint(Style::Success, "Nothing to purge.")).map_err(w)?;
        return Ok(Outcome::NothingToPurge);
    }
    write_removals(out, &items, workspace).map_err(w)?;

    if options.dry_run {
        writeln!(out, "\nDry run: nothing was removed.").map_err(w)?;
        return Ok(Outcome::DryRun);
    }
    if !options.yes && !confirm(out, input)? {
        writeln!(out, "\nPurge cancelled; nothing was removed.").map_err(w)?;
        return Ok(Outcome::Cancelled);
    }

    let freed = workspace.purge(&items).map_err(|e| e.to_string())?;
    writeln!(
        out,
        "\n  {} {}, freed {}.",
        term.paint(Style::Success, "Removed"),
        count(items.len(), "item"),
        nux_log::size(freed as f64)
    )
    .map_err(w)?;
    Ok(Outcome::Purged)
}
