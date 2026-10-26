//! Source file management: an arena of loaded files, byte positions that
//! remember which file they belong to, and half-open spans over them.

use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// Handle to a file registered in a [`SourceArena`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(u32);

impl SourceId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// A byte offset into a specific source file.
///
/// The offset may equal the file length, which denotes end-of-file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BytePos {
    file: SourceId,
    offset: usize,
}

impl BytePos {
    /// Creates a position without checking it against the file contents.
    /// Prefer [`SourceFile::pos`], which validates bounds and UTF-8 boundaries.
    pub fn new(file: SourceId, offset: usize) -> Self {
        Self { file, offset }
    }

    pub fn file(self) -> SourceId {
        self.file
    }

    pub fn offset(self) -> usize {
        self.offset
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanError {
    /// `begin` and `end` belong to different source files.
    CrossFile { begin: SourceId, end: SourceId },
    /// `begin` is after `end`.
    Inverted { begin: usize, end: usize },
}

impl fmt::Display for SpanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpanError::CrossFile { begin, end } => write!(
                f,
                "span crosses files (begin in file #{}, end in file #{})",
                begin.0, end.0
            ),
            SpanError::Inverted { begin, end } => {
                write!(f, "span begins after it ends ({begin} > {end})")
            }
        }
    }
}

impl std::error::Error for SpanError {}

/// A half-open byte range `begin..end` within a single source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    begin: BytePos,
    end: BytePos,
}

impl Span {
    /// Creates a span, rejecting cross-file and inverted ranges.
    pub fn new(begin: BytePos, end: BytePos) -> Result<Self, SpanError> {
        if begin.file != end.file {
            return Err(SpanError::CrossFile {
                begin: begin.file,
                end: end.file,
            });
        }
        if begin.offset > end.offset {
            return Err(SpanError::Inverted {
                begin: begin.offset,
                end: end.offset,
            });
        }
        Ok(Self { begin, end })
    }

    /// An empty span located at `pos`.
    pub fn empty(pos: BytePos) -> Self {
        Self {
            begin: pos,
            end: pos,
        }
    }

    pub fn begin(self) -> BytePos {
        self.begin
    }

    pub fn end(self) -> BytePos {
        self.end
    }

    pub fn file(self) -> SourceId {
        self.begin.file
    }

    pub fn range(self) -> Range<usize> {
        self.begin.offset..self.end.offset
    }

    pub fn len(self) -> usize {
        self.end.offset - self.begin.offset
    }

    pub fn is_empty(self) -> bool {
        self.begin.offset == self.end.offset
    }

    /// The smallest span covering both `self` and `other`.
    pub fn join(self, other: Span) -> Result<Span, SpanError> {
        Span::new(
            BytePos::new(self.file(), self.begin.offset.min(other.begin.offset)),
            BytePos::new(other.file(), self.end.offset.max(other.end.offset)),
        )
    }

    /// Empty span at the start of `self`.
    pub fn shrink_to_begin(self) -> Span {
        Span::empty(self.begin)
    }

    /// Empty span at the end of `self`.
    pub fn shrink_to_end(self) -> Span {
        Span::empty(self.end)
    }
}

/// A 1-based line and column. Columns count Unicode scalar values, not bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    pub line: usize,
    pub column: usize,
}

/// A source file registered in a [`SourceArena`].
#[derive(Debug)]
pub struct SourceFile {
    id: SourceId,
    path: PathBuf,
    text: String,
    /// Byte offset of the start of every line. Always starts with 0.
    line_starts: Vec<usize>,
}

impl SourceFile {
    fn new(id: SourceId, path: PathBuf, text: String) -> Self {
        let line_starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self {
            id,
            path,
            text,
            line_starts,
        }
    }

    pub fn id(&self) -> SourceId {
        self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// A position in this file, or `None` if `offset` is past the end or
    /// splits a UTF-8 character. `offset == len()` is the end-of-file position.
    pub fn pos(&self, offset: usize) -> Option<BytePos> {
        self.text
            .is_char_boundary(offset)
            .then(|| BytePos::new(self.id, offset))
    }

    pub fn start_pos(&self) -> BytePos {
        BytePos::new(self.id, 0)
    }

    pub fn end_pos(&self) -> BytePos {
        BytePos::new(self.id, self.text.len())
    }

    /// Span over `range`, validated like [`SourceFile::pos`].
    pub fn span(&self, range: Range<usize>) -> Option<Span> {
        Span::new(self.pos(range.start)?, self.pos(range.end)?).ok()
    }

    /// Whether `span` belongs to this file and lies on character boundaries.
    pub fn contains(&self, span: Span) -> bool {
        span.file() == self.id
            && self.text.is_char_boundary(span.begin.offset)
            && self.text.is_char_boundary(span.end.offset)
    }

    /// The text covered by `span`, if it is valid for this file.
    pub fn slice(&self, span: Span) -> Option<&str> {
        self.contains(span).then(|| &self.text[span.range()])
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// 0-based index of the line containing byte `offset`.
    pub fn line_index(&self, offset: usize) -> usize {
        self.line_starts.partition_point(|&start| start <= offset) - 1
    }

    /// Byte range of the 0-based line `index`, excluding its line terminator.
    pub fn line_range(&self, index: usize) -> Option<Range<usize>> {
        let start = *self.line_starts.get(index)?;
        let mut end = self
            .line_starts
            .get(index + 1)
            .map_or(self.text.len(), |&next| next - 1);
        if self.text[start..end].ends_with('\r') {
            end -= 1;
        }
        Some(start..end)
    }

    /// Text of the 0-based line `index`, without its line terminator.
    pub fn line_text(&self, index: usize) -> Option<&str> {
        self.line_range(index).map(|range| &self.text[range])
    }

    /// 1-based line and column of `pos`, or `None` if it is invalid for this file.
    pub fn location(&self, pos: BytePos) -> Option<Location> {
        if pos.file != self.id || !self.text.is_char_boundary(pos.offset) {
            return None;
        }
        let line = self.line_index(pos.offset);
        let line_start = self.line_starts[line];
        let column = self.text[line_start..pos.offset].chars().count() + 1;
        Some(Location {
            line: line + 1,
            column,
        })
    }
}

/// Owns every loaded source file. Files are addressed by [`SourceId`] and are
/// never removed, so ids and spans stay valid for the arena's lifetime.
#[derive(Debug, Default)]
pub struct SourceArena {
    files: Vec<SourceFile>,
}

impl SourceArena {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers in-memory source text under `path`.
    pub fn add(&mut self, path: impl Into<PathBuf>, text: impl Into<String>) -> SourceId {
        let id = SourceId(u32::try_from(self.files.len()).expect("too many source files"));
        self.files
            .push(SourceFile::new(id, path.into(), text.into()));
        id
    }

    /// Reads `path` from disk and registers it.
    pub fn load(&mut self, path: impl AsRef<Path>) -> std::io::Result<SourceId> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)?;
        Ok(self.add(path, text))
    }

    pub fn get(&self, id: SourceId) -> Option<&SourceFile> {
        self.files.get(id.index())
    }

    pub fn path(&self, id: SourceId) -> Option<&Path> {
        self.get(id).map(SourceFile::path)
    }

    /// The file `span` points into.
    pub fn file_of(&self, span: Span) -> Option<&SourceFile> {
        self.get(span.file())
    }

    pub fn slice(&self, span: Span) -> Option<&str> {
        self.file_of(span)?.slice(span)
    }

    pub fn location(&self, pos: BytePos) -> Option<Location> {
        self.get(pos.file())?.location(pos)
    }

    pub fn iter(&self) -> impl Iterator<Item = &SourceFile> {
        self.files.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_are_half_open() {
        let mut arena = SourceArena::new();
        let id = arena.add("a.nux", "let x = 1;");
        let file = arena.get(id).unwrap();
        let span = file.span(4..5).unwrap();
        assert_eq!(file.slice(span), Some("x"));
        assert_eq!(span.len(), 1);
    }

    #[test]
    fn empty_and_eof_spans() {
        let mut arena = SourceArena::new();
        let id = arena.add("a.nux", "ab\n");
        let file = arena.get(id).unwrap();
        let eof = Span::empty(file.end_pos());
        assert!(eof.is_empty());
        assert_eq!(file.slice(eof), Some(""));
        assert_eq!(
            file.location(eof.begin()),
            Some(Location { line: 2, column: 1 })
        );
        assert!(file.pos(4).is_none());
    }

    #[test]
    fn cross_file_and_inverted_spans_are_rejected() {
        let mut arena = SourceArena::new();
        let a = arena.add("a.nux", "abc");
        let b = arena.add("b.nux", "abc");
        assert_eq!(
            Span::new(BytePos::new(a, 0), BytePos::new(b, 1)),
            Err(SpanError::CrossFile { begin: a, end: b })
        );
        assert_eq!(
            Span::new(BytePos::new(a, 2), BytePos::new(a, 1)),
            Err(SpanError::Inverted { begin: 2, end: 1 })
        );
        let sa = Span::empty(BytePos::new(a, 0));
        let sb = Span::empty(BytePos::new(b, 0));
        assert!(sa.join(sb).is_err());
    }

    #[test]
    fn utf8_positions() {
        let mut arena = SourceArena::new();
        let id = arena.add("u.nux", "é = \"ü\";\nnext");
        let file = arena.get(id).unwrap();
        // 'é' is two bytes; offset 1 splits it.
        assert!(file.pos(1).is_none());
        assert!(file.span(0..1).is_none());
        let after = file.pos(2).unwrap();
        assert_eq!(file.location(after), Some(Location { line: 1, column: 2 }));
        let next = file.pos(file.text().find("next").unwrap()).unwrap();
        assert_eq!(file.location(next), Some(Location { line: 2, column: 1 }));
        // A span built by hand that splits a character is rejected on use.
        let bad = Span::new(BytePos::new(id, 0), BytePos::new(id, 1)).unwrap();
        assert_eq!(arena.slice(bad), None);
    }

    #[test]
    fn paths_resolve_through_arena() {
        let mut arena = SourceArena::new();
        let a = arena.add("dir/a.nux", "");
        let b = arena.add("b.nux", "x");
        assert_eq!(arena.path(a), Some(Path::new("dir/a.nux")));
        assert_eq!(arena.path(b), Some(Path::new("b.nux")));
        let span = arena.get(b).unwrap().span(0..1).unwrap();
        assert_eq!(arena.file_of(span).unwrap().path(), Path::new("b.nux"));
    }

    #[test]
    fn crlf_lines_exclude_terminator() {
        let mut arena = SourceArena::new();
        let id = arena.add("c.nux", "a\r\nb");
        let file = arena.get(id).unwrap();
        assert_eq!(file.line_text(0), Some("a"));
        assert_eq!(file.line_text(1), Some("b"));
    }
}
