//! How `nux sync` presents build progress, on top of `nux-log`.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::Mutex;

use nux_build::{BuildError, CleanItem, Progress, Reporter, Step, Workspace};
use nux_log::{MultiProgress, Style, Term};
use nux_resolver::PackageVersionId;

const BAR_WIDTH: usize = 30;

/// A clonable handle to stdout, so several writers can share it.
#[derive(Clone, Copy)]
pub struct Stdout;

impl Write for Stdout {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        std::io::stdout().write(data)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::stdout().flush()
    }
}

/// The `Nux v0.1.0` banner every command starts with.
pub fn write_header(out: &mut impl Write, term: Term) -> std::io::Result<()> {
    writeln!(out, "{}\n", term.paint(Style::Emphasis, format!("Nux v{}", env!("CARGO_PKG_VERSION"))))
}

/// Asks `Continue? [Y/n]`; an empty answer means yes.
pub fn confirm(out: &mut impl Write, input: &mut impl BufRead) -> Result<bool, String> {
    let w = |e: std::io::Error| format!("cannot write output: {e}");
    loop {
        write!(out, "\nContinue? [Y/n] ").map_err(w)?;
        out.flush().map_err(w)?;
        let mut answer = String::new();
        let read = input.read_line(&mut answer).map_err(|e| format!("cannot read the answer: {e}"))?;
        if read == 0 {
            return Err("no answer to the confirmation prompt; pass `--yes` to sync without asking".into());
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "" | "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => writeln!(out, "Please answer `y` or `n`.").map_err(w)?,
        }
    }
}

/// `n thing` or `n things`.
pub fn count(n: usize, thing: &str) -> String {
    if n == 1 { format!("{n} {thing}") } else { format!("{n} {thing}s") }
}

/// Writes `name version` rows with aligned versions.
pub fn write_packages<'a>(out: &mut impl Write, packages: impl IntoIterator<Item = (&'a str, &'a str)>) -> std::io::Result<()> {
    let packages: Vec<_> = packages.into_iter().collect();
    let width = packages.iter().map(|(name, _)| name.chars().count()).max().unwrap_or(0);
    for (name, version) in packages {
        writeln!(out, "  {name:<width$} {version}")?;
    }
    Ok(())
}

/// Lists what `clean` or `purge` will remove, with sizes and a total.
pub fn write_removals(out: &mut impl Write, items: &[CleanItem], workspace: &Workspace) -> std::io::Result<()> {
    let describe = |item: &CleanItem| match &item.package {
        Some((name, version)) => format!("{name} {version}"),
        None => item.path.strip_prefix(workspace.home()).unwrap_or(&item.path).display().to_string(),
    };
    let kind_width = items.iter().map(|i| i.kind.to_string().len()).max().unwrap_or(0);
    let what_width = items.iter().map(|i| describe(i).chars().count()).max().unwrap_or(0);
    writeln!(out, "The following will be removed:")?;
    for item in items {
        writeln!(
            out,
            "  {:<kind_width$} {:<what_width$} {}",
            item.kind.to_string(),
            describe(item),
            nux_log::size(item.bytes as f64)
        )?;
    }
    let total: u64 = items.iter().map(|i| i.bytes).sum();
    writeln!(out, "\n{}, {}.", count(items.len(), "item"), nux_log::size(total as f64))
}

pub struct SyncReporter<W: Write + Send> {
    term: Term,
    /// Width of the longest package name, for alignment.
    width: usize,
    downloads: MultiProgress<W>,
    rows: Mutex<HashMap<PackageVersionId, (usize, Progress)>>,
    /// Several builds can run at once: one row each.
    builds: MultiProgress<W>,
    build_rows: Mutex<HashMap<PackageVersionId, usize>>,
}

impl<W: Write + Send> SyncReporter<W> {
    pub fn new<'a>(term: Term, names: impl IntoIterator<Item = &'a str>, downloads: W, builds: W) -> Self {
        Self {
            term,
            width: names.into_iter().map(|n| n.chars().count()).max().unwrap_or(0),
            downloads: MultiProgress::new(downloads, term.animate),
            rows: Mutex::new(HashMap::new()),
            builds: MultiProgress::new(builds, term.animate),
            build_rows: Mutex::new(HashMap::new()),
        }
    }

    fn download_label(&self, step: &Step) -> String {
        format!("  {} {:<w$}", self.term.paint(Style::Progress, "Downloading"), step.package.name, w = self.width)
    }

    fn download_row(&self, step: &Step, progress: Progress, done: bool) -> String {
        let fraction = progress.fraction.unwrap_or(if done { 1.0 } else { 0.0 });
        let fraction = if done { 1.0 } else { fraction };
        let bar = nux_log::bar(fraction, BAR_WIDTH);
        let bar = if done { self.term.paint(Style::Success, bar) } else { bar };
        let mut row = format!("{} {bar} {:>3.0}%", self.download_label(step), fraction * 100.0);
        if let Some(rate) = progress.bytes_per_second.filter(|r| *r > 0.0) {
            row.push(' ');
            row.push_str(&nux_log::rate(rate));
        }
        row
    }

    fn build_label(&self, step: &Step) -> String {
        let name = format!("{}...", step.package.name);
        format!("  {} {name:<w$}", self.term.paint(Style::Progress, "Building"), w = self.width + 3)
    }
}

impl<W: Write + Send> Reporter for SyncReporter<W> {
    fn download_skipped(&self, step: &Step) {
        let row = self.downloads.add(String::new());
        self.downloads.finish(
            row,
            format!("{} {}", self.download_label(step), self.term.paint(Style::Dim, "Already downloaded")),
        );
    }

    fn download_cached(&self, step: &Step) {
        let rows = self.rows.lock().unwrap();
        let Some(&(row, _)) = rows.get(&step.package.id) else { return };
        self.downloads.finish(row, format!("{} {}", self.download_label(step), self.term.paint(Style::Dim, "From the cache")));
    }

    fn download_queued(&self, step: &Step) {
        let progress = Progress::default();
        let row = self.downloads.add(self.download_row(step, progress, false));
        self.rows.lock().unwrap().insert(step.package.id, (row, progress));
    }

    fn download_progress(&self, step: &Step, progress: Progress) {
        let mut rows = self.rows.lock().unwrap();
        let Some((row, last)) = rows.get_mut(&step.package.id) else { return };
        *last = progress;
        self.downloads.update(*row, self.download_row(step, progress, false));
    }

    fn download_finished(&self, step: &Step, result: Result<(), &BuildError>) {
        let rows = self.rows.lock().unwrap();
        let Some(&(row, progress)) = rows.get(&step.package.id) else { return };
        let text = match result {
            Ok(()) => self.download_row(step, progress, true),
            Err(_) => format!("{} {}", self.download_label(step), self.term.paint(Style::Error, "Failed")),
        };
        self.downloads.finish(row, text);
    }

    fn build_skipped(&self, step: &Step) {
        let status = if step.host().is_some() { "Provided by the host" } else { "Already built" };
        let row = self.builds.add(String::new());
        self.builds.finish(row, format!("{} {}", self.build_label(step), self.term.paint(Style::Dim, status)));
    }

    fn build_started(&self, step: &Step) {
        let row = self.builds.add(self.build_label(step));
        self.build_rows.lock().unwrap().insert(step.package.id, row);
    }

    fn build_finished(&self, step: &Step, result: Result<(), &BuildError>) {
        let Some(row) = self.build_rows.lock().unwrap().remove(&step.package.id) else { return };
        let status = match result {
            Ok(()) => self.term.paint(Style::Success, "Done!"),
            Err(_) => self.term.paint(Style::Error, "Failed"),
        };
        self.builds.finish(row, format!("{} {status}", self.build_label(step)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts() {
        assert_eq!(count(1, "package"), "1 package");
        assert_eq!(count(0, "source"), "0 sources");
        assert_eq!(count(8, "package"), "8 packages");
    }
}
