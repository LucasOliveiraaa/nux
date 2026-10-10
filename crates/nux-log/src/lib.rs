//! Terminal output for nux: consistent colors, progress bars and live
//! multi-row progress. Nothing here knows about packages or builds.
//!
//! Colors are disabled when the stream is not a terminal, when `NO_COLOR`
//! is set or when `TERM=dumb`; animations (in-place redraws) only run on a
//! terminal.

use std::io::{IsTerminal, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Completed operations.
    Success,
    Warning,
    Error,
    /// Work in progress: bars, "Downloading", "Building".
    Progress,
    /// Headings and names worth highlighting.
    Emphasis,
    /// Secondary information such as "Already built".
    Dim,
}

impl Style {
    fn code(self) -> &'static str {
        match self {
            Style::Success => "32",
            Style::Warning => "33",
            Style::Error => "1;31",
            Style::Progress => "36",
            Style::Emphasis => "1",
            Style::Dim => "2",
        }
    }
}

/// How a stream may be written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Term {
    pub color: bool,
    pub animate: bool,
}

impl Term {
    /// Plain output: no colors, no animation.
    pub const PLAIN: Term = Term { color: false, animate: false };

    pub fn stdout() -> Self {
        Self::detect(std::io::stdout().is_terminal())
    }

    pub fn stderr() -> Self {
        Self::detect(std::io::stderr().is_terminal())
    }

    fn detect(is_terminal: bool) -> Self {
        let dumb = std::env::var_os("TERM").is_some_and(|t| t == "dumb");
        let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        Self {
            color: is_terminal && !dumb && !no_color,
            animate: is_terminal && !dumb,
        }
    }

    pub fn paint(&self, style: Style, text: impl std::fmt::Display) -> String {
        if self.color {
            format!("\x1b[{}m{text}\x1b[0m", style.code())
        } else {
            text.to_string()
        }
    }
}

/// `[=========>          ]`, `width` characters between the brackets.
pub fn bar(fraction: f64, width: usize) -> String {
    let fraction = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
    let filled = (fraction * width as f64).round() as usize;
    let mut bar = "=".repeat(filled);
    if filled < width {
        bar.push('>');
        bar.push_str(&" ".repeat(width - filled - 1));
    }
    format!("[{bar}]")
}

/// `1.0 MB/s`, `512 B/s`.
pub fn rate(bytes_per_second: f64) -> String {
    format!("{}/s", size(bytes_per_second))
}

/// `1.0 MB`, `512 B`.
pub fn size(bytes: f64) -> String {
    const UNITS: [&str; 4] = ["B", "kB", "MB", "GB"];
    let mut value = bytes.max(0.0);
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `8.0s`, `1m 2s`, `1h 3m`.
pub fn duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{:.1}s", duration.as_secs_f64())
    } else if seconds < 3600 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    }
}

/// Pads `text` to `width` columns (by characters).
pub fn pad(text: &str, width: usize) -> String {
    format!("{text:<width$}")
}

/// A block of rows that several threads update concurrently.
///
/// When animating, all rows are drawn and redrawn in place. Otherwise only
/// finished rows are printed, one line each, in the order they finish.
pub struct MultiProgress<W: Write + Send> {
    inner: Mutex<Inner<W>>,
    animate: bool,
}

struct Inner<W> {
    out: W,
    rows: Vec<String>,
    /// How many rows are currently on screen.
    drawn: usize,
    last_draw: Option<Instant>,
}

/// Redraws caused by progress updates are limited to this rate.
const REDRAW_INTERVAL: Duration = Duration::from_millis(80);

impl<W: Write + Send> MultiProgress<W> {
    pub fn new(out: W, animate: bool) -> Self {
        Self {
            inner: Mutex::new(Inner { out, rows: Vec::new(), drawn: 0, last_draw: None }),
            animate,
        }
    }

    /// Adds a row and returns its index.
    pub fn add(&self, text: impl Into<String>) -> usize {
        let mut inner = self.inner.lock().unwrap();
        inner.rows.push(text.into());
        if self.animate {
            inner.redraw();
        }
        inner.rows.len() - 1
    }

    /// Replaces a row that is still in progress.
    pub fn update(&self, row: usize, text: impl Into<String>) {
        let mut inner = self.inner.lock().unwrap();
        inner.rows[row] = text.into();
        if self.animate && inner.last_draw.is_none_or(|t| t.elapsed() >= REDRAW_INTERVAL) {
            inner.redraw();
        }
    }

    /// Sets a row's final text.
    pub fn finish(&self, row: usize, text: impl Into<String>) {
        let mut inner = self.inner.lock().unwrap();
        inner.rows[row] = text.into();
        if self.animate {
            inner.redraw();
        } else {
            let line = inner.rows[row].clone();
            let _ = writeln!(inner.out, "{line}");
            let _ = inner.out.flush();
        }
    }

    pub fn into_inner(self) -> W {
        self.inner.into_inner().unwrap().out
    }
}

impl<W: Write> Inner<W> {
    fn redraw(&mut self) {
        let mut frame = String::new();
        if self.drawn > 0 {
            frame.push_str(&format!("\x1b[{}A", self.drawn));
        }
        for row in &self.rows {
            frame.push_str("\r\x1b[2K");
            frame.push_str(row);
            frame.push('\n');
        }
        let _ = self.out.write_all(frame.as_bytes());
        let _ = self.out.flush();
        self.drawn = self.rows.len();
        self.last_draw = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paints_only_with_color() {
        let color = Term { color: true, animate: true };
        assert_eq!(color.paint(Style::Success, "Done!"), "\x1b[32mDone!\x1b[0m");
        assert_eq!(Term::PLAIN.paint(Style::Error, "error"), "error");
    }

    #[test]
    fn bars() {
        assert_eq!(bar(0.0, 5), "[>    ]");
        assert_eq!(bar(0.5, 4), "[==> ]");
        assert_eq!(bar(1.0, 5), "[=====]");
        assert_eq!(bar(7.0, 3), "[===]");
        assert_eq!(bar(f64::NAN, 3), "[>  ]");
    }

    #[test]
    fn rates_and_durations() {
        assert_eq!(rate(512.0), "512 B/s");
        assert_eq!(rate(1_000_000.0), "1.0 MB/s");
        assert_eq!(rate(2_400_000.0), "2.4 MB/s");
        assert_eq!(size(0.0), "0 B");
        assert_eq!(size(15_200_000.0), "15.2 MB");
        assert_eq!(duration(Duration::from_millis(8000)), "8.0s");
        assert_eq!(duration(Duration::from_secs(62)), "1m 2s");
        assert_eq!(duration(Duration::from_secs(3780)), "1h 3m");
    }

    #[test]
    fn plain_progress_prints_finished_rows_only() {
        let progress = MultiProgress::new(Vec::new(), false);
        let a = progress.add("a 0%");
        let b = progress.add("b 0%");
        progress.update(a, "a 50%");
        progress.finish(b, "b done");
        progress.finish(a, "a done");
        assert_eq!(String::from_utf8(progress.into_inner()).unwrap(), "b done\na done\n");
    }

    #[test]
    fn animated_progress_redraws_in_place() {
        let progress = MultiProgress::new(Vec::new(), true);
        let a = progress.add("a");
        progress.add("b");
        progress.finish(a, "a done");
        let out = String::from_utf8(progress.into_inner()).unwrap();
        // The last frame moves up over both rows and rewrites them.
        assert!(out.ends_with("\x1b[2A\r\x1b[2Ka done\n\r\x1b[2Kb\n"), "{out:?}");
    }
}
