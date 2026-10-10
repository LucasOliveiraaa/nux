//! Runtime values.

use std::collections::HashMap;
use std::ops::{BitAnd, BitOr};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    String(String),
    Number(f64),
    Boolean(bool),
    Object(HashMap<String, Value>),
    List(Vec<Value>),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueType {
    String,
    Number,
    Boolean,
    Object,
    List,
    Null,
}

impl std::fmt::Display for ValueType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValueType::String => write!(f, "string"),
            ValueType::Number => write!(f, "number"),
            ValueType::Boolean => write!(f, "boolean"),
            ValueType::Object => write!(f, "object"),
            ValueType::List => write!(f, "list"),
            ValueType::Null => write!(f, "null"),
        }
    }
}

impl Value {
    pub fn get_type(&self) -> &'static str {
        match self {
            Value::String(_) => "string",
            Value::Number(_) => "number",
            Value::Boolean(_) => "boolean",
            Value::Object(_) => "object",
            Value::List(_) => "list",
            Value::Null => "null",
        }
    }

    pub fn value_type(&self) -> ValueType {
        match self {
            Value::String(_) => ValueType::String,
            Value::Number(_) => ValueType::Number,
            Value::Boolean(_) => ValueType::Boolean,
            Value::Object(_) => ValueType::Object,
            Value::List(_) => ValueType::List,
            Value::Null => ValueType::Null,
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::String(s) => !s.is_empty(),
            Value::Number(n) => *n != 0.0,
            Value::Boolean(b) => *b,
            Value::Object(_) => true,
            Value::List(items) => !items.is_empty(),
            Value::Null => false,
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::String(s) => write!(f, "{}", s),
            Value::Number(n) => write!(f, "{}", n),
            Value::Boolean(b) => write!(f, "{}", b),
            Value::Object(_) => write!(f, "[object]"),
            // As shell words: `["a", "b"]` interpolates as `a b`.
            Value::List(items) => {
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    write!(f, "{item}")?;
                }
                Ok(())
            }
            Value::Null => write!(f, "null"),
        }
    }
}

impl BitAnd for Value {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::Number(lhs), Value::Number(rhs)) => {
                let result = (lhs as i64) & (rhs as i64);
                Value::Number(result as f64)
            }
            (Value::Boolean(lhs), Value::Boolean(rhs)) => Value::Boolean(lhs & rhs),
            _ => Value::Null,
        }
    }
}

impl BitOr for Value {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Value::Number(lhs), Value::Number(rhs)) => {
                let result = (lhs as i64) | (rhs as i64);
                Value::Number(result as f64)
            }
            (Value::Boolean(lhs), Value::Boolean(rhs)) => Value::Boolean(lhs | rhs),
            _ => Value::Null,
        }
    }
}
