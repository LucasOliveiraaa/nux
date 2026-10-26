//! Rust-style rendering of error messages against source spans.
//!
//! Rendering is independent from error detection: any error that can produce
//! a message and a [`Span`] can be turned into a [`Diagnostic`].

use std::fmt::Write;

use crate::source::{SourceArena, SourceFile, Span};

const TAB_WIDTH: usize = 4;
/// Multiline spans longer than this are elided in the middle.
const MAX_LINES: usize = 6;

/// An error message attached to a source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub message: String,
    pub span: Span,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }

    pub fn render(&self, arena: &SourceArena) -> String {
        render_diagnostic(arena, &self.message, self.span)
    }
}

/// Renders `message` with the source excerpt covered by `span`:
///
/// ```text
/// error: expected `;`, found `}`
///  --> config.nux:3:14
///   |
/// 3 |     let x = 1
///   |              ^
/// ```
///
/// If the span cannot be resolved through `arena`, only the message (and the
/// path, when known) is rendered.
pub fn render_diagnostic(arena: &SourceArena, message: &str, span: Span) -> String {
    let mut out = format!("error: {message}\n");

    let Some(file) = arena.file_of(span) else {
        out.push_str(" --> <unknown source>\n");
        return out;
    };
    let (Some(begin), true) = (file.location(span.begin()), file.contains(span)) else {
        let _ = writeln!(out, " --> {}", file.path().display());
        return out;
    };

    let first = file.line_index(span.begin().offset());
    let mut last = file.line_index(span.end().offset());
    // A span ending right after a newline does not touch the next line.
    if last > first && file.line_range(last).unwrap().start == span.end().offset() {
        last -= 1;
    }

    let lines: Vec<usize> = if last - first + 1 > MAX_LINES {
        (first..first + MAX_LINES / 2)
            .chain(last + 1 - (MAX_LINES / 2 - 1)..=last)
            .collect()
    } else {
        (first..=last).collect()
    };
    let width = (lines.last().unwrap() + 1).to_string().len();
    let pad = " ".repeat(width);

    let _ = writeln!(
        out,
        "{pad}--> {}:{}:{}",
        file.path().display(),
        begin.line,
        begin.column
    );
    let _ = writeln!(out, "{pad} |");

    let mut previous = None;
    for &line in &lines {
        if previous.is_some_and(|p| p + 1 != line) {
            let _ = writeln!(out, "{pad} ...");
        }
        previous = Some(line);
        render_line(&mut out, file, span, line, width);
    }
    out
}

fn render_line(out: &mut String, file: &SourceFile, span: Span, line: usize, width: usize) {
    let range = file.line_range(line).unwrap();
    let text = &file.text()[range.clone()];
    let _ = writeln!(out, "{:>width$} | {}", line + 1, expand_tabs(text));

    let start = span.begin().offset().clamp(range.start, range.end) - range.start;
    let end = span.end().offset().clamp(range.start, range.end) - range.start;
    let col_start = display_width(&text[..start]);
    let mut carets = display_width(&text[start..end]);

    if carets == 0 {
        // Empty spans (and spans that only cover a line break) still get a
        // marker; blank interior lines of a multiline span do not.
        let interior = span.begin().offset() < range.start && span.end().offset() > range.end;
        if interior {
            return;
        }
        carets = 1;
    }

    let _ = writeln!(
        out,
        "{:width$} | {}{}",
        "",
        " ".repeat(col_start),
        "^".repeat(carets)
    );
}

fn expand_tabs(text: &str) -> String {
    text.replace('\t', &" ".repeat(TAB_WIDTH))
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|c| if c == '\t' { TAB_WIDTH } else { 1 })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::BytePos;

    fn setup(text: &str) -> (SourceArena, crate::source::SourceId) {
        let mut arena = SourceArena::new();
        let id = arena.add("test.nux", text);
        (arena, id)
    }

    fn span(arena: &SourceArena, id: crate::source::SourceId, needle: &str) -> Span {
        let file = arena.get(id).unwrap();
        let start = file.text().find(needle).unwrap();
        file.span(start..start + needle.len()).unwrap()
    }

    #[test]
    fn single_line() {
        let (arena, id) = setup("program p {\n    let x = 1\n}\n");
        let out = render_diagnostic(&arena, "bad value", span(&arena, id, "1"));
        assert_eq!(
            out,
            "error: bad value\n \
             --> test.nux:2:13\n  \
             |\n\
             2 |     let x = 1\n  \
             |             ^\n"
        );
    }

    #[test]
    fn multiline_span() {
        let (arena, id) = setup("a {\n  b\n}\nrest");
        let out = render_diagnostic(&arena, "block", span(&arena, id, "{\n  b\n}"));
        assert_eq!(
            out,
            "error: block\n \
             --> test.nux:1:3\n  \
             |\n\
             1 | a {\n  \
             |   ^\n\
             2 |   b\n  \
             | ^^^\n\
             3 | }\n  \
             | ^\n"
        );
    }

    #[test]
    fn long_multiline_span_is_elided() {
        let text = (1..=10)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (arena, id) = setup(&text);
        let file = arena.get(id).unwrap();
        let out = render_diagnostic(&arena, "long", file.span(0..file.len()).unwrap());
        assert!(out.contains(" 3 | l3\n"));
        assert!(!out.contains("l4\n"));
        assert!(out.contains("...\n"));
        assert!(out.contains("10 | l10\n"));
    }

    #[test]
    fn utf8_columns_and_carets() {
        let (arena, id) = setup("let é = \"ünï\";");
        let out = render_diagnostic(&arena, "utf8", span(&arena, id, "\"ünï\""));
        assert!(out.contains("--> test.nux:1:9\n"), "{out}");
        assert!(out.ends_with("  |         ^^^^^\n"), "{out}");
    }

    #[test]
    fn empty_span_and_eof() {
        let (arena, id) = setup("let x = 1");
        let file = arena.get(id).unwrap();
        let out = render_diagnostic(&arena, "expected `;`", Span::empty(file.end_pos()));
        assert!(out.contains("--> test.nux:1:10\n"), "{out}");
        assert!(out.ends_with("1 | let x = 1\n  |          ^\n"), "{out}");

        let (arena, id) = setup("");
        let file = arena.get(id).unwrap();
        let out = render_diagnostic(&arena, "empty", Span::empty(file.end_pos()));
        assert!(out.contains("--> test.nux:1:1\n"), "{out}");
        assert!(out.ends_with("1 | \n  | ^\n"), "{out}");
    }

    #[test]
    fn eof_after_trailing_newline() {
        let (arena, id) = setup("a\n");
        let file = arena.get(id).unwrap();
        let out = render_diagnostic(&arena, "eof", Span::empty(file.end_pos()));
        assert!(out.contains("--> test.nux:2:1\n"), "{out}");
    }

    #[test]
    fn tabs_are_expanded() {
        let (arena, id) = setup("\tx;");
        let out = render_diagnostic(&arena, "tab", span(&arena, id, "x"));
        assert!(out.ends_with("1 |     x;\n  |     ^\n"), "{out}");
    }

    #[test]
    fn unresolvable_span() {
        let (arena, id) = setup("é");
        // Splits the two-byte character.
        let bad = Span::empty(BytePos::new(id, 1));
        let out = render_diagnostic(&arena, "bad", bad);
        assert_eq!(out, "error: bad\n --> test.nux\n");

        let other = SourceArena::new();
        let out = render_diagnostic(&other, "missing", bad);
        assert_eq!(out, "error: missing\n --> <unknown source>\n");
    }
}
