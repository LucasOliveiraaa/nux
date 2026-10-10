//! `nux status`: what an environment requests and what the workspace has.
//! Nothing is written.

use std::io::Write;

use nux_build::{BuildPlan, Builder, LinkStatus, SourceStatus, Step, Workspace};
use nux_eval::Host;
use nux_log::{Style, Term};

use crate::config::Config;
use crate::environment::{Environment, Selection};
use crate::lockfile::Lock;
use crate::commands::sync::resolve_environment;
use crate::ui::{count, write_header};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    UpToDate,
    OutOfDate,
}

pub fn run(
    selection: &Selection,
    config: &Config,
    out: &mut impl Write,
    term: Term,
    workspace: &Workspace,
    host: &Host,
) -> Result<Outcome, String> {
    let w = |e: std::io::Error| format!("cannot write output: {e}");
    write_header(out, term).map_err(w)?;
    let env = Environment::select(selection, config)?;
    let global = if env.global { ", global" } else { "" };
    writeln!(out, "Environment: {} ({}{global})", term.paint(Style::Emphasis, &env.name), env.path.display()).map_err(w)?;
    let created = if workspace.exists() { "" } else { " (not created yet)" };
    writeln!(out, "Workspace:   {}{created}", workspace.home().display()).map_err(w)?;

    let lock_path = Lock::path(&env);
    let old_lock = Lock::read(&lock_path)?;
    let (packages, requested) = match resolve_environment(&env, host, old_lock.as_ref(), config.jobs) {
        Ok(resolved) => resolved,
        Err(error) => {
            // Still show what was asked for.
            writeln!(out, "\nRequested packages:").map_err(w)?;
            let rows = env.packages.iter().map(|r| vec![r.name.clone(), r.version.clone()]).collect();
            write_table(out, rows).map_err(w)?;
            return Err(error);
        }
    };
    let pins = old_lock.as_ref().map(|lock| lock.pins(&packages)).unwrap_or_default();
    let plan = Builder::new(workspace.home()).with_local_root(env.root()).with_pins(pins).plan(packages, &requested);
    let dropped = workspace.dropped_versions(&env.name, &plan).map_err(|e| e.to_string())?;
    let unused_links = workspace.links_into(&dropped, &plan);
    // What `nux sync` would write; status itself never writes it.
    let lock_change = match &old_lock {
        None => Some("it does not exist yet".to_string()),
        Some(old) => old.difference(&Lock::from_plan(&plan, workspace, Some(old))),
    };

    let requirement = |step: &Step| {
        env.packages
            .iter()
            .find(|r| r.name == step.package.name)
            .map_or(String::new(), |r| r.version.clone())
    };
    let row = |step: &Step, first: String| {
        vec![first, step.package.version.clone(), state(step, workspace, term)]
    };

    writeln!(out, "\nRequested packages:").map_err(w)?;
    let rows = plan.steps.iter().filter(|s| s.requested).map(|s| {
        let mut row = row(s, s.package.name.clone());
        row.insert(1, format!("{} ->", requirement(s)));
        row
    });
    write_table(out, rows.collect()).map_err(w)?;

    let dependencies: Vec<_> = plan.steps.iter().filter(|s| !s.requested).map(|s| row(s, s.package.name.clone())).collect();
    if !dependencies.is_empty() {
        writeln!(out, "\nDependencies:").map_err(w)?;
        write_table(out, dependencies).map_err(w)?;
    }

    if !plan.links.is_empty() || !unused_links.is_empty() {
        writeln!(out, "\nExecutable links:").map_err(w)?;
        let mut rows: Vec<_> = plan
            .links
            .iter()
            .map(|link| {
                let status = match &link.status {
                    LinkStatus::Current => term.paint(Style::Success, "linked"),
                    LinkStatus::Missing => term.paint(Style::Warning, "not linked"),
                    LinkStatus::Outdated => term.paint(Style::Warning, "points to another version"),
                    LinkStatus::Conflict(reason) => term.paint(Style::Error, format!("conflict: {reason}")),
                };
                vec![link.name.clone(), format!("-> {} {}", link.package, link.version), status]
            })
            .collect();
        rows.extend(unused_links.iter().map(|link| {
            let status = term.paint(Style::Warning, "to remove, no longer used");
            vec![link.name.clone(), format!("-> {} {}", link.package, link.version), status]
        }));
        rows.sort_by(|a, b| a[0].cmp(&b[0]));
        write_table(out, rows).map_err(w)?;
    }
    for warning in &plan.warnings {
        writeln!(out, "{} {warning}", term.paint(Style::Warning, "warning:")).map_err(w)?;
    }

    writeln!(out).map_err(w)?;
    let lock_name = lock_path.file_name().unwrap_or_default().to_string_lossy();
    match &lock_change {
        None => writeln!(out, "Lock file:   {lock_name} is up to date").map_err(w)?,
        Some(change) => writeln!(out, "Lock file:   {lock_name} needs updating: {change}").map_err(w)?,
    }

    if !dropped.is_empty() {
        // Versions the last sync needed that nothing needs any more.
        let list: Vec<_> = dropped.iter().map(|(name, version)| format!("{name} {version}")).collect();
        writeln!(out, "Unused:      {} (after `nux sync`, `nux purge` removes them)", list.join(", ")).map_err(w)?;
    }

    writeln!(out).map_err(w)?;
    if plan.is_up_to_date() && unused_links.is_empty() && lock_change.is_none() {
        writeln!(out, "{}", term.paint(Style::Success, "Environment is up to date.")).map_err(w)?;
        Ok(Outcome::UpToDate)
    } else {
        let mut work = pending(&plan, unused_links.len());
        if lock_change.is_some() {
            work = if work.is_empty() { format!("{lock_name} to update") } else { format!("{work}, {lock_name} to update") };
        }
        writeln!(out, "{} {work}.", term.paint(Style::Warning, "Environment is not synchronized:")).map_err(w)?;
        writeln!(out, "Run `nux sync` to update it.").map_err(w)?;
        Ok(Outcome::OutOfDate)
    }
}

fn state(step: &Step, workspace: &Workspace, term: Term) -> String {
    let mut state = if let (true, Some(path)) = (step.installed, step.host()) {
        term.paint(Style::Success, format!("provided by the host ({})", path.display()))
    } else if step.installed {
        term.paint(Style::Success, "installed")
    } else {
        let source = match step.source {
            SourceStatus::Missing => ", source not downloaded",
            SourceStatus::Cached => ", source downloaded",
            SourceStatus::None | SourceStatus::Local(_) => "",
        };
        term.paint(Style::Warning, format!("not built{source}"))
    };
    let others: Vec<_> = workspace
        .installed_versions(&step.package.name)
        .into_iter()
        .filter(|v| *v != step.package.version)
        .collect();
    if !others.is_empty() {
        state.push_str(&term.paint(Style::Dim, format!(" (also installed: {})", others.join(", "))));
    }
    state
}

fn pending(plan: &BuildPlan, unused_links: usize) -> String {
    let builds = plan.to_build().count();
    let links = plan.links.iter().filter(|l| l.status != LinkStatus::Current).count();
    let mut parts = Vec::new();
    if builds > 0 {
        parts.push(format!("{} to build", count(builds, "package")));
        parts.push(format!("{} to download", count(plan.downloads(), "source")));
    }
    if links > 0 {
        parts.push(format!("{} to update", count(links, "link")));
    }
    if unused_links > 0 {
        parts.push(format!("{} to remove", count(unused_links, "link")));
    }
    parts.join(", ")
}

/// Left-aligned columns; the last column is not padded.
fn write_table(out: &mut impl Write, rows: Vec<Vec<String>>) -> std::io::Result<()> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| rows.iter().filter_map(|r| r.get(c)).map(|s| visible_width(s)).max().unwrap_or(0))
        .collect();
    for row in rows {
        let mut line = String::from(" ");
        for (c, cell) in row.iter().enumerate() {
            line.push(' ');
            line.push_str(cell);
            if c + 1 < row.len() {
                line.push_str(&" ".repeat(widths[c] - visible_width(cell)));
            }
        }
        writeln!(out, "{line}")?;
    }
    Ok(())
}

/// Width without ANSI color codes.
fn visible_width(text: &str) -> usize {
    let mut width = 0;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            chars.by_ref().find(|&c| c == 'm');
        } else {
            width += 1;
        }
    }
    width
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_ignore_color_codes() {
        assert_eq!(visible_width("\x1b[32minstalled\x1b[0m"), 9);
        let mut out = Vec::new();
        let rows = vec![
            vec!["a".into(), "\x1b[2m1\x1b[0m".into(), "x".into()],
            vec!["long".into(), "22".into(), "y".into()],
        ];
        write_table(&mut out, rows).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "  a    \x1b[2m1\x1b[0m  x\n  long 22 y\n");
    }
}
