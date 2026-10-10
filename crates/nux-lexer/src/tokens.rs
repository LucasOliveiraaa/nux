use crate::source::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// `/*`; the comment runs to the next `*/`, which the lexer finds by
    /// searching the text, so it has no token of its own.
    LCommentBlock,
    /// `//` to the end of the line.
    LineComment,

    /// (
    OParen,
    /// )
    CParen,
    /// [
    OBracket,
    /// ]
    CBracket,
    /// {
    OBrace,
    /// }
    CBrace,

    Semi,
    Comma,
    Dot,

    Str,
    Num,

    Le,
    Ge,
    Lt,
    Gt,
    NeEq,
    EqEq,
    Eq,

    Plus,
    Minus,
    Star,
    Slash,
    AndAnd,
    OrOr,
    And,
    Or,

    Percent,

    Bang,

    Let,
    If,
    Else,
    While,
    Return,
    Program,
    Bash,
    True,
    False,

    Ident,

    Whitespace,
    Newline,

    Any,

    /// Raw text between the braces of a `bash { ... }` block. Produced by the
    /// lexer directly, never by pattern matching.
    BashBody,
    /// End of input. Produced by the lexer directly, never by pattern matching.
    Eof,
}

impl TokenKind {
    /// Every kind that is matched by a pattern, in priority order: when two
    /// patterns match the same length, the one listed first wins. Keywords
    /// must therefore precede `Ident`, and `Any` must come last.
    pub fn into_iter() -> impl Iterator<Item = TokenKind> {
        use TokenKind::*;
        vec![
            LCommentBlock,
            LineComment,
            OParen,
            CParen,
            OBracket,
            CBracket,
            OBrace,
            CBrace,
            Semi,
            Comma,
            Dot,
            Str,
            Num,
            Le,
            Ge,
            Lt,
            Gt,
            NeEq,
            EqEq,
            Eq,
            Plus,
            Minus,
            Star,
            Slash,
            Percent,
            AndAnd,
            OrOr,
            And,
            Or,
            Bang,
            Let,
            If,
            Else,
            While,
            Return,
            Program,
            Bash,
            True,
            False,
            Ident,
            Whitespace,
            Newline,
            Any,
        ]
        .into_iter()
    }

    /// The regex for this kind. Synthesized kinds (`BashBody`, `Eof`) are not
    /// part of [`TokenKind::into_iter`] and use a pattern that never matches.
    pub fn pattern(&self) -> &'static str {
        match self {
            Self::LCommentBlock => r"/\*",
            Self::LineComment => r"//[^\n]*",
            Self::OParen => r"\(",
            Self::CParen => r"\)",
            Self::OBracket => r"\[",
            Self::CBracket => r"\]",
            Self::OBrace => r"\{",
            Self::CBrace => r"\}",
            Self::Semi => r";",
            Self::Comma => r",",
            Self::Dot => r"\.",
            Self::Str => r#""([^"\\]|\\.)*""#,
            // ASCII only: Unicode `\d` would accept digits `f64` cannot parse.
            Self::Num => r"[0-9]+(\.[0-9]+)?",
            Self::Le => r"<=",
            Self::Ge => r">=",
            Self::Lt => r"<",
            Self::Gt => r">",
            Self::NeEq => r"!=",
            Self::EqEq => r"==",
            Self::Eq => r"=",
            Self::Plus => r"\+",
            Self::Minus => r"-",
            Self::Star => r"\*",
            Self::Slash => r"/",
            Self::Percent => r"%",
            Self::AndAnd => r"&&",
            Self::OrOr => r"\|\|",
            Self::And => r"&",
            Self::Or => r"\|",
            Self::Bang => r"!",
            Self::Let => r"let",
            Self::If => r"if",
            Self::Else => r"else",
            Self::While => r"while",
            Self::Return => r"return",
            Self::Program => r"program",
            Self::Bash => r"bash",
            Self::True => r"true",
            Self::False => r"false",
            Self::Ident => r"[a-zA-Z_][a-zA-Z0-9_]*",
            Self::Whitespace => r"[ \t\r]+",
            Self::Newline => r"\n",
            Self::Any => r".",
            Self::BashBody | Self::Eof => r"[^\s\S]",
        }
    }
}

impl From<TokenKind> for String {
    fn from(kind: TokenKind) -> Self {
        match kind {
            TokenKind::LCommentBlock => "/*".to_string(),
            TokenKind::LineComment => "//".to_string(),

            TokenKind::OParen => "(".to_string(),
            TokenKind::CParen => ")".to_string(),
            TokenKind::OBracket => "[".to_string(),
            TokenKind::CBracket => "]".to_string(),
            TokenKind::OBrace => "{".to_string(),
            TokenKind::CBrace => "}".to_string(),

            TokenKind::Semi => ";".to_string(),
            TokenKind::Comma => ",".to_string(),
            TokenKind::Dot => ".".to_string(),

            TokenKind::Str => "string literal".to_string(),
            TokenKind::Num => "number literal".to_string(),

            TokenKind::Le => "<=".to_string(),
            TokenKind::Ge => ">=".to_string(),
            TokenKind::Lt => "<".to_string(),
            TokenKind::Gt => ">".to_string(),
            TokenKind::NeEq => "!=".to_string(),
            TokenKind::EqEq => "==".to_string(),
            TokenKind::Eq => "=".to_string(),
            TokenKind::Plus => "+".to_string(),
            TokenKind::Minus => "-".to_string(),
            TokenKind::Star => "*".to_string(),
            TokenKind::Slash => "/".to_string(),
            TokenKind::Percent => "%".to_string(),
            TokenKind::AndAnd => "&&".to_string(),
            TokenKind::OrOr => "||".to_string(),
            TokenKind::And => "&".to_string(),
            TokenKind::Or => "|".to_string(),
            TokenKind::Bang => "!".to_string(),

            TokenKind::Let => "let".to_string(),
            TokenKind::If => "if".to_string(),
            TokenKind::Else => "else".to_string(),
            TokenKind::While => "while".to_string(),
            TokenKind::Return => "return".to_string(),

            TokenKind::Ident => "identifier".to_string(),
            TokenKind::Program => "program".to_string(),
            TokenKind::Bash => "bash".to_string(),
            TokenKind::True => "true".to_string(),
            TokenKind::False => "false".to_string(),

            TokenKind::Whitespace => "whitespace".to_string(),
            TokenKind::Newline => "newline".to_string(),
            TokenKind::Any => "any".to_string(),
            TokenKind::BashBody => "bash body".to_string(),
            TokenKind::Eof => "end of input".to_string(),
        }
    }
}

impl From<String> for TokenKind {
    fn from(s: String) -> Self {
        match s.as_str() {
            "/*" => TokenKind::LCommentBlock,
            "//" => TokenKind::LineComment,

            "(" => TokenKind::OParen,
            ")" => TokenKind::CParen,
            "[" => TokenKind::OBracket,
            "]" => TokenKind::CBracket,
            "{" => TokenKind::OBrace,
            "}" => TokenKind::CBrace,

            ";" => TokenKind::Semi,
            "," => TokenKind::Comma,
            "." => TokenKind::Dot,

            "<=" => TokenKind::Le,
            ">=" => TokenKind::Ge,
            "<" => TokenKind::Lt,
            ">" => TokenKind::Gt,
            "!=" => TokenKind::NeEq,
            "==" => TokenKind::EqEq,
            "=" => TokenKind::Eq,
            "+" => TokenKind::Plus,
            "-" => TokenKind::Minus,
            "*" => TokenKind::Star,
            "/" => TokenKind::Slash,
            "%" => TokenKind::Percent,
            "&&" => TokenKind::AndAnd,
            "||" => TokenKind::OrOr,
            "&" => TokenKind::And,
            "|" => TokenKind::Or,
            "!" => TokenKind::Bang,

            "let" => TokenKind::Let,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "while" => TokenKind::While,
            "return" => TokenKind::Return,

            "identifier" => TokenKind::Ident,
            "program" => TokenKind::Program,
            "bash" => TokenKind::Bash,
            "true" => TokenKind::True,
            "false" => TokenKind::False,

            "whitespace" => TokenKind::Whitespace,
            "newline" => TokenKind::Newline,
            "any" => TokenKind::Any,
            "bash body" => TokenKind::BashBody,
            "end of input" => TokenKind::Eof,

            _ => panic!("Unknown token kind: {}", s),
        }
    }
}

impl std::fmt::Display for TokenKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from(*self))
    }
}

/// A lexed token and the source span it was read from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Token {
    pub kind: TokenKind,
    /// The exact source text of the token.
    pub content: String,
    pub span: Span,
}
