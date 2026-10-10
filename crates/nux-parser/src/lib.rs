use nux_lexer::{Diagnostic, SourceArena, Span, Token, TokenKind, render_diagnostic};

use crate::ast::{FunctionArg, FunctionArgKind, Ident};

pub mod ast;

/// A syntax error located by `span`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl ParseError {
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

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ParseError {}

type Result<T> = std::result::Result<T, ParseError>;

/// Joins two spans produced from the same token stream.
fn join(begin: Span, end: Span) -> Span {
    begin
        .join(end)
        .expect("tokens of one stream come from one file")
}

/// How a token kind is named in "expected ..." messages.
fn describe_kind(kind: TokenKind) -> String {
    match kind {
        TokenKind::Ident | TokenKind::Str | TokenKind::Num | TokenKind::BashBody | TokenKind::Eof => {
            kind.to_string()
        }
        _ => format!("`{kind}`"),
    }
}

/// How a found token is named in error messages.
fn describe_token(token: &Token) -> String {
    match token.kind {
        TokenKind::Eof => "end of input".to_string(),
        TokenKind::Ident => format!("identifier `{}`", token.content),
        TokenKind::Str => "string literal".to_string(),
        TokenKind::Num => format!("number `{}`", token.content),
        TokenKind::BashBody => "bash body".to_string(),
        _ => format!("`{}`", token.content),
    }
}

/// Resolves backslash escapes in the body of a string literal. Unknown
/// escapes are kept verbatim.
fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

struct Parser<'a> {
    tokens: &'a [Token],
    position: usize,
}

impl<'a> Parser<'a> {
    pub fn new(tokens: &'a [Token]) -> Self {
        assert!(
            tokens.last().is_some_and(|t| t.kind == TokenKind::Eof),
            "token stream must end with an Eof token"
        );
        Self {
            tokens,
            position: 0,
        }
    }

    /// The current token. Past the end this keeps returning the `Eof` token.
    pub fn peek(&self) -> &'a Token {
        let last = self.tokens.len() - 1;
        &self.tokens[self.position.min(last)]
    }

    pub fn peek_kind(&self) -> TokenKind {
        self.peek().kind
    }

    fn peek_nth_kind(&self, n: usize) -> TokenKind {
        let last = self.tokens.len() - 1;
        self.tokens[(self.position + n).min(last)].kind
    }

    pub fn next(&mut self) -> &'a Token {
        let token = self.peek();
        if token.kind != TokenKind::Eof {
            self.position += 1;
        }
        token
    }

    pub fn expect(&mut self, expected_kind: TokenKind) -> Result<&'a Token> {
        let token = self.peek();
        if token.kind == expected_kind {
            return Ok(self.next());
        }
        Err(ParseError::new(
            format!(
                "expected {}, found {}",
                describe_kind(expected_kind),
                describe_token(token)
            ),
            token.span,
        ))
    }

    fn expect_ident(&mut self) -> Result<Ident> {
        let token = self.expect(TokenKind::Ident)?;
        Ok(Ident {
            name: token.content.clone(),
            span: token.span,
        })
    }

    pub fn is_kind(&self, kind: TokenKind) -> bool {
        self.peek_kind() == kind
    }

    pub fn parse_program(&mut self) -> Result<ast::Program> {
        let start = self.expect(TokenKind::Program)?.span;
        let name = self.expect_ident()?;
        let body = self.parse_block()?;
        self.expect(TokenKind::Eof)?;

        Ok(ast::Program {
            name,
            statements: body.statements,
            span: join(start, body.span),
        })
    }

    fn parse_block(&mut self) -> Result<ast::Block> {
        let open = self.expect(TokenKind::OBrace)?.span;

        let mut statements = Vec::new();
        while !self.is_kind(TokenKind::CBrace) && !self.is_kind(TokenKind::Eof) {
            statements.push(self.parse_statement()?);
        }
        let close = self.expect(TokenKind::CBrace)?.span;

        Ok(ast::Block {
            statements,
            span: join(open, close),
        })
    }

    fn parse_statement(&mut self) -> Result<ast::Statement> {
        match self.peek_kind() {
            TokenKind::Let => self.parse_let_statement(),
            TokenKind::If => self.parse_if_statement(),
            TokenKind::While => self.parse_while_statement(),
            TokenKind::Bash => self.parse_bash_statement(),
            TokenKind::Return => self.parse_return_statement(),
            TokenKind::Ident if self.peek_nth_kind(1) == TokenKind::Eq => {
                self.parse_assign_statement()
            }
            _ => self.parse_expression_statement(),
        }
    }

    fn parse_let_statement(&mut self) -> Result<ast::Statement> {
        let start = self.expect(TokenKind::Let)?.span;
        let name = self.expect_ident()?;
        self.expect(TokenKind::Eq)?;
        let value = self.parse_expression()?;
        let end = self.expect(TokenKind::Semi)?.span;

        Ok(ast::Statement {
            kind: ast::StatementKind::Let { name, value },
            span: join(start, end),
        })
    }

    fn parse_assign_statement(&mut self) -> Result<ast::Statement> {
        let name = self.expect_ident()?;
        self.expect(TokenKind::Eq)?;
        let value = self.parse_expression()?;
        let end = self.expect(TokenKind::Semi)?.span;

        Ok(ast::Statement {
            span: join(name.span, end),
            kind: ast::StatementKind::Assign { name, value },
        })
    }

    fn parse_if_statement(&mut self) -> Result<ast::Statement> {
        let start = self.expect(TokenKind::If)?.span;
        let condition = self.parse_expression()?;
        let then_branch = self.parse_block()?;

        let else_branch = if self.is_kind(TokenKind::Else) {
            self.next(); // consume 'else'
            if self.is_kind(TokenKind::If) {
                let nested = self.parse_if_statement()?;
                Some(ast::Block {
                    span: nested.span,
                    statements: vec![nested],
                })
            } else {
                Some(self.parse_block()?)
            }
        } else {
            None
        };

        let end = else_branch.as_ref().unwrap_or(&then_branch).span;
        Ok(ast::Statement {
            kind: ast::StatementKind::If {
                condition,
                then_branch,
                else_branch,
            },
            span: join(start, end),
        })
    }

    fn parse_while_statement(&mut self) -> Result<ast::Statement> {
        let start = self.expect(TokenKind::While)?.span;
        let condition = self.parse_expression()?;
        let body = self.parse_block()?;

        Ok(ast::Statement {
            span: join(start, body.span),
            kind: ast::StatementKind::While { condition, body },
        })
    }

    /// Splits the raw text inside a `bash { ... }` block into lines, trimming whitespace and ignoring empty lines.
    /// 
    /// This accounts for multi-line commands in this form:
    /// foo \
    ///     bar
    fn split_commands(&self, raw: &str) -> Vec<String> {
        let mut commands = Vec::new();
        let mut current_command = String::new();
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.ends_with('\\') {
                current_command.push_str(trimmed.trim_end_matches('\\').trim());
                current_command.push(' ');
            } else {
                current_command.push_str(trimmed);
                commands.push(current_command.clone());
                current_command.clear();
            }
        }
        if !current_command.is_empty() {
            commands.push(current_command);
        }
        commands
    }

    fn parse_bash_statement(&mut self) -> Result<ast::Statement> {
        let start = self.expect(TokenKind::Bash)?.span;
        self.expect(TokenKind::OBrace)?;
        let command = self.expect(TokenKind::BashBody)?.content.clone();
        let end = self.expect(TokenKind::CBrace)?.span;
        let commands = self.split_commands(&command);

        Ok(ast::Statement {
            kind: ast::StatementKind::Bash { commands },
            span: join(start, end),
        })
    }

    fn parse_return_statement(&mut self) -> Result<ast::Statement> {
        let start = self.expect(TokenKind::Return)?.span;
        if !self.is_kind(TokenKind::Semi) {
            // Recipes have no functions, so there is nothing to return a value to.
            return Err(ParseError::new(
                format!("expected `;` after `return` (it takes no value), found {}", describe_token(self.peek())),
                self.peek().span,
            ));
        }
        let end = self.expect(TokenKind::Semi)?.span;
        Ok(ast::Statement {
            kind: ast::StatementKind::Return,
            span: join(start, end),
        })
    }

    fn parse_expression_statement(&mut self) -> Result<ast::Statement> {
        let expr = self.parse_expression()?;
        let end = self.expect(TokenKind::Semi)?.span;
        Ok(ast::Statement {
            span: join(expr.span, end),
            kind: ast::StatementKind::Expr(expr),
        })
    }

    fn parse_expression(&mut self) -> Result<ast::Expr> {
        self.parse_binary(1)
    }

    fn parse_binary(&mut self, min_precedence: u8) -> Result<ast::Expr> {
        let mut left = self.parse_unary()?;

        while let Some(op) = self.peek_binary_operator() {
            let precedence = Self::precedence(&op);

            if precedence < min_precedence {
                break;
            }

            self.next(); // Consume the operator

            // +1 makes operators at the same precedence left-associative.
            let right = self.parse_binary(precedence + 1)?;

            left = ast::Expr {
                span: join(left.span, right.span),
                kind: ast::ExprKind::BinaryOp {
                    left: Box::new(left),
                    op,
                    right: Box::new(right),
                },
            };
        }

        Ok(left)
    }

    fn peek_binary_operator(&self) -> Option<ast::BinaryOperator> {
        use TokenKind as Tk;
        use ast::BinaryOperator as Op;

        match self.peek_kind() {
            Tk::Plus => Some(Op::Add),
            Tk::Minus => Some(Op::Subtract),
            Tk::Star => Some(Op::Multiply),
            Tk::Slash => Some(Op::Divide),
            Tk::Percent => Some(Op::Modulo),

            Tk::AndAnd => Some(Op::AndAnd),
            Tk::OrOr => Some(Op::OrOr),
            Tk::And => Some(Op::And),
            Tk::Or => Some(Op::Or),

            Tk::EqEq => Some(Op::Equal),
            Tk::NeEq => Some(Op::NotEqual),
            Tk::Lt => Some(Op::LessThan),
            Tk::Gt => Some(Op::GreaterThan),
            Tk::Le => Some(Op::LessThanOrEqual),
            Tk::Ge => Some(Op::GreaterThanOrEqual),

            _ => None,
        }
    }

    fn precedence(op: &ast::BinaryOperator) -> u8 {
        use ast::BinaryOperator::*;

        match op {
            OrOr => 1,
            AndAnd => 2,
            Or => 3,
            And => 4,
            Equal | NotEqual => 5,
            LessThan | GreaterThan | LessThanOrEqual | GreaterThanOrEqual => 6,
            Add | Subtract => 7,
            Multiply | Divide | Modulo => 8,
        }
    }

    fn parse_unary(&mut self) -> Result<ast::Expr> {
        let op = match self.peek_kind() {
            TokenKind::Minus => ast::UnaryOperator::Negate,
            TokenKind::Bang => ast::UnaryOperator::Not,
            _ => return self.parse_postfix(),
        };
        let start = self.next().span;
        let expr = self.parse_unary()?;

        Ok(ast::Expr {
            span: join(start, expr.span),
            kind: ast::ExprKind::UnaryOp {
                op,
                expr: Box::new(expr),
            },
        })
    }

    fn parse_function_arg(&mut self) -> Result<FunctionArg> {
        if self.is_kind(TokenKind::Ident) && self.peek_nth_kind(1) == TokenKind::Eq {
            let name = self.expect_ident()?;
            self.next(); // consume '='
            let value = self.parse_expression()?;
            return Ok(FunctionArg {
                span: join(name.span, value.span),
                kind: FunctionArgKind::Bind { name, value },
            });
        }

        let value = self.parse_expression()?;
        Ok(FunctionArg {
            span: value.span,
            kind: FunctionArgKind::Value(value),
        })
    }

    fn parse_postfix(&mut self) -> Result<ast::Expr> {
        let mut expr = self.parse_primary()?;

        loop {
            match self.peek_kind() {
                TokenKind::OParen => {
                    let callee = match expr.kind {
                        ast::ExprKind::Variable(name) => Ident {
                            name,
                            span: expr.span,
                        },
                        _ => {
                            return Err(ParseError::new(
                                "only named functions can be called",
                                expr.span,
                            ));
                        }
                    };
                    self.next(); // consume '('

                    let mut args = Vec::new();
                    while !self.is_kind(TokenKind::CParen) {
                        args.push(self.parse_function_arg()?);
                        if self.is_kind(TokenKind::CParen) {
                            break;
                        }
                        self.expect(TokenKind::Comma)?;
                    }
                    let end = self.expect(TokenKind::CParen)?.span;

                    expr = ast::Expr {
                        span: join(callee.span, end),
                        kind: ast::ExprKind::FunctionCall { callee, args },
                    };
                }
                TokenKind::Dot => {
                    self.next(); // consume '.'
                    let member = self.expect_ident()?;
                    expr = ast::Expr {
                        span: join(expr.span, member.span),
                        kind: ast::ExprKind::MemberAccess {
                            object: Box::new(expr),
                            member,
                        },
                    };
                }
                _ => return Ok(expr),
            }
        }
    }

    fn parse_primary(&mut self) -> Result<ast::Expr> {
        let token = self.peek();
        let kind = match token.kind {
            TokenKind::Num => {
                let num = token
                    .content
                    .parse::<f64>()
                    .map_err(|e| ParseError::new(format!("invalid number: {e}"), token.span))?;
                ast::ExprKind::Literal(ast::Literal::Number(num))
            }
            TokenKind::Str => {
                // Remove the surrounding quotes
                let raw = &token.content[1..token.content.len() - 1];
                ast::ExprKind::Literal(ast::Literal::String(unescape(raw)))
            }
            TokenKind::True => ast::ExprKind::Literal(ast::Literal::Boolean(true)),
            TokenKind::False => ast::ExprKind::Literal(ast::Literal::Boolean(false)),
            TokenKind::Ident => ast::ExprKind::Variable(token.content.clone()),
            TokenKind::OBracket => {
                self.next(); // consume '['
                let mut items = Vec::new();
                while !self.is_kind(TokenKind::CBracket) {
                    items.push(self.parse_expression()?);
                    if self.is_kind(TokenKind::CBracket) {
                        break;
                    }
                    self.expect(TokenKind::Comma)?;
                }
                let end = self.expect(TokenKind::CBracket)?.span;
                return Ok(ast::Expr {
                    kind: ast::ExprKind::List(items),
                    span: join(token.span, end),
                });
            }
            TokenKind::OParen => {
                self.next(); // consume '('
                let inner = self.parse_expression()?;
                let end = self.expect(TokenKind::CParen)?.span;
                return Ok(ast::Expr {
                    kind: ast::ExprKind::Group(Box::new(inner)),
                    span: join(token.span, end),
                });
            }
            _ => {
                return Err(ParseError::new(
                    format!("expected expression, found {}", describe_token(token)),
                    token.span,
                ));
            }
        };
        self.next();
        Ok(ast::Expr {
            kind,
            span: token.span,
        })
    }
}

/// Parses a token stream produced by [`nux_lexer::Lexer::lex`].
///
/// # Panics
/// If `tokens` does not end with a [`TokenKind::Eof`] token.
pub fn parse(tokens: &[Token]) -> std::result::Result<ast::Program, ParseError> {
    let mut parser = Parser::new(tokens);
    parser.parse_program()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ast::{BinaryOperator as Op, ExprKind, Literal, StatementKind};
    use nux_lexer::{Lexer, SourceArena, SourceId};
    use std::sync::OnceLock;

    fn lexer() -> &'static Lexer {
        static LEXER: OnceLock<Lexer> = OnceLock::new();
        LEXER.get_or_init(|| Lexer::new().unwrap())
    }

    fn setup(src: &str) -> (SourceArena, SourceId) {
        let mut arena = SourceArena::new();
        let id = arena.add("test.nux", src);
        (arena, id)
    }

    fn try_parse(arena: &SourceArena, id: SourceId) -> std::result::Result<ast::Program, ParseError> {
        let tokens = lexer().lex(arena.get(id).unwrap()).unwrap();
        parse(&tokens)
    }

    /// Parses `program p { <body> }` and returns the arena too, for span checks.
    fn parse_body(body: &str) -> (SourceArena, ast::Program) {
        let (arena, id) = setup(&format!("program p {{ {body} }}"));
        let program = try_parse(&arena, id).unwrap_or_else(|e| panic!("{}", e.render(&arena)));
        check_nesting(&program);
        (arena, program)
    }

    fn parse_err(src: &str) -> (String, std::ops::Range<usize>) {
        let (arena, id) = setup(src);
        let err = try_parse(&arena, id).unwrap_err();
        (err.message, err.span.range())
    }

    fn text(arena: &SourceArena, span: Span) -> &str {
        arena.slice(span).unwrap()
    }

    fn within(outer: Span, inner: Span) {
        assert!(
            outer.begin().offset() <= inner.begin().offset()
                && inner.end().offset() <= outer.end().offset(),
            "{inner:?} not within {outer:?}"
        );
    }

    /// Every child node's span must lie within its parent's span.
    fn check_nesting(program: &ast::Program) {
        within(program.span, program.name.span);
        for stmt in &program.statements {
            within(program.span, stmt.span);
            check_stmt(stmt);
        }
    }

    fn check_block(block: &ast::Block) {
        for stmt in &block.statements {
            within(block.span, stmt.span);
            check_stmt(stmt);
        }
    }

    fn check_stmt(stmt: &ast::Statement) {
        let expr = |e: &ast::Expr| {
            within(stmt.span, e.span);
            check_expr(e);
        };
        match &stmt.kind {
            StatementKind::Let { name, value } | StatementKind::Assign { name, value } => {
                within(stmt.span, name.span);
                expr(value);
            }
            StatementKind::If { condition, then_branch, else_branch } => {
                expr(condition);
                within(stmt.span, then_branch.span);
                check_block(then_branch);
                if let Some(e) = else_branch {
                    within(stmt.span, e.span);
                    check_block(e);
                }
            }
            StatementKind::While { condition, body } => {
                expr(condition);
                within(stmt.span, body.span);
                check_block(body);
            }
            StatementKind::Bash { .. } | StatementKind::Return => {}
            StatementKind::Expr(e) => expr(e),
        }
    }

    fn check_expr(expr: &ast::Expr) {
        let child = |e: &ast::Expr| {
            within(expr.span, e.span);
            check_expr(e);
        };
        match &expr.kind {
            ExprKind::Literal(_) | ExprKind::Variable(_) => {}
            ExprKind::MemberAccess { object, member } => {
                child(object);
                within(expr.span, member.span);
            }
            ExprKind::FunctionCall { callee, args } => {
                within(expr.span, callee.span);
                for arg in args {
                    within(expr.span, arg.span);
                    within(arg.span, arg.value().span);
                    check_expr(arg.value());
                }
            }
            ExprKind::BinaryOp { left, right, .. } => {
                child(left);
                child(right);
            }
            ExprKind::UnaryOp { expr: inner, .. } | ExprKind::Group(inner) => child(inner),
            ExprKind::List(items) => items.iter().for_each(child),
        }
    }

    fn only_expr(program: &ast::Program) -> &ast::Expr {
        match &program.statements[0].kind {
            StatementKind::Expr(e) | StatementKind::Let { value: e, .. } => e,
            other => panic!("unexpected statement {other:?}"),
        }
    }

    #[test]
    fn statement_spans_include_terminators() {
        let (arena, program) = parse_body("let x = 1 + 2; x = 3; f(a, b = 2); while x { } if a { } else if b { } else { }");
        let spans: Vec<_> = program.statements.iter().map(|s| text(&arena, s.span)).collect();
        assert_eq!(
            spans,
            [
                "let x = 1 + 2;",
                "x = 3;",
                "f(a, b = 2);",
                "while x { }",
                "if a { } else if b { } else { }",
            ]
        );
        assert_eq!(text(&arena, program.span), format!("program p {{ {} }}", spans.join(" ")));

        let StatementKind::If { then_branch, else_branch, .. } = &program.statements[4].kind else {
            panic!()
        };
        assert_eq!(text(&arena, then_branch.span), "{ }");
        let else_if = else_branch.as_ref().unwrap();
        assert_eq!(text(&arena, else_if.span), "if b { } else { }");
        assert!(matches!(else_if.statements[0].kind, StatementKind::If { .. }));
    }

    #[test]
    fn expression_spans() {
        let (arena, program) = parse_body("f(-(a + b) * c, name = nux.arch);");
        let call = only_expr(&program);
        assert_eq!(text(&arena, call.span), "f(-(a + b) * c, name = nux.arch)");
        let ExprKind::FunctionCall { callee, args } = &call.kind else { panic!() };
        assert_eq!(text(&arena, callee.span), "f");
        assert_eq!(text(&arena, args[0].span), "-(a + b) * c");
        assert_eq!(text(&arena, args[1].span), "name = nux.arch");

        let ExprKind::BinaryOp { left, .. } = &args[0].value().kind else { panic!() };
        assert_eq!(text(&arena, left.span), "-(a + b)");
        let ExprKind::UnaryOp { expr: group, .. } = &left.kind else { panic!() };
        assert_eq!(text(&arena, group.span), "(a + b)");

        let ast::FunctionArgKind::Bind { name, value } = &args[1].kind else { panic!() };
        assert_eq!(text(&arena, name.span), "name");
        let ExprKind::MemberAccess { member, .. } = &value.kind else { panic!() };
        assert_eq!(text(&arena, member.span), "arch");
    }

    #[test]
    fn chained_postfix() {
        let (arena, program) = parse_body("a.b.c;");
        let expr = only_expr(&program);
        assert_eq!(text(&arena, expr.span), "a.b.c");
        let ExprKind::MemberAccess { object, member } = &expr.kind else { panic!() };
        assert_eq!(member.name, "c");
        assert!(matches!(object.kind, ExprKind::MemberAccess { .. }));
    }

    #[test]
    fn precedence_and_associativity() {
        let (_, program) = parse_body("let x = 10 - 4 - 3 % 2 * 1;");
        // ((10 - 4) - ((3 % 2) * 1))
        let ExprKind::BinaryOp { left, op: Op::Subtract, right } = &only_expr(&program).kind else {
            panic!()
        };
        assert!(matches!(left.kind, ExprKind::BinaryOp { op: Op::Subtract, .. }));
        let ExprKind::BinaryOp { left: inner, op: Op::Multiply, .. } = &right.kind else { panic!() };
        assert!(matches!(inner.kind, ExprKind::BinaryOp { op: Op::Modulo, .. }));
    }

    #[test]
    fn literals() {
        let (_, program) = parse_body(r#"f("a\"b\n\q", true, false, 1.5);"#);
        let ExprKind::FunctionCall { args, .. } = &only_expr(&program).kind else { panic!() };
        let lits: Vec<_> = args
            .iter()
            .map(|a| match &a.value().kind {
                ExprKind::Literal(l) => l.clone(),
                _ => panic!(),
            })
            .collect();
        assert_eq!(
            lits,
            [
                Literal::String("a\"b\n\\q".into()),
                Literal::Boolean(true),
                Literal::Boolean(false),
                Literal::Number(1.5),
            ]
        );
    }

    #[test]
    fn list_literals() {
        let (arena, program) = parse_body(r#"let xs = ["a", 1 + 2, [], [x],];"#);
        check_nesting(&program);
        let expr = only_expr(&program);
        assert_eq!(text(&arena, expr.span), r#"["a", 1 + 2, [], [x],]"#);
        let ExprKind::List(items) = &expr.kind else { panic!("{expr:?}") };
        let texts: Vec<_> = items.iter().map(|i| text(&arena, i.span)).collect();
        assert_eq!(texts, [r#""a""#, "1 + 2", "[]", "[x]"]);

        let (message, _) = parse_err("program p { let xs = [1 2]; }");
        assert_eq!(message, "expected `,`, found number `2`");
    }

    #[test]
    fn return_statements() {
        let (arena, program) = parse_body("if x { return; } return;");
        check_nesting(&program);
        assert_eq!(text(&arena, program.statements[1].span), "return;");
        assert!(matches!(program.statements[1].kind, StatementKind::Return));

        let (message, range) = parse_err("program p { return 1; }");
        assert_eq!(message, "expected `;` after `return` (it takes no value), found number `1`");
        assert_eq!(range, 19..20);
    }

    #[test]
    fn trailing_comma_in_call() {
        let (_, program) = parse_body("f(1, 2,);");
        let ExprKind::FunctionCall { args, .. } = &only_expr(&program).kind else { panic!() };
        assert_eq!(args.len(), 2);
    }

    #[test]
    fn wrong_first_token_is_an_error_not_a_panic() {
        // `expect` used to compute `position - 1` and underflow here.
        assert_eq!(parse_err("x"), ("expected `program`, found identifier `x`".into(), 0..1));
    }

    #[test]
    fn errors_point_at_offending_token() {
        assert_eq!(parse_err("program p { let x = 1 }"), ("expected `;`, found `}`".into(), 22..23));
        assert_eq!(parse_err("program p { let x = 1;"), ("expected `}`, found end of input".into(), 22..22));
        assert_eq!(parse_err("program p { } extra"), ("expected end of input, found identifier `extra`".into(), 14..19));
        assert_eq!(parse_err("program p { ; }"), ("expected expression, found `;`".into(), 12..13));
        assert_eq!(parse_err("program p { a.b(); }"), ("only named functions can be called".into(), 12..15));
        assert_eq!(parse_err("program p { f(1 2); }"), ("expected `,`, found number `2`".into(), 16..17));
        assert_eq!(parse_err("program p { let = 1; }"), ("expected identifier, found `=`".into(), 16..17));
        assert_eq!(parse_err(""), ("expected `program`, found end of input".into(), 0..0));
    }

    #[test]
    #[should_panic(expected = "Eof")]
    fn token_stream_without_eof_panics() {
        parse(&[]).ok();
    }
}
