use nux_lexer::Span;

/// An identifier together with where it was written.
#[derive(Debug, Clone, PartialEq)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

/// `program <name> { ... }`; the span covers the keyword through the closing brace.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub name: Ident,
    pub statements: Vec<Statement>,
    pub span: Span,
}

/// A brace-delimited list of statements; the span includes both braces.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub statements: Vec<Statement>,
    pub span: Span,
}

/// A statement; the span includes its terminating `;` or closing brace.
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StatementKind {
    Let {
        name: Ident,
        value: Expr,
    },
    Assign {
        name: Ident,
        value: Expr,
    },
    /// `else if` is represented as an else block containing a single `If`
    /// statement; that block's span is the nested statement's span.
    If {
        condition: Expr,
        then_branch: Block,
        else_branch: Option<Block>,
    },
    While {
        condition: Expr,
        body: Block,
    },
    /// The commands inside a `bash { ... }` block; the span includes the braces.
    Bash {
        commands: Vec<String>,
    },
    /// `return;`: stops evaluating the program.
    Return,
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    String(String),
    Number(f64),
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq, Hash)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    AndAnd,
    OrOr,
    And,
    Or,
    Equal,
    NotEqual,
    LessThan,
    GreaterThan,
    LessThanOrEqual,
    GreaterThanOrEqual,
}

#[derive(Debug, Clone, PartialEq, Hash)]
pub enum UnaryOperator {
    Negate,
    Not,
}

/// A call argument; for `name = value` the span covers both sides.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionArg {
    pub kind: FunctionArgKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FunctionArgKind {
    Value(Expr),
    Bind { name: Ident, value: Expr },
}

impl FunctionArg {
    pub fn value(&self) -> &Expr {
        match &self.kind {
            FunctionArgKind::Value(value) | FunctionArgKind::Bind { value, .. } => value,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Literal(Literal),
    Variable(String),
    MemberAccess {
        object: Box<Expr>,
        member: Ident,
    },
    /// The span of the call expression includes the parentheses.
    FunctionCall {
        callee: Ident,
        args: Vec<FunctionArg>,
    },

    BinaryOp {
        left: Box<Expr>,
        op: BinaryOperator,
        right: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOperator,
        expr: Box<Expr>,
    },
    /// A parenthesized expression; the span includes the parentheses.
    Group(Box<Expr>),
    /// `[a, b, c]`; the span includes the brackets.
    List(Vec<Expr>),
}
