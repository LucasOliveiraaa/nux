//! Version numbers and the requirement strings used to select them.
//!
//! Versions are split on `.`, `-` and `+` into numeric or textual segments
//! and compared segment by segment. Missing trailing segments count as `0`,
//! so `1.2` and `1.2.0` compare equal; a textual segment sorts before a
//! numeric one, so `1.0.rc1` comes before `1.0.0`.
//!
//! A requirement is `latest` (or `*`), or a comma-separated list of
//! comparators that must all hold: `1.2.3` / `=1.2.3` (exact), `>`, `>=`,
//! `<`, `<=`, `^1.2` (same leftmost non-zero component) and `~1.2` (same
//! minor version).

use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Num(u64),
    Text(String),
}

impl Segment {
    const ZERO: Segment = Segment::Num(0);

    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Segment::Num(a), Segment::Num(b)) => a.cmp(b),
            (Segment::Text(a), Segment::Text(b)) => a.cmp(b),
            (Segment::Text(_), Segment::Num(_)) => Ordering::Less,
            (Segment::Num(_), Segment::Text(_)) => Ordering::Greater,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Version {
    raw: String,
    segments: Vec<Segment>,
}

impl Version {
    /// Parses `raw`; fails on an empty string, whitespace or empty segments.
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.is_empty() {
            return Err("version is empty".into());
        }
        if raw.chars().any(char::is_whitespace) {
            return Err(format!("version `{raw}` contains whitespace"));
        }
        let segments = raw
            .split(['.', '-', '+'])
            .map(|part| match part {
                "" => Err(format!("version `{raw}` has an empty component")),
                _ => Ok(part
                    .parse()
                    .map(Segment::Num)
                    .unwrap_or_else(|_| Segment::Text(part.to_string()))),
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            raw: raw.to_string(),
            segments,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Compares by value only, ignoring how the version was spelled.
    fn cmp_value(&self, other: &Self) -> Ordering {
        let len = self.segments.len().max(other.segments.len());
        (0..len)
            .map(|i| {
                let a = self.segments.get(i).unwrap_or(&Segment::ZERO);
                let b = other.segments.get(i).unwrap_or(&Segment::ZERO);
                a.cmp(b)
            })
            .find(|o| o.is_ne())
            .unwrap_or(Ordering::Equal)
    }

    /// The smallest version above every version that keeps the first
    /// `index + 1` segments, e.g. bumping `1.2.3` at 1 gives `1.3`.
    fn bump(&self, index: usize) -> Option<Version> {
        let mut segments = self.segments[..=index].to_vec();
        let Segment::Num(n) = segments[index] else {
            return None;
        };
        segments[index] = Segment::Num(n + 1);
        Some(Version {
            raw: String::new(),
            segments,
        })
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Version {}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A total order: by value, then by spelling so that `1.2` and `1.2.0` are
/// still distinct and consistently ordered.
impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.cmp_value(other).then_with(|| self.raw.cmp(&other.raw))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.raw)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Exact,
    Greater,
    GreaterEq,
    Less,
    LessEq,
    Caret,
    Tilde,
}

#[derive(Debug, Clone)]
struct Comparator {
    op: Op,
    version: Version,
    /// Exclusive upper bound for `^` and `~`.
    upper: Option<Version>,
}

impl Comparator {
    fn parse(text: &str) -> Result<Self, String> {
        let (op, rest) = [
            (">=", Op::GreaterEq),
            ("<=", Op::LessEq),
            (">", Op::Greater),
            ("<", Op::Less),
            ("=", Op::Exact),
            ("^", Op::Caret),
            ("~", Op::Tilde),
        ]
        .into_iter()
        .find_map(|(prefix, op)| text.strip_prefix(prefix).map(|rest| (op, rest)))
        .unwrap_or((Op::Exact, text));

        let version = Version::parse(rest.trim())?;
        let upper = match op {
            Op::Caret => {
                // The leftmost non-zero component must stay the same.
                let index = version
                    .segments
                    .iter()
                    .position(|s| *s != Segment::ZERO)
                    .unwrap_or(version.segments.len() - 1);
                Some(version.bump(index))
            }
            Op::Tilde => Some(version.bump(1.min(version.segments.len() - 1))),
            _ => None,
        };
        let upper = match upper {
            Some(None) => {
                return Err(format!("`{text}` needs a numeric component to bound the range"));
            }
            Some(bound) => bound,
            None => None,
        };
        Ok(Self { op, version, upper })
    }

    fn matches(&self, v: &Version) -> bool {
        let ord = v.cmp_value(&self.version);
        match self.op {
            Op::Exact => ord.is_eq(),
            Op::Greater => ord.is_gt(),
            Op::GreaterEq => ord.is_ge(),
            Op::Less => ord.is_lt(),
            Op::LessEq => ord.is_le(),
            Op::Caret | Op::Tilde => {
                ord.is_ge() && v.cmp_value(self.upper.as_ref().unwrap()).is_lt()
            }
        }
    }
}

/// A parsed requirement such as `latest`, `4.2.0` or `>=1.2, <2`.
#[derive(Debug, Clone)]
pub struct VersionReq {
    comparators: Vec<Comparator>,
}

impl VersionReq {
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if matches!(text, "latest" | "*") {
            return Ok(Self {
                comparators: Vec::new(),
            });
        }
        let comparators = text
            .split(',')
            .map(|part| Comparator::parse(part.trim()))
            .collect::<Result<_, _>>()?;
        Ok(Self { comparators })
    }

    pub fn matches(&self, version: &Version) -> bool {
        self.comparators.iter().all(|c| c.matches(version))
    }
}
