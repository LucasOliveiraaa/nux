//! Statements and expressions.

use std::sync::LazyLock;

use nux_executor::Executor;
use nux_lexer::Span;
use nux_parser::ast::{self, FunctionArg, Ident};
use regex::Regex;

use crate::context::EvalContext;
use crate::{EvalError, EvalResult, Host, Result, Value};

/// An evaluated call argument.
pub(crate) struct ParsedFuncArg {
    pub(crate) name: Option<Ident>,
    pub(crate) value: Value,
    pub(crate) span: Span,
}

pub(crate) static INTERPOLATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\$\{([a-zA-Z_][a-zA-Z0-9_]*)\}"#).unwrap());

/// Whether evaluation goes on after a statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    Continue,
    /// A `return;` ran: skip everything up to the end of the program.
    Return,
}

pub(crate) struct Evaluator<'e> {
    /// Runs the programs of `exec(...)`.
    pub(crate) executor: &'e mut Executor,
    pub(crate) host: &'e Host,
    pub(crate) context: EvalContext,
}

impl<'e> Evaluator<'e> {
    pub(crate) fn new(executor: &'e mut Executor, host: &'e Host) -> Self {
        Self {
            executor,
            host,
            context: EvalContext::new(),
        }
    }

    pub(crate) fn in_scope<Ret>(&mut self, f: impl FnOnce(&mut Self) -> Ret) -> Ret {
        self.context.enter_scope();
        let result = f(self);
        self.context.exit_scope();
        result
    }

    pub(crate) fn eval_program(&mut self, program: &ast::Program) -> Result<EvalResult> {
        // Whether it ran to the end or returned, the program is complete.
        self.eval_statements(&program.statements)?;
        self.context.finish_version();

        if self.context.versions.is_empty() {
            return Err(EvalError::new(
                format!(
                    "program `{}` does not specify a version; call `version(\"...\")`",
                    program.name.name
                ),
                program.name.span,
            ));
        }

        Ok(EvalResult {
            name: program.name.name.clone(),
            versions: std::mem::take(&mut self.context.versions),
        })
    }

    pub(crate) fn eval_statements(&mut self, statements: &[ast::Statement]) -> Result<Flow> {
        for statement in statements {
            if self.eval_statement(statement)? == Flow::Return {
                return Ok(Flow::Return);
            }
        }
        Ok(Flow::Continue)
    }

    pub(crate) fn eval_block(&mut self, block: &ast::Block) -> Result<Flow> {
        self.in_scope(|this| this.eval_statements(&block.statements))
    }

    pub(crate) fn eval_statement(&mut self, statement: &ast::Statement) -> Result<Flow> {
        match &statement.kind {
            ast::StatementKind::Let { name, value } => {
                let evaluated_value = self.eval_expr(value)?;
                self.context.set_binding(name.name.clone(), evaluated_value);
            }
            ast::StatementKind::Assign { name, value } => {
                let evaluated_value = self.eval_expr(value)?;
                if !self.context.assign(&name.name, evaluated_value) {
                    return Err(EvalError::new(
                        format!("cannot assign to undefined variable `{}`", name.name),
                        name.span,
                    ));
                }
            }
            ast::StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                if self.eval_expr(condition)?.is_truthy() {
                    return self.eval_block(then_branch);
                } else if let Some(else_branch) = else_branch {
                    return self.eval_block(else_branch);
                }
            }
            ast::StatementKind::While { condition, body } => {
                while self.eval_expr(condition)?.is_truthy() {
                    if self.eval_block(body)? == Flow::Return {
                        return Ok(Flow::Return);
                    }
                }
            }
            ast::StatementKind::Return => return Ok(Flow::Return),
            ast::StatementKind::Bash { commands } => {
                // Unknown names are left alone: `${HOME}` and friends belong to the shell.
                let interpolated = commands
                    .iter()
                    .map(|command| self.interpolate_string(command, statement.span, false))
                    .collect::<Result<Vec<_>>>()?;
                self.context
                    .current_version("`bash` block", statement.span)?
                    .commands
                    .extend(interpolated);
            }
            ast::StatementKind::Expr(expr) => {
                self.eval_expr(expr)?;
            }
        }
        Ok(Flow::Continue)
    }

    /// Substitutes `${name}` with bound values. With `strict`, an unbound name
    /// is an error reported at `span`; otherwise it is kept verbatim.
    pub(crate) fn interpolate_string(
        &self,
        input: &str,
        span: Span,
        strict: bool,
    ) -> Result<String> {
        let mut missing = None;

        let result = INTERPOLATION.replace_all(input, |caps: &regex::Captures| {
            let var_name = &caps[1];
            match self.context.get_binding(var_name) {
                Some(value) => value.to_string(),
                None => {
                    missing.get_or_insert_with(|| var_name.to_string());
                    caps[0].to_string()
                }
            }
        });

        match missing {
            Some(name) if strict => Err(EvalError::new(
                format!("undefined variable `{name}` in string interpolation"),
                span,
            )),
            _ => Ok(result.into_owned()),
        }
    }

    pub(crate) fn eval_expr(&mut self, expr: &ast::Expr) -> Result<Value> {
        let span = expr.span;
        match &expr.kind {
            ast::ExprKind::Literal(ast::Literal::String(string)) => {
                let interpolated = self.interpolate_string(string, span, true)?;
                Ok(Value::String(interpolated))
            }
            ast::ExprKind::Literal(ast::Literal::Number(num)) => Ok(Value::Number(*num)),
            ast::ExprKind::Literal(ast::Literal::Boolean(b)) => Ok(Value::Boolean(*b)),
            ast::ExprKind::Variable(name) => self
                .context
                .get_binding(name)
                .cloned()
                .ok_or_else(|| EvalError::new(format!("undefined variable `{name}`"), span)),
            ast::ExprKind::Group(inner) => self.eval_expr(inner),
            ast::ExprKind::List(items) => Ok(Value::List(
                items
                    .iter()
                    .map(|item| self.eval_expr(item))
                    .collect::<Result<_>>()?,
            )),
            ast::ExprKind::MemberAccess { object, member } => {
                let object_value = self.eval_expr(object)?;
                match object_value {
                    Value::Object(map) => map.get(&member.name).cloned().ok_or_else(|| {
                        EvalError::new(
                            format!("no member `{}` on this object", member.name),
                            member.span,
                        )
                    }),
                    other => Err(EvalError::new(
                        format!(
                            "cannot access member `{}` of a {}",
                            member.name,
                            other.get_type()
                        ),
                        span,
                    )),
                }
            }
            ast::ExprKind::UnaryOp { op, expr } => {
                let value = self.eval_expr(expr)?;

                match (op, value) {
                    (ast::UnaryOperator::Negate, Value::Number(num)) => Ok(Value::Number(-num)),
                    (ast::UnaryOperator::Not, Value::Boolean(b)) => Ok(Value::Boolean(!b)),
                    (ast::UnaryOperator::Negate, other) => Err(EvalError::new(
                        format!("unary `-` cannot be applied to a {}", other.get_type()),
                        span,
                    )),
                    (ast::UnaryOperator::Not, other) => Err(EvalError::new(
                        format!("unary `!` cannot be applied to a {}", other.get_type()),
                        span,
                    )),
                }
            }
            ast::ExprKind::BinaryOp { left, op, right } => self.eval_binary(left, op, right, span),
            ast::ExprKind::FunctionCall { callee, args } => {
                // `add_dependency(gcc)` names a program, not a variable.
                let bare_names = callee.name == "add_dependency";

                let evaluated_args = args
                    .iter()
                    .map(|arg| self.eval_function_arg(arg, bare_names))
                    .collect::<Result<Vec<_>>>()?;

                self.eval_function_call(callee, span, evaluated_args)
            }
        }
    }

    pub(crate) fn eval_binary(
        &mut self,
        left: &ast::Expr,
        op: &ast::BinaryOperator,
        right: &ast::Expr,
        span: Span,
    ) -> Result<Value> {
        use ast::BinaryOperator as Op;

        let left = self.eval_expr(left)?;

        // `&&` and `||` short-circuit: the right side may be invalid when the
        // left side already decides the result (e.g. `x != 0 && 1 / x > 0`).
        if let Op::AndAnd | Op::OrOr = op {
            let symbol = if *op == Op::AndAnd { "&&" } else { "||" };
            let Value::Boolean(l) = left else {
                return Err(EvalError::new(
                    format!(
                        "binary `{symbol}` expects booleans, found {}",
                        left.get_type()
                    ),
                    span,
                ));
            };
            if l == (*op == Op::OrOr) {
                return Ok(Value::Boolean(l));
            }
            return match self.eval_expr(right)? {
                Value::Boolean(r) => Ok(Value::Boolean(r)),
                other => Err(EvalError::new(
                    format!(
                        "binary `{symbol}` expects booleans, found {}",
                        other.get_type()
                    ),
                    span,
                )),
            };
        }

        let right = self.eval_expr(right)?;
        let mismatch = |symbol: &str| {
            EvalError::new(
                format!(
                    "binary `{symbol}` cannot be applied to {} and {}",
                    left.get_type(),
                    right.get_type()
                ),
                span,
            )
        };

        let numbers = match (&left, &right) {
            (Value::Number(l), Value::Number(r)) => Some((*l, *r)),
            _ => None,
        };

        match op {
            Op::Add => numbers
                .map(|(l, r)| Value::Number(l + r))
                .ok_or_else(|| mismatch("+")),
            Op::Subtract => numbers
                .map(|(l, r)| Value::Number(l - r))
                .ok_or_else(|| mismatch("-")),
            Op::Multiply => numbers
                .map(|(l, r)| Value::Number(l * r))
                .ok_or_else(|| mismatch("*")),
            Op::Divide | Op::Modulo => {
                let (symbol, what) = if *op == Op::Divide {
                    ("/", "division")
                } else {
                    ("%", "modulo")
                };
                let (l, r) = numbers.ok_or_else(|| mismatch(symbol))?;
                if r == 0.0 {
                    Err(EvalError::new(format!("{what} by zero"), span))
                } else if *op == Op::Divide {
                    Ok(Value::Number(l / r))
                } else {
                    Ok(Value::Number(l % r))
                }
            }
            Op::And => match left.clone() & right.clone() {
                Value::Null => Err(mismatch("&")),
                result => Ok(result),
            },
            Op::Or => match left.clone() | right.clone() {
                Value::Null => Err(mismatch("|")),
                result => Ok(result),
            },
            Op::Equal => Ok(Value::Boolean(left == right)),
            Op::NotEqual => Ok(Value::Boolean(left != right)),
            Op::GreaterThan => numbers
                .map(|(l, r)| Value::Boolean(l > r))
                .ok_or_else(|| mismatch(">")),
            Op::LessThan => numbers
                .map(|(l, r)| Value::Boolean(l < r))
                .ok_or_else(|| mismatch("<")),
            Op::GreaterThanOrEqual => numbers
                .map(|(l, r)| Value::Boolean(l >= r))
                .ok_or_else(|| mismatch(">=")),
            Op::LessThanOrEqual => numbers
                .map(|(l, r)| Value::Boolean(l <= r))
                .ok_or_else(|| mismatch("<=")),
            Op::AndAnd | Op::OrOr => unreachable!("handled above"),
        }
    }

    pub(crate) fn eval_function_arg(
        &mut self,
        arg: &FunctionArg,
        bare_names: bool,
    ) -> Result<ParsedFuncArg> {
        let (name, expr) = match &arg.kind {
            ast::FunctionArgKind::Value(expr) => (None, expr),
            ast::FunctionArgKind::Bind { name, value } => (Some(name.clone()), value),
        };

        let value = match &expr.kind {
            ast::ExprKind::Variable(var)
                if bare_names && self.context.get_binding(var).is_none() =>
            {
                Value::String(var.clone())
            }
            _ => self.eval_expr(expr)?,
        };

        Ok(ParsedFuncArg {
            name,
            value,
            span: arg.span,
        })
    }
}
