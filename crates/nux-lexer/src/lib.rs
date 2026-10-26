mod cache;
pub mod diagnostic;
pub mod source;
mod tokens;

pub use diagnostic::{Diagnostic, render_diagnostic};
pub use source::{BytePos, Location, SourceArena, SourceFile, SourceId, Span, SpanError};
pub use tokens::*;

use std::ops::Range;

use anyhow::Result;
use tfla::search::{EngineBuilder, SearchEngine, TokenizeError};

pub struct Lexer {
    engine: SearchEngine<TokenKind>,
}

/// An error found while lexing, located by `span`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    pub message: String,
    pub span: Span,
}

impl LexError {
    fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }

    pub fn diagnostic(&self) -> Diagnostic {
        Diagnostic::error(self.message.clone(), self.span)
    }

    pub fn render(&self, arena: &SourceArena) -> String {
        render_diagnostic(arena, &self.message, self.span)
    }
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LexError {}

/// A token from the underlying engine, before comments and whitespace are
/// removed. Ranges are absolute byte offsets into the file.
#[derive(Debug, Clone)]
struct RawToken {
    kind: TokenKind,
    range: Range<usize>,
}

impl Lexer {
    /// Builds the lexer from the token patterns, which takes about a
    /// second; [`Lexer::cached`] reuses one built earlier.
    pub fn new() -> Result<Self> {
        let mut engine = EngineBuilder::new();

        for kind in TokenKind::into_iter() {
            let id = engine.add_pattern(kind, kind.pattern());

            // Only newlines are registered with the engine (for line numbers in
            // its errors). Comment tokens must NOT be registered: the engine
            // would then drop them, and `lex` relies on raw tokens covering the
            // input contiguously to compute byte offsets. Comments are removed
            // in `lex` itself, which also keeps `bash { ... }` bodies verbatim.
            if kind == TokenKind::Newline {
                engine.nline_token(id);
            }
        }

        Ok(Self {
            engine: engine.build()?,
        })
    }

    /// Lexes `file`, dropping whitespace and comments. The returned tokens
    /// always end with a [`TokenKind::Eof`] token at the end of the file.
    pub fn lex(&self, file: &SourceFile) -> Result<Vec<Token>, LexError> {
        let text = file.text();
        let mut tokens = Vec::new();
        let mut raw = self.raw_tokens(file, 0)?;
        let mut idx = 0;

        let span = |range: Range<usize>| {
            file.span(range)
                .expect("lexer ranges lie on character boundaries")
        };
        let mut push = |kind: TokenKind, range: Range<usize>| {
            tokens.push(Token {
                kind,
                content: text[range.clone()].to_string(),
                span: span(range),
            })
        };

        while idx < raw.len() {
            let RawToken { kind, range } = raw[idx].clone();
            idx += 1;

            match kind {
                TokenKind::Whitespace | TokenKind::Newline => {}

                TokenKind::LCommentBlock => {
                    let body = range.end;
                    let Some(close) = text[body..].find("*/") else {
                        return Err(LexError::new("unterminated block comment", span(range)));
                    };
                    self.resync(file, &mut raw, &mut idx, body + close + 2)?;
                }
                TokenKind::LineComment => {}

                TokenKind::Any => {
                    let content = &text[range.clone()];
                    let message = if content == "\"" {
                        "unterminated string literal".to_string()
                    } else {
                        format!("unexpected character `{}`", content.escape_debug())
                    };
                    return Err(LexError::new(message, span(range)));
                }

                TokenKind::Bash => {
                    push(TokenKind::Bash, range);

                    while raw.get(idx).is_some_and(|t| {
                        matches!(t.kind, TokenKind::Whitespace | TokenKind::Newline)
                    }) {
                        idx += 1;
                    }
                    let Some(open) = raw.get(idx).filter(|t| t.kind == TokenKind::OBrace) else {
                        continue;
                    };
                    let open = open.range.clone();

                    // The body is shell code, not nux: keep it verbatim.
                    let Some(close) = find_bash_end(text, open.end) else {
                        return Err(LexError::new("unclosed bash block", span(open)));
                    };
                    push(TokenKind::OBrace, open.clone());
                    push(TokenKind::BashBody, open.end..close);
                    push(TokenKind::CBrace, close..close + 1);
                    self.resync(file, &mut raw, &mut idx, close + 1)?;
                }

                _ => push(kind, range),
            }
        }

        let eof = Span::empty(file.end_pos());
        tokens.push(Token {
            kind: TokenKind::Eof,
            content: String::new(),
            span: eof,
        });
        Ok(tokens)
    }

    /// Tokenizes `file` from byte `from` to the end, keeping every token.
    fn raw_tokens(&self, file: &SourceFile, from: usize) -> Result<Vec<RawToken>, LexError> {
        let rest = &file.text()[from..];
        let tokens = self.engine.tokenize(rest).map_err(|err| {
            let TokenizeError::UnknownContent { line, column } = err;
            let offset = from + offset_of(rest, line, column);
            let pos = file.pos(offset).unwrap_or(file.end_pos());
            LexError::new("unrecognized input", Span::empty(pos))
        })?;

        let mut offset = from;
        Ok(tokens
            .into_iter()
            .map(|token| {
                let start = offset;
                offset += token.content.len();
                RawToken {
                    kind: token.kind,
                    range: start..offset,
                }
            })
            .collect())
    }

    /// Moves `idx` to the first raw token starting at `at`. If a token
    /// straddles `at` (the skipped region ended mid-token, possibly inside the
    /// token just consumed), the rest of the file is re-tokenized from `at`.
    fn resync(
        &self,
        file: &SourceFile,
        raw: &mut Vec<RawToken>,
        idx: &mut usize,
        at: usize,
    ) -> Result<(), LexError> {
        let mut i = idx.saturating_sub(1);
        while raw.get(i).is_some_and(|t| t.range.end <= at) {
            i += 1;
        }
        if raw.get(i).is_some_and(|t| t.range.start < at) {
            *raw = self.raw_tokens(file, at)?;
            i = 0;
        }
        *idx = i;
        Ok(())
    }
}

/// Byte offset of the matching `}` for a bash block whose `{` ends at `start`.
/// Quotes, backslash escapes and `#` comments are skipped so braces inside
/// them do not count.
fn find_bash_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = start;

    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'\'' => i += 1 + text[i + 1..].find('\'')?,
            b'"' => {
                i += 1;
                while *bytes.get(i)? != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'#' if i == start || bytes[i - 1].is_ascii_whitespace() => {
                i += text[i..].find('\n')?;
            }
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some(i),
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}

/// Converts the engine's 0-based (line, byte column) to a byte offset in `text`.
fn offset_of(text: &str, line: usize, column: usize) -> usize {
    let line_start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
    let mut offset = (line_start + column).min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lexer() -> &'static Lexer {
        static LEXER: std::sync::OnceLock<Lexer> = std::sync::OnceLock::new();
        LEXER.get_or_init(|| Lexer::new().unwrap())
    }

    fn lex(text: &str) -> Result<Vec<Token>, LexError> {
        let mut arena = SourceArena::new();
        let id = arena.add("test.nux", text);
        lexer().lex(arena.get(id).unwrap())
    }

    fn kinds(text: &str) -> Vec<TokenKind> {
        lex(text).unwrap().into_iter().map(|t| t.kind).collect()
    }

    fn contents(text: &str) -> Vec<String> {
        lex(text).unwrap().into_iter().map(|t| t.content).collect()
    }

    #[test]
    fn keywords_win_over_identifiers() {
        use TokenKind::*;
        assert_eq!(
            kinds("program bash let if else while return true false programs bashful returned"),
            vec![
                Program, Bash, Let, If, Else, While, Return, True, False, Ident, Ident, Ident, Eof
            ]
        );
    }

    #[test]
    fn whitespace_and_newlines_are_skipped() {
        use TokenKind::*;
        assert_eq!(
            kinds("let x =\n\t1 ;\r\n"),
            vec![Let, Ident, Eq, Num, Semi, Eof]
        );
    }

    #[test]
    fn comments_are_skipped() {
        assert_eq!(
            contents("a // line comment\nb /* block\n comment */ c /*x*/d"),
            vec!["a", "b", "c", "d", ""]
        );
        // Block comments do not nest; `/*/` does not close itself.
        assert_eq!(contents("a /* /* */ b /*/ */ c"), vec!["a", "b", "c", ""]);
        // A block comment ending inside what would have been a string token.
        assert_eq!(contents("/* \" */ x \"y\""), vec!["x", "\"y\"", ""]);
        // Comment markers inside strings are text.
        assert_eq!(
            contents(r#""https://x" "/* no */""#),
            vec![r#""https://x""#, r#""/* no */""#, ""]
        );
    }

    #[test]
    fn double_minus_is_two_operators() {
        use TokenKind::*;
        // `--` used to start a comment, silently dropping the rest of the line.
        assert_eq!(kinds("a--b"), vec![Ident, Minus, Minus, Ident, Eof]);
        assert_eq!(
            kinds("f()-- 1;"),
            vec![Ident, OParen, CParen, Minus, Minus, Num, Semi, Eof]
        );
    }

    #[test]
    fn operators() {
        use TokenKind::*;
        assert_eq!(
            kinds("<= >= < > != == = + - * / % && || & | !"),
            vec![
                Le, Ge, Lt, Gt, NeEq, EqEq, Eq, Plus, Minus, Star, Slash, Percent, AndAnd, OrOr,
                And, Or, Bang, Eof
            ]
        );
    }

    #[test]
    fn bash_body_is_raw() {
        use TokenKind::*;
        let src = "bash {\n  ./bootstrap --prefix=${p} -- \"}\" '{' # {\n  make\n} x";
        let tokens = lex(src).unwrap();
        let kinds: Vec<_> = tokens.iter().map(|t| t.kind).collect();
        assert_eq!(kinds, vec![Bash, OBrace, BashBody, CBrace, Ident, Eof]);
        assert_eq!(
            tokens[2].content,
            "\n  ./bootstrap --prefix=${p} -- \"}\" '{' # {\n  make\n"
        );
    }

    #[test]
    fn spans_match_source() {
        let src = "let a = \"ü€\"; // é\n/* ö */x";
        let mut arena = SourceArena::new();
        let id = arena.add("test.nux", src);
        let tokens = lexer().lex(arena.get(id).unwrap()).unwrap();
        for token in &tokens {
            assert_eq!(arena.slice(token.span), Some(token.content.as_str()));
        }
        let eof = tokens.last().unwrap();
        assert!(eof.span.is_empty());
        assert_eq!(eof.span.begin().offset(), src.len());
    }

    #[test]
    fn errors_have_spans() {
        let err = lex("let x = \"abc;").unwrap_err();
        assert_eq!(err.message, "unterminated string literal");
        assert_eq!(err.span.range(), 8..9);

        let err = lex("a /* never closed").unwrap_err();
        assert_eq!(err.message, "unterminated block comment");
        assert_eq!(err.span.range(), 2..4);

        let err = lex("bash { echo").unwrap_err();
        assert_eq!(err.message, "unclosed bash block");
        assert_eq!(err.span.range(), 5..6);

        let err = lex("x = $y").unwrap_err();
        assert_eq!(err.message, "unexpected character `$`");
        assert_eq!(err.span.range(), 4..5);

        let err = lex("é = ¤").unwrap_err();
        assert_eq!(err.span.range(), 0..2);
    }

    #[test]
    fn numbers_are_ascii_only() {
        use TokenKind::*;
        assert_eq!(kinds("12 3.5 4."), vec![Num, Num, Num, Dot, Eof]);
        // Arabic-Indic digit three: not a number (Unicode `\d` used to accept it).
        assert!(lex("\u{663}").is_err());
    }

    #[test]
    fn multiline_string_keeps_offsets() {
        let tokens = lex("\"a\nb\" c").unwrap();
        assert_eq!(tokens[1].content, "c");
        assert_eq!(tokens[1].span.range(), 6..7);
    }
}
