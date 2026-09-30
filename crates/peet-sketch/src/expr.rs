//! Expressions and named parameters for dimension values (`width = 2 * height + 5`).
//!
//! # Grammar
//!
//! ```text
//! sum      = product { ("+" | "-") product }
//! product  = unary { ("*" | "/") unary }
//! unary    = ("-" | "+") unary | power
//! power    = postfix [ "^" unary ]          (right associative: 2^3^2 = 2^9)
//! postfix  = number [unit] | "(" sum ")" [unit] | name "(" [sum {"," sum}] ")" | name
//! number   = 12 | 12.5 | .5 | 1e3 | 2.5E-2
//! name     = [A-Za-z_][A-Za-z0-9_]*
//! unit     = mm | cm | m | in | ft | deg | rad | °
//! ```
//!
//! `^` binds tighter than unary minus, so `-2^2 = -4` (as in mathematics).
//!
//! **Units** are plain scale factors to the display units: lengths become millimetres
//! (`1in = 25.4`), angles become degrees (`1rad = 57.29…`). Nothing checks that a length
//! is used where a length is expected; unit-aware expressions arrive in Phase 3.
//!
//! **Functions:** `sin cos tan asin acos atan atan2 sqrt abs min max round floor ceil`.
//! Trigonometry works in **degrees**, like the rest of the sketcher: `sin(30) = 0.5`,
//! `atan2(1, 1) = 45`. `min`/`max` take one or more arguments, `atan2(y, x)` two, the
//! others one. The constant `pi` is π.
//!
//! **Names** refer to dimensions of the sketch (`d1`, or a name the user gave it) and to
//! [`Parameters`]; dimension names win when both exist.

use std::f64::consts::PI;

use crate::sketch::{ConstraintId, Sketch};

/// A parse or evaluation error, with a message meant for the user.
#[derive(Clone, Debug, PartialEq)]
pub struct ExprError {
    pub message: String,
}

impl ExprError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ExprError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ExprError {}

// ---- Built-in names ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Func {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Atan2,
    Sqrt,
    Abs,
    Min,
    Max,
    Round,
    Floor,
    Ceil,
}

const FUNCTIONS: &[(&str, Func)] = &[
    ("sin", Func::Sin),
    ("cos", Func::Cos),
    ("tan", Func::Tan),
    ("asin", Func::Asin),
    ("acos", Func::Acos),
    ("atan", Func::Atan),
    ("atan2", Func::Atan2),
    ("sqrt", Func::Sqrt),
    ("abs", Func::Abs),
    ("min", Func::Min),
    ("max", Func::Max),
    ("round", Func::Round),
    ("floor", Func::Floor),
    ("ceil", Func::Ceil),
];

/// Unit suffixes and their factor to display units (mm, degrees).
const UNITS: &[(&str, f64)] = &[
    ("mm", 1.0),
    ("cm", 10.0),
    ("m", 1000.0),
    ("in", 25.4),
    ("ft", 304.8),
    ("deg", 1.0),
    ("rad", 180.0 / PI),
];

const CONSTANTS: &[(&str, f64)] = &[("pi", PI)];

fn function(name: &str) -> Option<Func> {
    FUNCTIONS.iter().find(|(n, _)| *n == name).map(|(_, f)| *f)
}

fn unit(name: &str) -> Option<f64> {
    UNITS.iter().find(|(n, _)| *n == name).map(|(_, f)| *f)
}

fn constant(name: &str) -> Option<f64> {
    CONSTANTS.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

impl Func {
    fn name(self) -> &'static str {
        FUNCTIONS
            .iter()
            .find(|(_, f)| *f == self)
            .map(|(n, _)| *n)
            .unwrap_or("?")
    }

    /// (min, max) argument count; `None` max means variadic.
    fn arity(self) -> (usize, Option<usize>) {
        match self {
            Func::Atan2 => (2, Some(2)),
            Func::Min | Func::Max => (1, None),
            _ => (1, Some(1)),
        }
    }
}

/// Whether `name` is a valid identifier for a parameter or dimension: letters, digits and
/// `_`, not starting with a digit, and not a function, constant or unit name.
pub fn check_name(name: &str) -> Result<(), ExprError> {
    let mut chars = name.chars();
    let valid = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        return Err(ExprError::new(format!(
            "'{name}' is not a valid name: use letters, digits and '_', starting with a letter or '_'"
        )));
    }
    let reserved = if function(name).is_some() {
        Some("a function")
    } else if constant(name).is_some() {
        Some("a constant")
    } else if unit(name).is_some() {
        Some("a unit")
    } else {
        None
    };
    match reserved {
        Some(what) => Err(ExprError::new(format!(
            "'{name}' is {what} and can't be used as a name"
        ))),
        None => Ok(()),
    }
}

// ---- Lexer ----

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    /// `+ - * / ^`
    Op(char),
    LParen,
    RParen,
    Comma,
    Degree,
    End,
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    /// 1-based character column.
    col: usize,
    /// The source text of the token (for messages).
    text: String,
}

fn lex(src: &str) -> Result<Vec<Token>, ExprError> {
    let chars: Vec<char> = src.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let col = i + 1;
        let start = i;
        let digit_at = |j: usize| chars.get(j).is_some_and(|c| c.is_ascii_digit());
        let tok = if c.is_whitespace() {
            i += 1;
            continue;
        } else if c.is_ascii_digit() || (c == '.' && digit_at(i + 1)) {
            while digit_at(i) {
                i += 1;
            }
            if chars.get(i) == Some(&'.') {
                i += 1;
                while digit_at(i) {
                    i += 1;
                }
            }
            if matches!(chars.get(i), Some('e' | 'E'))
                && (digit_at(i + 1)
                    || (matches!(chars.get(i + 1), Some('+' | '-')) && digit_at(i + 2)))
            {
                i += 2;
                while digit_at(i) {
                    i += 1;
                }
            }
            let text: String = chars[start..i].iter().collect();
            let value = text
                .parse::<f64>()
                .map_err(|_| ExprError::new(format!("invalid number '{text}' at column {col}")))?;
            Tok::Num(value)
        } else if c.is_ascii_alphabetic() || c == '_' {
            while chars
                .get(i)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
            {
                i += 1;
            }
            Tok::Ident(chars[start..i].iter().collect())
        } else {
            i += 1;
            match c {
                '+' | '-' | '*' | '/' | '^' => Tok::Op(c),
                '(' => Tok::LParen,
                ')' => Tok::RParen,
                ',' => Tok::Comma,
                '°' => Tok::Degree,
                _ => {
                    return Err(ExprError::new(format!(
                        "unexpected character '{c}' at column {col}"
                    )));
                }
            }
        };
        tokens.push(Token {
            tok,
            col,
            text: chars[start..i].iter().collect(),
        });
    }
    tokens.push(Token {
        tok: Tok::End,
        col: chars.len() + 1,
        text: String::new(),
    });
    Ok(tokens)
}

/// If `src` is just a number (optionally signed, no unit), returns it.
fn plain_number(src: &str) -> Option<f64> {
    let tokens = lex(src).ok()?;
    match tokens.as_slice() {
        [
            Token {
                tok: Tok::Num(v), ..
            },
            _end,
        ] => Some(*v),
        [
            Token {
                tok: Tok::Op(sign @ ('+' | '-')),
                ..
            },
            Token {
                tok: Tok::Num(v), ..
            },
            _end,
        ] => Some(if *sign == '-' { -*v } else { *v }),
        _ => None,
    }
}

// ---- Parser ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Num(f64),
    Name {
        name: String,
        col: usize,
    },
    Neg(Box<Node>),
    Bin {
        op: BinOp,
        lhs: Box<Node>,
        rhs: Box<Node>,
        col: usize,
    },
    Call {
        func: Func,
        args: Vec<Node>,
        col: usize,
    },
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn next(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn unexpected(&self, t: &Token, expected: &str) -> ExprError {
        match &t.tok {
            Tok::End => {
                ExprError::new(format!("unexpected end of expression, expected {expected}"))
            }
            Tok::Ident(name) => ExprError::new(format!(
                "unexpected name '{name}' at column {} — missing an operator? (units are mm, cm, m, in, ft, deg, rad)",
                t.col
            )),
            _ => ExprError::new(format!("unexpected '{}' at column {}", t.text, t.col)),
        }
    }

    fn sum(&mut self) -> Result<Node, ExprError> {
        let mut lhs = self.product()?;
        while let Tok::Op(c @ ('+' | '-')) = self.peek().tok {
            let col = self.next().col;
            let rhs = self.product()?;
            let op = if c == '+' { BinOp::Add } else { BinOp::Sub };
            lhs = Node::Bin {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                col,
            };
        }
        Ok(lhs)
    }

    fn product(&mut self) -> Result<Node, ExprError> {
        let mut lhs = self.unary()?;
        while let Tok::Op(c @ ('*' | '/')) = self.peek().tok {
            let col = self.next().col;
            let rhs = self.unary()?;
            let op = if c == '*' { BinOp::Mul } else { BinOp::Div };
            lhs = Node::Bin {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                col,
            };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Node, ExprError> {
        match self.peek().tok {
            Tok::Op('-') => {
                self.next();
                Ok(Node::Neg(Box::new(self.unary()?)))
            }
            Tok::Op('+') => {
                self.next();
                self.unary()
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<Node, ExprError> {
        let base = self.postfix()?;
        if self.peek().tok == Tok::Op('^') {
            let col = self.next().col;
            let exponent = self.unary()?;
            return Ok(Node::Bin {
                op: BinOp::Pow,
                lhs: Box::new(base),
                rhs: Box::new(exponent),
                col,
            });
        }
        Ok(base)
    }

    fn postfix(&mut self) -> Result<Node, ExprError> {
        let t = self.next();
        let value = match t.tok {
            Tok::Num(v) => Node::Num(v),
            Tok::LParen => {
                let inner = self.sum()?;
                let close = self.next();
                if close.tok != Tok::RParen {
                    return Err(match close.tok {
                        Tok::End => {
                            ExprError::new(format!("missing ')' to close '(' at column {}", t.col))
                        }
                        _ => self.unexpected(&close, "')'"),
                    });
                }
                inner
            }
            Tok::Ident(name) => match self.name_or_call(name, t.col)? {
                // Constants (`pi`) take units like numbers do.
                n @ Node::Num(_) => n,
                n => return Ok(n),
            },
            _ => return Err(self.unexpected(&t, "a number, a name or '('")),
        };
        // Optional unit suffix after a number or a parenthesised expression.
        let factor = match &self.peek().tok {
            Tok::Degree => Some(1.0),
            Tok::Ident(name) => unit(name),
            _ => None,
        };
        match factor {
            Some(f) => {
                let col = self.next().col;
                Ok(scale(value, f, col))
            }
            None => Ok(value),
        }
    }

    fn name_or_call(&mut self, name: String, col: usize) -> Result<Node, ExprError> {
        if self.peek().tok != Tok::LParen {
            if function(&name).is_some() {
                return Err(ExprError::new(format!(
                    "'{name}' at column {col} is a function — write {name}(...)"
                )));
            }
            if let Some(v) = constant(&name) {
                return Ok(Node::Num(v));
            }
            return Ok(Node::Name { name, col });
        }
        let Some(func) = function(&name) else {
            let hint = suggest(&name, FUNCTIONS.iter().map(|(n, _)| *n))
                .map(|s| format!(" — did you mean '{s}'?"))
                .unwrap_or_default();
            return Err(ExprError::new(format!(
                "unknown function '{name}' at column {col}{hint}"
            )));
        };
        let open = self.next();
        let mut args = Vec::new();
        if self.peek().tok == Tok::RParen {
            self.next();
        } else {
            loop {
                args.push(self.sum()?);
                let t = self.next();
                match t.tok {
                    Tok::Comma => continue,
                    Tok::RParen => break,
                    Tok::End => {
                        return Err(ExprError::new(format!(
                            "missing ')' to close '(' at column {}",
                            open.col
                        )));
                    }
                    _ => return Err(self.unexpected(&t, "',' or ')'")),
                }
            }
        }
        let (min, max) = func.arity();
        if args.len() < min || max.is_some_and(|m| args.len() > m) {
            let expected = match (min, max) {
                (1, Some(1)) => "1 argument".to_owned(),
                (a, Some(b)) if a == b => format!("{a} arguments"),
                (a, _) => format!("at least {a} argument{}", if a == 1 { "" } else { "s" }),
            };
            return Err(ExprError::new(format!(
                "{name} at column {col} takes {expected}, got {}",
                args.len()
            )));
        }
        Ok(Node::Call { func, args, col })
    }
}

fn scale(node: Node, factor: f64, col: usize) -> Node {
    if factor == 1.0 {
        return node;
    }
    match node {
        Node::Num(v) => Node::Num(v * factor),
        n => Node::Bin {
            op: BinOp::Mul,
            lhs: Box::new(n),
            rhs: Box::new(Node::Num(factor)),
            col,
        },
    }
}

// ---- Evaluation helpers ----

/// `x mod 360` in `[0, 360)`, exact for exact inputs.
fn reduce_degrees(x: f64) -> f64 {
    x.rem_euclid(360.0)
}

/// Sine of an angle in degrees, exact at multiples of 90°.
fn sin_deg(x: f64) -> f64 {
    let r = reduce_degrees(x);
    match r {
        0.0 | 180.0 => 0.0,
        90.0 => 1.0,
        270.0 => -1.0,
        _ => r.to_radians().sin(),
    }
}

fn cos_deg(x: f64) -> f64 {
    sin_deg(x + 90.0)
}

fn fmt_num(v: f64) -> String {
    format!("{v}")
}

/// A parsed expression.
#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    root: Node,
}

impl Expr {
    pub fn parse(source: &str) -> Result<Self, ExprError> {
        let tokens = lex(source)?;
        if tokens.len() == 1 {
            return Err(ExprError::new("enter a value or an expression"));
        }
        let mut p = Parser { tokens, pos: 0 };
        let root = p.sum()?;
        let t = p.next();
        if t.tok != Tok::End {
            return Err(p.unexpected(&t, "the end of the expression"));
        }
        Ok(Self { root })
    }

    /// Evaluates with `lookup` resolving names to values.
    pub fn eval(&self, lookup: &dyn Fn(&str) -> Option<f64>) -> Result<f64, ExprError> {
        eval_node(&self.root, lookup)
    }

    /// Names the expression refers to (parameters or dimensions), without duplicates.
    pub fn names(&self) -> Vec<String> {
        self.refs().into_iter().map(|(n, _)| n.to_owned()).collect()
    }

    /// Names with the column of their first use, in order of first use.
    fn refs(&self) -> Vec<(&str, usize)> {
        fn walk<'a>(n: &'a Node, out: &mut Vec<(&'a str, usize)>) {
            match n {
                Node::Num(_) => {}
                Node::Name { name, col } => {
                    if !out.iter().any(|(m, _)| m == name) {
                        out.push((name, *col));
                    }
                }
                Node::Neg(a) => walk(a, out),
                Node::Bin { lhs, rhs, .. } => {
                    walk(lhs, out);
                    walk(rhs, out);
                }
                Node::Call { args, .. } => args.iter().for_each(|a| walk(a, out)),
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &mut out);
        out
    }
}

fn eval_node(n: &Node, lookup: &dyn Fn(&str) -> Option<f64>) -> Result<f64, ExprError> {
    match n {
        Node::Num(v) => Ok(*v),
        Node::Name { name, col } => lookup(name)
            .ok_or_else(|| ExprError::new(format!("unknown name '{name}' at column {col}"))),
        Node::Neg(a) => Ok(-eval_node(a, lookup)?),
        Node::Bin { op, lhs, rhs, col } => {
            let a = eval_node(lhs, lookup)?;
            let b = eval_node(rhs, lookup)?;
            let v = match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => {
                    if b == 0.0 {
                        return Err(ExprError::new(format!("division by zero at column {col}")));
                    }
                    a / b
                }
                BinOp::Pow => {
                    if a < 0.0 && b.fract() != 0.0 {
                        return Err(ExprError::new(format!(
                            "can't raise a negative number ({}) to a fractional power ({}) at column {col}",
                            fmt_num(a),
                            fmt_num(b)
                        )));
                    }
                    if a == 0.0 && b < 0.0 {
                        return Err(ExprError::new(format!(
                            "division by zero (0 to a negative power) at column {col}"
                        )));
                    }
                    a.powf(b)
                }
            };
            if !v.is_finite() {
                return Err(ExprError::new(format!(
                    "the result at column {col} is too large"
                )));
            }
            Ok(v)
        }
        Node::Call { func, args, col } => {
            let mut values = [0.0; 2];
            let mut rest = Vec::new();
            for (i, a) in args.iter().enumerate() {
                let v = eval_node(a, lookup)?;
                if i < 2 {
                    values[i] = v;
                } else {
                    rest.push(v);
                }
            }
            let x = values[0];
            let name = func.name();
            let domain = |what: &str| {
                Err(ExprError::new(format!(
                    "{name}({}) at column {col}: {what}",
                    fmt_num(x)
                )))
            };
            let v = match func {
                Func::Sin => sin_deg(x),
                Func::Cos => cos_deg(x),
                Func::Tan => {
                    let r = x.rem_euclid(180.0);
                    if r == 90.0 {
                        return domain("tan is undefined at 90°");
                    }
                    match r {
                        0.0 => 0.0,
                        45.0 => 1.0,
                        135.0 => -1.0,
                        _ => r.to_radians().tan(),
                    }
                }
                Func::Asin | Func::Acos => {
                    if !(-1.0..=1.0).contains(&x) {
                        return domain("needs a value between -1 and 1");
                    }
                    if *func == Func::Asin {
                        x.asin().to_degrees()
                    } else {
                        x.acos().to_degrees()
                    }
                }
                Func::Atan => x.atan().to_degrees(),
                Func::Atan2 => {
                    let (y, xx) = (values[0], values[1]);
                    if y == 0.0 && xx == 0.0 {
                        return Err(ExprError::new(format!(
                            "atan2(0, 0) at column {col} is undefined"
                        )));
                    }
                    y.atan2(xx).to_degrees()
                }
                Func::Sqrt => {
                    if x < 0.0 {
                        return domain("square root of a negative number");
                    }
                    x.sqrt()
                }
                Func::Abs => x.abs(),
                Func::Round => x.round(),
                Func::Floor => x.floor(),
                Func::Ceil => x.ceil(),
                Func::Min | Func::Max => {
                    let all = values[..args.len().min(2)].iter().chain(rest.iter());
                    if *func == Func::Min {
                        all.copied().fold(f64::INFINITY, f64::min)
                    } else {
                        all.copied().fold(f64::NEG_INFINITY, f64::max)
                    }
                }
            };
            if !v.is_finite() {
                return Err(ExprError::new(format!(
                    "the result of {name} at column {col} is not a finite number"
                )));
            }
            Ok(v)
        }
    }
}

// ---- Suggestions ----

/// Optimal string alignment distance (Levenshtein plus adjacent transpositions),
/// ignoring ASCII case.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().map(|c| c.to_ascii_lowercase()).collect();
    let b: Vec<char> = b.chars().map(|c| c.to_ascii_lowercase()).collect();
    let w = b.len() + 1;
    let mut d = vec![0usize; (a.len() + 1) * w];
    for i in 0..=a.len() {
        d[i * w] = i;
    }
    for (j, cell) in d.iter_mut().enumerate().take(w) {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (d[(i - 1) * w + j] + 1)
                .min(d[i * w + j - 1] + 1)
                .min(d[(i - 1) * w + j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[(i - 2) * w + j - 2] + 1);
            }
            d[i * w + j] = v;
        }
    }
    d[a.len() * w + b.len()]
}

/// The closest candidate to `name`, if it's close enough to be a plausible typo.
/// Ties go to the alphabetically first candidate.
fn suggest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let limit = if name.chars().count() <= 3 { 1 } else { 2 };
    candidates
        .into_iter()
        .filter(|c| *c != name)
        .map(|c| (edit_distance(name, c), c))
        .filter(|(d, _)| *d <= limit)
        .min()
        .map(|(_, c)| c)
}

fn unknown_name(name: &str, col: usize, candidates: &[&str]) -> ExprError {
    let hint = suggest(name, candidates.iter().copied())
        .map(|s| format!(" — did you mean '{s}'?"))
        .unwrap_or_default();
    ExprError::new(format!("unknown name '{name}' at column {col}{hint}"))
}

// ---- Dependency-ordered evaluation ----

/// How a name that isn't one of the graph's own nodes resolves.
enum External {
    Value(f64),
    /// Known but unusable; the message says why.
    Failed(String),
    Unknown,
}

/// Evaluates a set of named expressions that may refer to each other, in dependency order.
/// Names that aren't nodes go to `external`. `check` validates each node's value (so that
/// dependents of an invalid value fail too).
struct Graph<'a> {
    names: &'a [&'a str],
    exprs: &'a [Result<Expr, ExprError>],
    external: &'a dyn Fn(&str) -> External,
    candidates: &'a dyn Fn() -> Vec<String>,
    check: &'a dyn Fn(usize, f64) -> Result<(), ExprError>,
    state: Vec<u8>, // 0 = unvisited, 1 = on the stack, 2 = done
    stack: Vec<usize>,
    cycle: Vec<Option<ExprError>>,
    results: Vec<Option<Result<f64, ExprError>>>,
}

impl<'a> Graph<'a> {
    fn run(
        names: &'a [&'a str],
        exprs: &'a [Result<Expr, ExprError>],
        external: &'a dyn Fn(&str) -> External,
        candidates: &'a dyn Fn() -> Vec<String>,
        check: &'a dyn Fn(usize, f64) -> Result<(), ExprError>,
    ) -> Vec<Result<f64, ExprError>> {
        let n = names.len();
        let mut g = Graph {
            names,
            exprs,
            external,
            candidates,
            check,
            state: vec![0; n],
            stack: Vec::new(),
            cycle: vec![None; n],
            results: vec![None; n],
        };
        for i in 0..n {
            g.visit(i);
        }
        g.results
            .into_iter()
            .map(|r| r.expect("every node visited"))
            .collect()
    }

    fn node(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| *n == name)
    }

    fn visit(&mut self, i: usize) {
        if self.state[i] != 0 {
            return;
        }
        self.state[i] = 1;
        self.stack.push(i);
        let exprs = self.exprs;
        if let Ok(expr) = &exprs[i] {
            for (name, _) in expr.refs() {
                let Some(j) = self.node(name) else { continue };
                match self.state[j] {
                    0 => self.visit(j),
                    1 => {
                        let pos = self.stack.iter().position(|k| *k == j).expect("on stack");
                        let members: Vec<usize> = self.stack[pos..].to_vec();
                        for (s, &k) in members.iter().enumerate() {
                            if self.cycle[k].is_some() {
                                continue;
                            }
                            let path: Vec<&str> = (0..=members.len())
                                .map(|o| self.names[members[(s + o) % members.len()]])
                                .collect();
                            self.cycle[k] = Some(ExprError::new(format!(
                                "circular reference: {}",
                                path.join(" → ")
                            )));
                        }
                    }
                    _ => {}
                }
            }
        }
        self.stack.pop();
        self.state[i] = 2;
        let result = self.evaluate(i);
        self.results[i] = Some(result);
    }

    fn evaluate(&self, i: usize) -> Result<f64, ExprError> {
        if let Some(e) = &self.cycle[i] {
            return Err(e.clone());
        }
        let expr = self.exprs[i].as_ref().map_err(Clone::clone)?;
        for (name, col) in expr.refs() {
            match self.node(name) {
                Some(j) => {
                    if let Some(Err(_)) = &self.results[j] {
                        return Err(ExprError::new(format!(
                            "'{name}' at column {col} has an error"
                        )));
                    }
                }
                None => match (self.external)(name) {
                    External::Value(_) => {}
                    External::Failed(why) => {
                        return Err(ExprError::new(format!("'{name}' at column {col} {why}")));
                    }
                    External::Unknown => {
                        let mut all = (self.candidates)();
                        all.extend(self.names.iter().map(|s| (*s).to_owned()));
                        let refs: Vec<&str> = all.iter().map(String::as_str).collect();
                        return Err(unknown_name(name, col, &refs));
                    }
                },
            }
        }
        let value = expr.eval(&|name| match self.node(name) {
            Some(j) => self.results[j]
                .as_ref()
                .and_then(|r| r.as_ref().ok())
                .copied(),
            None => match (self.external)(name) {
                External::Value(v) => Some(v),
                _ => None,
            },
        })?;
        (self.check)(i, value)?;
        Ok(value)
    }
}

// ---- Parameters ----

/// A named, user-defined parameter.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Parameter {
    pub name: String,
    pub expression: String,
    /// Last evaluated value (NaN if evaluation failed).
    pub value: f64,
}

/// A table of named parameters. Parameters may refer to each other; cycles are errors.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Parameters {
    pub entries: Vec<Parameter>,
}

impl Parameters {
    /// The parameter's value, or `None` if it doesn't exist or failed to evaluate.
    pub fn get(&self, name: &str) -> Option<f64> {
        self.entries
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.value)
            .filter(|v| v.is_finite())
    }

    /// Adds or replaces a parameter and re-evaluates the table. The table is left
    /// unchanged if the new expression fails, or if it would break another parameter that
    /// evaluated fine before.
    pub fn set(&mut self, name: &str, expression: &str) -> Result<f64, ExprError> {
        check_name(name)?;
        Expr::parse(expression)?;
        let before = self.clone();
        let expression = expression.trim().to_owned();
        match self.entries.iter_mut().find(|p| p.name == name) {
            Some(p) => p.expression = expression,
            None => self.entries.push(Parameter {
                name: name.to_owned(),
                expression,
                value: f64::NAN,
            }),
        }
        let errors = self.evaluate();
        if let Some((_, e)) = errors.iter().find(|(n, _)| n == name) {
            let e = e.clone();
            *self = before;
            return Err(e);
        }
        if let Some((other, e)) = errors.iter().find(|(n, _)| before.get(n).is_some()) {
            let e = ExprError::new(format!("this would break '{other}': {e}"));
            *self = before;
            return Err(e);
        }
        Ok(self.get(name).expect("evaluated without error"))
    }

    /// Removes a parameter (parameters referring to it then fail to evaluate).
    pub fn remove(&mut self, name: &str) {
        self.entries.retain(|p| p.name != name);
        self.evaluate();
    }

    /// Re-evaluates all parameters in dependency order. Returns the errors, per name.
    pub fn evaluate(&mut self) -> Vec<(String, ExprError)> {
        let names: Vec<&str> = self.entries.iter().map(|p| p.name.as_str()).collect();
        let exprs: Vec<Result<Expr, ExprError>> = self
            .entries
            .iter()
            .map(|p| Expr::parse(&p.expression))
            .collect();
        let results = Graph::run(
            &names,
            &exprs,
            &|_| External::Unknown,
            &|| vec!["pi".to_owned()],
            &|_, _| Ok(()),
        );
        let mut errors = Vec::new();
        for (p, r) in self.entries.iter_mut().zip(results) {
            match r {
                Ok(v) => p.value = v,
                Err(e) => {
                    p.value = f64::NAN;
                    errors.push((p.name.clone(), e));
                }
            }
        }
        errors
    }
}

// ---- Dimensions ----

/// Checks that a value suits the dimension: lengths must be positive, angles in (0, 180].
fn check_dimension_value(sketch: &Sketch, id: ConstraintId, value: f64) -> Result<(), ExprError> {
    let Some(c) = sketch.constraint(id) else {
        return Ok(());
    };
    if !value.is_finite() {
        return Err(ExprError::new("the value is not a finite number"));
    }
    let label = c.kind.label();
    if c.kind.is_angular() {
        // A little slack at 180° so that `pi rad` is accepted.
        if value <= 0.0 || value > 180.0 + 1e-9 {
            return Err(ExprError::new(format!(
                "{label} must be more than 0° and at most 180°, got {}°",
                fmt_num(value)
            )));
        }
    } else if value <= 0.0 {
        return Err(ExprError::new(format!(
            "{label} must be greater than zero, got {} mm",
            fmt_num(value)
        )));
    }
    Ok(())
}

/// Every dimension: (id, name, expression).
fn dimension_table(sketch: &Sketch) -> Vec<(ConstraintId, &str, Option<&str>)> {
    sketch
        .constraints()
        .filter_map(|(id, c)| {
            let d = c.dimension.as_ref()?;
            Some((id, d.name.as_str(), d.expression.as_deref()))
        })
        .collect()
}

/// Evaluates the dimension expressions (`overrides` replaces or adds one), returning
/// the node ids and their results.
fn evaluate_dimensions(
    sketch: &Sketch,
    params: &Parameters,
    overrides: Option<(ConstraintId, &str)>,
) -> Vec<(ConstraintId, Result<f64, ExprError>)> {
    let table = dimension_table(sketch);
    let mut ids = Vec::new();
    let mut names: Vec<&str> = Vec::new();
    let mut exprs = Vec::new();
    for &(id, name, expression) in &table {
        let expression = match overrides {
            Some((o, e)) if o == id => Some(e),
            _ => expression,
        };
        if let Some(e) = expression {
            ids.push(id);
            names.push(name);
            exprs.push(Expr::parse(e));
        }
    }
    let external = |name: &str| {
        if let Some(id) = sketch.dimension_by_name(name) {
            let value = sketch
                .constraint(id)
                .and_then(|c| c.dimension.as_ref())
                .map_or(f64::NAN, |d| d.value);
            return External::Value(value);
        }
        match params.entries.iter().find(|p| p.name == name) {
            Some(p) if p.value.is_finite() => External::Value(p.value),
            Some(_) => External::Failed("is a parameter with an error".to_owned()),
            None => External::Unknown,
        }
    };
    let candidates = || {
        table
            .iter()
            .map(|(_, n, _)| (*n).to_owned())
            .chain(params.entries.iter().map(|p| p.name.clone()))
            .chain(std::iter::once("pi".to_owned()))
            .collect()
    };
    let check = |i: usize, v: f64| check_dimension_value(sketch, ids[i], v);
    let results = Graph::run(&names, &exprs, &external, &candidates, &check);
    ids.iter().copied().zip(results).collect()
}

/// Sets a dimension from user input: a plain number clears the expression, anything else
/// is stored as an expression (evaluated against `params` and the sketch's dimension
/// names). Returns the new value. The dimension is left unchanged on error.
///
/// Lengths must be greater than zero and angles in `(0°, 180°]`. The `driving` flag is not
/// touched. Dimensions whose expressions refer to this one are not updated; call
/// [`apply_expressions`] afterwards to propagate.
pub fn set_dimension_input(
    sketch: &mut Sketch,
    params: &Parameters,
    dimension: ConstraintId,
    input: &str,
) -> Result<f64, ExprError> {
    let name = match sketch.constraint(dimension) {
        None => {
            return Err(ExprError::new(format!(
                "constraint {} does not exist",
                dimension.0
            )));
        }
        Some(c) => match &c.dimension {
            None => {
                return Err(ExprError::new(format!(
                    "{} is not a dimension and has no value",
                    c.kind.label()
                )));
            }
            Some(d) => d.name.clone(),
        },
    };
    let input = input.trim();
    let (value, expression) = if let Some(v) = plain_number(input) {
        check_dimension_value(sketch, dimension, v)?;
        (v, None)
    } else {
        let expr = Expr::parse(input)?;
        if let Some((_, col)) = expr.refs().into_iter().find(|(n, _)| *n == name) {
            return Err(ExprError::new(format!(
                "a dimension can't refer to itself ('{name}' at column {col})"
            )));
        }
        let results = evaluate_dimensions(sketch, params, Some((dimension, input)));
        let (_, result) = results
            .into_iter()
            .find(|(id, _)| *id == dimension)
            .expect("the dimension is part of the evaluation");
        (result?, Some(input.to_owned()))
    };
    let d = sketch
        .constraint_mut(dimension)
        .and_then(|c| c.dimension.as_mut())
        .expect("checked above");
    d.value = value;
    d.expression = expression;
    Ok(value)
}

/// Re-evaluates every dimension that has an expression, in dependency order (dimensions
/// may refer to each other). Returns the failures; failed dimensions keep their value.
pub fn apply_expressions(
    sketch: &mut Sketch,
    params: &Parameters,
) -> Vec<(ConstraintId, ExprError)> {
    let results = evaluate_dimensions(sketch, params, None);
    let mut failures = Vec::new();
    for (id, r) in results {
        match r {
            Ok(v) => {
                if let Some(d) = sketch.constraint_mut(id).and_then(|c| c.dimension.as_mut()) {
                    d.value = v;
                }
            }
            Err(e) => failures.push((id, e)),
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use peet_math::DVec2;

    use super::*;
    use crate::sketch::ConstraintKind;

    fn ev(src: &str) -> f64 {
        Expr::parse(src)
            .unwrap_or_else(|e| panic!("{src}: {e}"))
            .eval(&|n| match n {
                "width" => Some(10.0),
                "h" => Some(4.0),
                _ => None,
            })
            .unwrap_or_else(|e| panic!("{src}: {e}"))
    }

    fn err(src: &str) -> String {
        match Expr::parse(src) {
            Err(e) => e.message,
            Ok(x) => x.eval(&|_| Some(1.0)).expect_err(src).message,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn numbers() {
        assert_eq!(ev("12"), 12.0);
        assert_eq!(ev("12.5"), 12.5);
        assert_eq!(ev(".5"), 0.5);
        assert_eq!(ev("1e3"), 1000.0);
        assert_eq!(ev("2.5E-2"), 0.025);
        assert_eq!(ev("1."), 1.0);
    }

    #[test]
    fn precedence_and_associativity() {
        assert_eq!(ev("2 + 3 * 4"), 14.0);
        assert_eq!(ev("(2 + 3) * 4"), 20.0);
        assert_eq!(ev("10 - 4 - 3"), 3.0);
        assert_eq!(ev("16 / 4 / 2"), 2.0);
        assert_eq!(ev("2 ^ 3 ^ 2"), 512.0);
        assert_eq!(ev("-2^2"), -4.0);
        assert_eq!(ev("(-2)^2"), 4.0);
        assert_eq!(ev("2^-1"), 0.5);
        assert_eq!(ev("--3"), 3.0);
        assert_eq!(ev("+3 - -2"), 5.0);
        assert_eq!(ev("2 * width + 5"), 25.0);
        assert_eq!(ev("width / h * 2"), 5.0);
    }

    #[test]
    fn units() {
        assert_eq!(ev("1in"), 25.4);
        assert_eq!(ev("2 cm"), 20.0);
        assert_eq!(ev("1.5m"), 1500.0);
        assert!(close(ev("1ft + 1in"), 330.2));
        assert_eq!(ev("(1 + 1)in"), 50.8);
        assert_eq!(ev("5mm"), 5.0);
        assert_eq!(ev("90deg"), 90.0);
        assert_eq!(ev("45°"), 45.0);
        assert!(close(ev("pi rad"), 180.0));
        assert_eq!(ev("1e1mm"), 10.0);
        assert_eq!(ev("-2in"), -50.8);
    }

    #[test]
    fn functions_in_degrees() {
        assert!(close(ev("sin(30)"), 0.5));
        assert_eq!(ev("sin(90)"), 1.0);
        assert_eq!(ev("cos(90)"), 0.0);
        assert_eq!(ev("cos(180)"), -1.0);
        assert_eq!(ev("tan(45)"), 1.0);
        assert!(close(ev("asin(0.5)"), 30.0));
        assert!(close(ev("acos(0)"), 90.0));
        assert!(close(ev("atan(1)"), 45.0));
        assert!(close(ev("atan2(1, -1)"), 135.0));
        assert_eq!(ev("sqrt(16)"), 4.0);
        assert_eq!(ev("abs(-3)"), 3.0);
        assert_eq!(ev("min(3, 1, 2)"), 1.0);
        assert_eq!(ev("max(3, 1, 2, 7)"), 7.0);
        assert_eq!(ev("min(4)"), 4.0);
        assert_eq!(ev("round(2.5)"), 3.0);
        assert_eq!(ev("floor(-1.5)"), -2.0);
        assert_eq!(ev("ceil(1.2)"), 2.0);
        assert!(close(ev("2 * pi"), std::f64::consts::TAU));
        assert!(close(ev("sin(pi rad / 6)"), 0.5));
        assert_eq!(ev("sqrt(width - 1) * 2"), 6.0);
    }

    #[test]
    fn syntax_errors_have_positions() {
        assert_eq!(err("(1 + 2))"), "unexpected ')' at column 8");
        assert_eq!(err("1 + * 2"), "unexpected '*' at column 5");
        assert_eq!(err("(1 + 2"), "missing ')' to close '(' at column 1");
        assert_eq!(
            err("1 +"),
            "unexpected end of expression, expected a number, a name or '('"
        );
        assert_eq!(err("2 # 3"), "unexpected character '#' at column 3");
        assert!(err("2 x").starts_with("unexpected name 'x' at column 3 — missing an operator?"));
        assert_eq!(err(""), "enter a value or an expression");
        assert_eq!(err("   "), "enter a value or an expression");
        assert_eq!(
            err("sin"),
            "'sin' at column 1 is a function — write sin(...)"
        );
        assert_eq!(err("sin(1, 2)"), "sin at column 1 takes 1 argument, got 2");
        assert_eq!(
            err("atan2(1)"),
            "atan2 at column 1 takes 2 arguments, got 1"
        );
        assert_eq!(
            err("max()"),
            "max at column 1 takes at least 1 argument, got 0"
        );
        assert_eq!(
            err("sine(3)"),
            "unknown function 'sine' at column 1 — did you mean 'sin'?"
        );
        assert_eq!(err("sin(1"), "missing ')' to close '(' at column 4");
    }

    #[test]
    fn evaluation_errors() {
        assert_eq!(err("1 / (2 - 2)"), "division by zero at column 3");
        assert!(err("sqrt(-4)").contains("square root of a negative number"));
        assert!(err("asin(2)").contains("between -1 and 1"));
        assert!(err("tan(90)").contains("undefined"));
        assert!(err("(-8)^0.5").contains("fractional power"));
        assert!(err("10^400").contains("too large"));
        let e = Expr::parse("a + b").unwrap().eval(&|_| None).unwrap_err();
        assert_eq!(e.message, "unknown name 'a' at column 1");
    }

    #[test]
    fn names_are_unique_in_order() {
        let e = Expr::parse("b + a * b + sin(c) + pi").unwrap();
        assert_eq!(e.names(), vec!["b", "a", "c"]);
    }

    #[test]
    fn suggestions() {
        assert_eq!(suggest("widht", ["width", "height"]), Some("width"));
        assert_eq!(suggest("Width", ["width"]), Some("width"));
        assert_eq!(suggest("xyz", ["width"]), None);
        assert_eq!(suggest("d", ["d1", "d2"]), Some("d1"));
    }

    #[test]
    fn name_validation() {
        assert!(check_name("width_2").is_ok());
        assert!(check_name("_x").is_ok());
        assert!(check_name("2x").is_err());
        assert!(check_name("a-b").is_err());
        assert!(check_name("").is_err());
        assert!(check_name("sin").unwrap_err().message.contains("function"));
        assert!(check_name("pi").unwrap_err().message.contains("constant"));
        assert!(check_name("mm").unwrap_err().message.contains("unit"));
    }

    #[test]
    fn plain_numbers() {
        assert_eq!(plain_number(" 12.5 "), Some(12.5));
        assert_eq!(plain_number("-3"), Some(-3.0));
        assert_eq!(plain_number("12mm"), None);
        assert_eq!(plain_number("1+1"), None);
        assert_eq!(plain_number("inf"), None);
        assert_eq!(plain_number("NaN"), None);
    }

    #[test]
    fn parameters_evaluate_in_dependency_order() {
        let mut p = Parameters::default();
        // Declared before the parameters it uses.
        p.entries.push(Parameter {
            name: "area".into(),
            expression: "width * height".into(),
            value: f64::NAN,
        });
        p.entries.push(Parameter {
            name: "width".into(),
            expression: "2 * height".into(),
            value: f64::NAN,
        });
        p.entries.push(Parameter {
            name: "height".into(),
            expression: "5".into(),
            value: f64::NAN,
        });
        assert!(p.evaluate().is_empty());
        assert_eq!(p.get("area"), Some(50.0));
        assert_eq!(p.set("height", "10").unwrap(), 10.0);
        assert_eq!(p.get("area"), Some(200.0));
        assert_eq!(p.get("nope"), None);
    }

    #[test]
    fn parameter_set_validates() {
        let mut p = Parameters::default();
        p.set("width", "20").unwrap();
        assert!(p.set("sin", "1").is_err());
        assert!(p.set("2x", "1").is_err());
        let e = p.set("h", "widht / 2").unwrap_err();
        assert_eq!(
            e.message,
            "unknown name 'widht' at column 1 — did you mean 'width'?"
        );
        assert_eq!(p.entries.len(), 1, "failed set leaves the table unchanged");
        // Replacing keeps the position.
        p.set("h", "width / 2").unwrap();
        assert_eq!(p.set("width", "30").unwrap(), 30.0);
        assert_eq!(p.get("h"), Some(15.0));
        // Would break h: rejected, table unchanged.
        p.set("d", "1").unwrap();
        p.set("h", "width / d").unwrap();
        let e = p.set("d", "0").unwrap_err();
        assert!(e.message.contains("break 'h'"), "{}", e.message);
        assert_eq!(p.get("d"), Some(1.0));
    }

    #[test]
    fn parameter_cycles() {
        let mut p = Parameters::default();
        p.set("a", "1").unwrap();
        p.set("b", "a + 1").unwrap();
        let e = p.set("a", "b * 2").unwrap_err();
        assert_eq!(e.message, "circular reference: a → b → a");
        assert_eq!(p.get("a"), Some(1.0));
        let e = p.set("c", "c + 1").unwrap_err();
        assert_eq!(e.message, "circular reference: c → c");

        // A cycle loaded from a file: every member reports it, dependents fail.
        let mut q = Parameters::default();
        for (n, e) in [("x", "y"), ("y", "x"), ("z", "x + 1")] {
            q.entries.push(Parameter {
                name: n.into(),
                expression: e.into(),
                value: 0.0,
            });
        }
        let errors = q.evaluate();
        assert_eq!(errors.len(), 3);
        assert_eq!(errors[0].1.message, "circular reference: x → y → x");
        assert_eq!(errors[1].1.message, "circular reference: y → x → y");
        assert_eq!(errors[2].1.message, "'x' at column 1 has an error");
        assert!(q.get("x").is_none() && q.entries[0].value.is_nan());
    }

    #[test]
    fn parameter_remove() {
        let mut p = Parameters::default();
        p.set("a", "2").unwrap();
        p.set("b", "a * 3").unwrap();
        p.remove("a");
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.get("b"), None);
    }

    fn sketch_with_dims() -> (Sketch, ConstraintId, ConstraintId, ConstraintId) {
        let mut s = Sketch::new();
        let l = s.add_line(DVec2::ZERO, DVec2::new(10.0, 0.0));
        let c = s.add_circle(DVec2::new(20.0, 0.0), 2.0);
        let m = s.add_line(DVec2::ZERO, DVec2::new(0.0, 5.0));
        let d1 = s.add_constraint(ConstraintKind::Length(l)).unwrap();
        let d2 = s.add_constraint(ConstraintKind::Radius(c)).unwrap();
        let d3 = s.add_constraint(ConstraintKind::Angle(l, m)).unwrap();
        (s, d1, d2, d3)
    }

    fn dim(s: &Sketch, id: ConstraintId) -> (f64, Option<String>) {
        let d = s.constraint(id).unwrap().dimension.as_ref().unwrap();
        (d.value, d.expression.clone())
    }

    #[test]
    fn dimension_plain_number_clears_expression() {
        let (mut s, d1, d2, _) = sketch_with_dims();
        let p = Parameters::default();
        assert_eq!(set_dimension_input(&mut s, &p, d2, "d1 / 4").unwrap(), 2.5);
        assert_eq!(dim(&s, d2), (2.5, Some("d1 / 4".into())));
        assert_eq!(set_dimension_input(&mut s, &p, d2, " 3 ").unwrap(), 3.0);
        assert_eq!(dim(&s, d2), (3.0, None));
        let _ = d1;
    }

    #[test]
    fn dimension_names_beat_parameters() {
        let (mut s, _, d2, _) = sketch_with_dims();
        let mut p = Parameters::default();
        p.set("d1", "100").unwrap();
        p.set("hole", "3").unwrap();
        assert_eq!(set_dimension_input(&mut s, &p, d2, "d1 / 5").unwrap(), 2.0);
        assert!(close(
            set_dimension_input(&mut s, &p, d2, "hole * 1in").unwrap(),
            76.2
        ));
    }

    #[test]
    fn dimension_input_errors_leave_it_unchanged() {
        let (mut s, d1, d2, d3) = sketch_with_dims();
        let p = Parameters::default();
        let before = s.clone();
        let e = set_dimension_input(&mut s, &p, d1, "d1 * 2").unwrap_err();
        assert!(e.message.contains("can't refer to itself"), "{}", e.message);
        let e = set_dimension_input(&mut s, &p, d1, "d4 + 1").unwrap_err();
        assert!(
            e.message
                .contains("unknown name 'd4' at column 1 — did you mean 'd1'?")
        );
        let e = set_dimension_input(&mut s, &p, d1, "-5").unwrap_err();
        assert!(e.message.contains("greater than zero"), "{}", e.message);
        let e = set_dimension_input(&mut s, &p, d1, "d2 - 2").unwrap_err();
        assert!(e.message.contains("greater than zero"), "{}", e.message);
        let e = set_dimension_input(&mut s, &p, d3, "200").unwrap_err();
        assert!(e.message.contains("at most 180°"), "{}", e.message);
        assert!(set_dimension_input(&mut s, &p, d3, "0deg").is_err());
        let e = set_dimension_input(&mut s, &p, d1, "1 / 0").unwrap_err();
        assert_eq!(e.message, "division by zero at column 3");
        let e = set_dimension_input(&mut s, &p, d1, "10 +").unwrap_err();
        assert!(e.message.starts_with("unexpected end"));
        assert_eq!(s, before);
        // Angles up to 180 are fine.
        assert!(close(
            set_dimension_input(&mut s, &p, d3, "pi rad").unwrap(),
            180.0
        ));
        let _ = d2;
    }

    #[test]
    fn dimension_cycles_are_rejected() {
        let (mut s, d1, d2, _) = sketch_with_dims();
        let p = Parameters::default();
        set_dimension_input(&mut s, &p, d2, "d1 / 2").unwrap();
        let e = set_dimension_input(&mut s, &p, d1, "d2 * 2").unwrap_err();
        assert_eq!(e.message, "circular reference: d1 → d2 → d1");
        assert_eq!(dim(&s, d1), (10.0, None));
    }

    #[test]
    fn dimension_parameter_with_error() {
        let (mut s, d1, _, _) = sketch_with_dims();
        let mut p = Parameters::default();
        p.entries.push(Parameter {
            name: "bad".into(),
            expression: "1 / 0".into(),
            value: f64::NAN,
        });
        p.evaluate();
        let e = set_dimension_input(&mut s, &p, d1, "bad + 1").unwrap_err();
        assert_eq!(e.message, "'bad' at column 1 is a parameter with an error");
    }

    #[test]
    fn not_a_dimension() {
        let mut s = Sketch::new();
        let l = s.add_line(DVec2::ZERO, DVec2::X);
        let h = s.add_constraint(ConstraintKind::Horizontal(l)).unwrap();
        let p = Parameters::default();
        assert!(set_dimension_input(&mut s, &p, h, "1").is_err());
        assert!(set_dimension_input(&mut s, &p, ConstraintId(99), "1").is_err());
    }

    #[test]
    fn apply_expressions_in_dependency_order() {
        let (mut s, d1, d2, d3) = sketch_with_dims();
        let mut p = Parameters::default();
        p.set("w", "40").unwrap();
        p.set("ang", "30").unwrap();
        // d1 depends on d2 which depends on w: set d1 first to test ordering.
        set_dimension_input(&mut s, &p, d2, "w / 8").unwrap();
        set_dimension_input(&mut s, &p, d1, "d2 * 4").unwrap();
        set_dimension_input(&mut s, &p, d3, "ang * 2").unwrap();
        assert_eq!(dim(&s, d1).0, 20.0);
        p.set("w", "80").unwrap();
        p.set("ang", "50").unwrap();
        // Force d1 to evaluate before d2 by id order: d1 < d2 but depends on it.
        assert!(apply_expressions(&mut s, &p).is_empty());
        assert_eq!(dim(&s, d2).0, 10.0);
        assert_eq!(dim(&s, d1).0, 40.0);
        assert_eq!(dim(&s, d3).0, 100.0);
    }

    #[test]
    fn apply_expressions_reports_failures() {
        let (mut s, d1, d2, d3) = sketch_with_dims();
        let mut p = Parameters::default();
        p.set("w", "40").unwrap();
        set_dimension_input(&mut s, &p, d2, "w / 8").unwrap();
        set_dimension_input(&mut s, &p, d1, "d2 * 4").unwrap();
        set_dimension_input(&mut s, &p, d3, "45").unwrap();
        p.set("w", "-8").unwrap();
        let failures = apply_expressions(&mut s, &p);
        assert_eq!(failures.len(), 2);
        let f2 = &failures.iter().find(|(id, _)| *id == d2).unwrap().1;
        assert!(f2.message.contains("greater than zero"), "{}", f2.message);
        let f1 = &failures.iter().find(|(id, _)| *id == d1).unwrap().1;
        assert_eq!(f1.message, "'d2' at column 1 has an error");
        assert_eq!(dim(&s, d2).0, 5.0, "unchanged");
        assert_eq!(dim(&s, d1).0, 20.0, "unchanged");

        p.remove("w");
        let failures = apply_expressions(&mut s, &p);
        let f2 = &failures.iter().find(|(id, _)| *id == d2).unwrap().1;
        assert_eq!(f2.message, "unknown name 'w' at column 1");
    }

    #[test]
    fn apply_expressions_detects_cycles_from_files() {
        let (mut s, d1, d2, _) = sketch_with_dims();
        s.constraint_mut(d1)
            .unwrap()
            .dimension
            .as_mut()
            .unwrap()
            .expression = Some("d2".into());
        s.constraint_mut(d2)
            .unwrap()
            .dimension
            .as_mut()
            .unwrap()
            .expression = Some("d1".into());
        let failures = apply_expressions(&mut s, &Parameters::default());
        assert_eq!(failures.len(), 2);
        assert_eq!(failures[0].1.message, "circular reference: d1 → d2 → d1");
    }
}
