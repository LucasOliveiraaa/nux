//! `nux clean`: show what the workspace can do without, then remove it.
//! The safeguards themselves live in `nux-build`.

use std::io::{BufRead, Write};

use nux_build::{CleanOptions, Workspace};
use nux_log::{Style, Term};

use crate::commands::lock_workspace;
use crate::ui::{confirm, count, write_header, write_removals};

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub yes: bool,
    pub sources: bool,
    pub cache: bool,
    pub dry_run: bool,
    /// Wait for another process using the workspace instead of failing.
    pub wait: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Cleaned,
    NothingToClean,
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
        writeln!(out, "No Nux workspace found at `{}`; nothing to clean.", workspace.home().display()).map_err(w)?;
        return Ok(Outcome::NothingToClean);
    }
    writeln!(out, "Workspace: {}\n", workspace.home().display()).map_err(w)?;

    // Nothing may be built or linked while it is being cleaned.
    let _workspace_lock = lock_workspace(workspace, options.wait, out)?;
    let items = workspace.clean_plan(CleanOptions { sources: options.sources, cache: options.cache }).map_err(|e| e.to_string())?;
    if items.is_empty() {
        writeln!(out, "{}", term.paint(Style::Success, "Nothing to clean.")).map_err(w)?;
        return Ok(Outcome::NothingToClean);
    }

    write_removals(out, &items, workspace).map_err(w)?;
    if !options.sources {
        writeln!(out, "Installed packages and downloaded sources are kept; pass `--sources` to remove sources too.")
            .map_err(w)?;
    }
    writeln!(out, "Installed packages no environment uses are removed by `nux purge`.").map_err(w)?;

    if options.dry_run {
        writeln!(out, "\nDry run: nothing was removed.").map_err(w)?;
        return Ok(Outcome::DryRun);
    }
    if !options.yes && !confirm(out, input)? {
        writeln!(out, "\nClean cancelled; nothing was removed.").map_err(w)?;
        return Ok(Outcome::Cancelled);
    }

    let freed = workspace.clean(&items).map_err(|e| e.to_string())?;
    writeln!(
        out,
        "\n  {} {}, freed {}.",
        term.paint(Style::Success, "Removed"),
        count(items.len(), "item"),
        nux_log::size(freed as f64)
    )
    .map_err(w)?;
    Ok(Outcome::Cleaned)
}
