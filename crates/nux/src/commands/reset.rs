//! `nux reset`: remove everything Nux put in the workspace. Environments,
//! their `Nux.lock` and the configuration are not part of it and stay.

use std::io::{BufRead, Write};

use nux_build::{ResetOptions, Workspace};
use nux_log::{Style, Term};

use crate::commands::lock_workspace;
use crate::ui::{confirm, count, write_header, write_removals};

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub yes: bool,
    pub keep_cache: bool,
    pub dry_run: bool,
    /// Wait for another process using the workspace instead of failing.
    pub wait: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Reset,
    NothingToReset,
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
    if !workspace.home().exists() {
        writeln!(
            out,
            "No Nux workspace found at `{}`; nothing to reset.",
            workspace.home().display()
        )
        .map_err(w)?;
        return Ok(Outcome::NothingToReset);
    }
    writeln!(out, "Workspace: {}\n", workspace.home().display()).map_err(w)?;

    // Nothing may be built or linked while the workspace goes away.
    let _workspace_lock = lock_workspace(workspace, options.wait, out)?;
    let items = workspace
        .reset_plan(ResetOptions {
            keep_cache: options.keep_cache,
        })
        .map_err(|e| e.to_string())?;
    let kept = workspace.reset_keeps();
    let write_kept = |out: &mut dyn Write| -> std::io::Result<()> {
        if !kept.is_empty() {
            writeln!(out, "\nKept, since Nux did not create them:")?;
            for path in &kept {
                writeln!(
                    out,
                    "  {}",
                    path.strip_prefix(workspace.home())
                        .unwrap_or(path)
                        .display()
                )?;
            }
        }
        Ok(())
    };
    if items.is_empty() {
        writeln!(out, "{}", term.paint(Style::Success, "Nothing to reset.")).map_err(w)?;
        write_kept(out).map_err(w)?;
        return Ok(Outcome::NothingToReset);
    }

    write_removals(out, &items, workspace).map_err(w)?;
    writeln!(
        out,
        "Every installed package and its links will be gone. Environments, their Nux.lock and the configuration are kept."
    )
    .map_err(w)?;
    if options.keep_cache {
        writeln!(
            out,
            "The download cache is kept: sources will not need downloading again."
        )
        .map_err(w)?;
    }
    write_kept(out).map_err(w)?;

    if options.dry_run {
        writeln!(out, "\nDry run: nothing was removed.").map_err(w)?;
        return Ok(Outcome::DryRun);
    }
    if !options.yes && !confirm(out, input)? {
        writeln!(out, "\nReset cancelled; nothing was removed.").map_err(w)?;
        return Ok(Outcome::Cancelled);
    }

    let freed = workspace.reset(&items).map_err(|e| e.to_string())?;
    writeln!(
        out,
        "\n  {} {}, freed {}.\n\nRun `nux sync` to build an environment again.",
        term.paint(Style::Success, "Removed"),
        count(items.len(), "item"),
        nux_log::size(freed as f64)
    )
    .map_err(w)?;
    Ok(Outcome::Reset)
}
