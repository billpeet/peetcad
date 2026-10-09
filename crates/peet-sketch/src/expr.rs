//! Expressions and named parameters for dimension values (`width = 2 * height + 5`).
//!
//! # Grammar
//!
//! ```text
//! compare  = sum [ ("<" | "<=" | ">" | ">=" | "==" | "=" | "!=" | "<>") sum ]
//! sum      = product { ("+" | "-") product }
//! product  = unary { ("*" | "/") unary }
//! unary    = ("-" | "+") unary | power
//! power    = postfix [ "^" unary ]          (right associative: 2^3^2 = 2^9)
//! postfix  = number [unit] | "(" compare ")" [unit] | name "(" [compare {"," compare}] ")" | name
//! number   = 12 | 12.5 | .5 | 1e3 | 2.5E-2
//! name     = [A-Za-z_][A-Za-z0-9_]*
//! unit     = (mm | cm | m | in | " | ft | deg | rad | °) [ "^" ["-"] digit ]
//! ```
//!
//! `^` binds tighter than unary minus, so `-2^2 = -4` (as in mathematics). A single-digit
//! power directly after a unit belongs to the unit: `4mm^2` is 4 mm², not (4 mm)².
//!
//! # Units
//!
//! Every value is a [`Quantity`]: a number with a dimension ([`Dim`], powers of length and
//! of angle). Lengths are held in millimetres and angles in degrees, whatever the document
//! units are. `5mm * 2mm` is an area, `10mm / 2mm` a plain number, `sqrt(4mm^2)` is 2 mm.
//!
//! A **plain number** has no unit and adopts one where it is needed:
//!
//! * combined with a quantity by `+`, `-`, `min`, `max` or `atan2` it takes that quantity's
//!   dimension, in **document units** ([`Units`]): `5mm + 3` is 5 mm + 3 in in an inch
//!   document, `30deg + 15` is 45°;
//! * where a result is consumed, a kind is expected ([`QuantityKind`]): a length dimension
//!   takes a plain number as a length in document units, an angle dimension as degrees.
//!
//! Adding two different dimensions (`10mm + 30deg`) and a result of the wrong dimension
//! (an area for a length) are errors.
//!
//! **Bare numbers in stored expressions follow the document unit.** `d1 = width / 2 + 3`
//! changes when the document switches from mm to inches: the `3` becomes 3 in, and so does
//! a `width = 100` parameter, which is a plain number. Write the unit (`3mm`,
//! `width = 100mm`) to pin a value. Dimensions entered as a number, with or without a unit
//! (`12`, `2in`), are stored as plain values in mm and never change.
//!
//! [`Expr::eval`] ignores all of this and treats units as scale factors to mm and degrees;
//! [`Expr::eval_quantity`] is the unit-aware evaluation.
//!
//! # Functions
//!
//! `sin cos tan asin acos atan atan2 sqrt abs min max round floor ceil int if iif`.
//! `if(condition, yes, no)` and its alias `iif` evaluate only the selected branch.
//! Conditions are plain numbers, zero false and nonzero true. Comparisons return 0 or 1,
//! adopting document units for bare numbers. `int` is an alias for `floor`.
//! `min`/`max` take one or more arguments, `atan2(y, x)` two, the others one. The constant `pi` is π, a
//! plain number.
//!
//! * `sin cos tan` take an angle; a plain number is in **degrees**: `sin(30)` and
//!   `sin(30deg)` are both 0.5. `asin acos atan atan2` return angles: `atan2(1, 1)` is 45°.
//! * `sqrt` halves the dimension, so it needs a plain number, an area, …
//! * `abs round floor ceil` keep the dimension. Rounding works in document units:
//!   `round(0.6in)` is 1 in in an inch document and 15 mm in a mm document.
//!
//! # Names
//!
//! Names refer to dimensions of the sketch (`d1`, or a name the user gave it) and to
//! [`Parameters`]; dimension names win when both exist. A dimension is a length or an
//! angle; a parameter is whatever its expression gives ([`Parameter::kind`]).

use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

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

// ---- Units and quantities ----

/// A unit of length a document can work in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LengthUnit {
    #[default]
    Mm,
    Cm,
    M,
    Inch,
    Ft,
}

impl LengthUnit {
    pub const ALL: [LengthUnit; 5] = [Self::Mm, Self::Cm, Self::M, Self::Inch, Self::Ft];

    /// The name for menus: "Millimetres".
    pub fn label(self) -> &'static str {
        match self {
            Self::Mm => "Millimetres",
            Self::Cm => "Centimetres",
            Self::M => "Metres",
            Self::Inch => "Inches",
            Self::Ft => "Feet",
        }
    }

    /// The suffix written after a value, which is also the unit's name in expressions.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Mm => "mm",
            Self::Cm => "cm",
            Self::M => "m",
            Self::Inch => "in",
            Self::Ft => "ft",
        }
    }

    pub fn mm_per_unit(self) -> f64 {
        match self {
            Self::Mm => 1.0,
            Self::Cm => 10.0,
            Self::M => 1000.0,
            Self::Inch => 25.4,
            Self::Ft => 304.8,
        }
    }

    /// Decimals shown for a length: about a micrometre in every unit.
    fn decimals(self) -> usize {
        match self {
            Self::Mm => 3,
            Self::Cm | Self::Inch => 4,
            Self::Ft => 5,
            Self::M => 6,
        }
    }
}

/// The units a document displays and takes plain numbers in. Angles are always degrees.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Units {
    #[serde(default)]
    pub length: LengthUnit,
}

impl Units {
    pub fn new(length: LengthUnit) -> Self {
        Self { length }
    }

    /// A length in document units, in mm.
    pub fn to_mm(&self, v: f64) -> f64 {
        v * self.length.mm_per_unit()
    }

    /// A length in mm, in document units.
    pub fn from_mm(&self, mm: f64) -> f64 {
        mm / self.length.mm_per_unit()
    }

    /// "12.5 mm", "0.75 in": the value in document units, trimmed to a sensible number of
    /// decimals, with the suffix.
    pub fn format_length(&self, mm: f64) -> String {
        format!("{} {}", self.format_length_value(mm), self.length.suffix())
    }

    /// The same without the suffix (for edit fields and compact labels).
    pub fn format_length_value(&self, mm: f64) -> String {
        trim_decimals(self.from_mm(mm), self.length.decimals())
    }

    /// "45°", "22.5°".
    pub fn format_angle(&self, degrees: f64) -> String {
        format!("{}°", trim_decimals(degrees, ANGLE_DECIMALS))
    }

    /// Base units (mm^n) per document unit for a dimension; angles are degrees already.
    fn scale(&self, dim: Dim) -> f64 {
        self.length.mm_per_unit().powi(i32::from(dim.length))
    }
}

const ANGLE_DECIMALS: usize = 3;
const NUMBER_DECIMALS: usize = 6;

/// `v` with at most `decimals` decimals, without trailing zeros and without "-0".
fn trim_decimals(v: f64, decimals: usize) -> String {
    if !v.is_finite() {
        return format!("{v}");
    }
    if v.abs() >= 1e15 {
        return format!("{v:e}");
    }
    let mut s = format!("{v:.decimals$}");
    if s.contains('.') {
        let keep = s.trim_end_matches('0').trim_end_matches('.').len();
        s.truncate(keep);
    }
    if s == "-0" { "0".to_owned() } else { s }
}

/// The dimension of a quantity: powers of length and of angle. `{1, 0}` is a length,
/// `{2, 0}` an area, `{0, 0}` a plain number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Dim {
    pub length: i8,
    pub angle: i8,
}

/// Largest power of a unit an expression may build up.
const MAX_POWER: i8 = 9;

impl Dim {
    pub const NONE: Dim = Dim::new(0, 0);
    pub const LENGTH: Dim = Dim::new(1, 0);
    pub const ANGLE: Dim = Dim::new(0, 1);
    const AREA: Dim = Dim::new(2, 0);
    const VOLUME: Dim = Dim::new(3, 0);

    pub const fn new(length: i8, angle: i8) -> Self {
        Self { length, angle }
    }

    pub fn is_none(self) -> bool {
        self == Self::NONE
    }

    /// The dimension to a power, if the result has whole powers within bounds.
    fn powf(self, power: f64) -> Option<Dim> {
        let p = |e: i8| {
            let v = f64::from(e) * power;
            let r = v.round();
            ((v - r).abs() < 1e-9 && r.abs() <= f64::from(MAX_POWER)).then_some(r as i8)
        };
        Some(Dim::new(p(self.length)?, p(self.angle)?))
    }

    fn plus(self, other: Dim) -> Option<Dim> {
        let s = |a: i8, b: i8| Some(a + b).filter(|e| e.abs() <= MAX_POWER);
        Some(Dim::new(
            s(self.length, other.length)?,
            s(self.angle, other.angle)?,
        ))
    }

    fn negated(self) -> Dim {
        Dim::new(-self.length, -self.angle)
    }

    /// "a length", "an area"… for the dimensions that have a name.
    fn name(self) -> Option<&'static str> {
        match self {
            Self::NONE => Some("a plain number"),
            Self::LENGTH => Some("a length"),
            Self::AREA => Some("an area"),
            Self::VOLUME => Some("a volume"),
            Self::ANGLE => Some("an angle"),
            _ => None,
        }
    }

    /// The unit written after a value: "mm", "in²", "°", "mm·deg".
    fn unit_label(self, units: &Units) -> String {
        let mut parts = Vec::new();
        if self.length != 0 {
            parts.push(format!(
                "{}{}",
                units.length.suffix(),
                superscript(self.length)
            ));
        }
        if self == Self::ANGLE {
            parts.push("°".to_owned());
        } else if self.angle != 0 {
            parts.push(format!("deg{}", superscript(self.angle)));
        }
        parts.join("·")
    }
}

/// "" for 1, "²" for 2, "⁻¹" for -1.
fn superscript(power: i8) -> String {
    if power == 1 {
        return String::new();
    }
    power
        .to_string()
        .chars()
        .map(|c| match c {
            '-' => '⁻',
            '0' => '⁰',
            '1' => '¹',
            '2' => '²',
            '3' => '³',
            '4' => '⁴',
            '5' => '⁵',
            '6' => '⁶',
            '7' => '⁷',
            '8' => '⁸',
            _ => '⁹',
        })
        .collect()
}

/// What a value is used as: the dimensions a parameter or an input can have.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuantityKind {
    /// A plain number; it adopts a unit where it is used.
    #[default]
    Number,
    /// A length, in mm.
    Length,
    /// An angle, in degrees.
    Angle,
}

impl QuantityKind {
    pub fn dim(self) -> Dim {
        match self {
            Self::Number => Dim::NONE,
            Self::Length => Dim::LENGTH,
            Self::Angle => Dim::ANGLE,
        }
    }
}

/// A number with a dimension. The value is in base units: mm for each power of length,
/// degrees for each power of angle (an area is in mm²).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quantity {
    pub value: f64,
    pub dim: Dim,
}

impl Quantity {
    pub fn new(value: f64, kind: QuantityKind) -> Self {
        Self {
            value,
            dim: kind.dim(),
        }
    }

    /// A plain number.
    pub fn number(value: f64) -> Self {
        Self::new(value, QuantityKind::Number)
    }

    pub fn length(mm: f64) -> Self {
        Self::new(mm, QuantityKind::Length)
    }

    pub fn angle(degrees: f64) -> Self {
        Self::new(degrees, QuantityKind::Angle)
    }

    /// Number, length or angle; `None` for anything else (an area, a length per angle…).
    pub fn kind(&self) -> Option<QuantityKind> {
        match self.dim {
            Dim::NONE => Some(QuantityKind::Number),
            Dim::LENGTH => Some(QuantityKind::Length),
            Dim::ANGLE => Some(QuantityKind::Angle),
            _ => None,
        }
    }

    /// The value where a `kind` is expected, in base units (mm, degrees or the plain
    /// number). A plain number is taken in document units; any other mismatch is an error.
    pub fn expect(&self, kind: QuantityKind, units: &Units) -> Result<f64, ExprError> {
        self.expect_for("this value", kind, units)
    }

    /// [`Self::expect`], with the subject of the error message ("this dimension").
    fn expect_for(&self, what: &str, kind: QuantityKind, units: &Units) -> Result<f64, ExprError> {
        let want = kind.dim();
        if self.dim == want {
            return Ok(self.value);
        }
        if self.dim.is_none() {
            return Ok(self.value * units.scale(want));
        }
        // What to divide (or multiply) by to get there, when that is something with a name.
        let hint = self
            .dim
            .plus(want.negated())
            .and_then(|extra| match (extra.name(), extra.negated().name()) {
                (Some(n), _) => Some(format!(": divide by {n}")),
                (_, Some(n)) => Some(format!(": multiply by {n}")),
                _ => None,
            })
            .unwrap_or_default();
        Err(ExprError::new(format!(
            "{what} needs {}, but the expression gives {}{hint}",
            want.name().expect("kinds have names"),
            self.describe(units)
        )))
    }

    /// "12.5 mm", "0.75 in", "45°", "10 mm²", "3": the value in document units.
    pub fn display(&self, units: &Units) -> String {
        let v = self.value / units.scale(self.dim);
        match self.dim {
            Dim::NONE => trim_decimals(v, NUMBER_DECIMALS),
            Dim::LENGTH => units.format_length(self.value),
            Dim::ANGLE => units.format_angle(v),
            dim => format!(
                "{} {}",
                trim_decimals(v, NUMBER_DECIMALS),
                dim.unit_label(units)
            ),
        }
    }

    /// "a length (10 mm)", for messages.
    fn describe(&self, units: &Units) -> String {
        format!(
            "{} ({})",
            self.dim.name().unwrap_or("a compound quantity"),
            self.display(units)
        )
    }
}

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
    Int,
    If,
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
    ("int", Func::Int),
    ("if", Func::If),
    ("iif", Func::If),
];

/// A unit suffix: its factor to base units (mm, degrees) and what it measures.
#[derive(Clone, Copy, Debug, PartialEq)]
struct UnitDef {
    name: &'static str,
    factor: f64,
    dim: Dim,
}

const fn unit_def(name: &'static str, factor: f64, dim: Dim) -> UnitDef {
    UnitDef { name, factor, dim }
}

const UNITS: &[UnitDef] = &[
    unit_def("mm", 1.0, Dim::LENGTH),
    unit_def("cm", 10.0, Dim::LENGTH),
    unit_def("m", 1000.0, Dim::LENGTH),
    unit_def("in", 25.4, Dim::LENGTH),
    unit_def("ft", 304.8, Dim::LENGTH),
    unit_def("deg", 1.0, Dim::ANGLE),
    unit_def("rad", 180.0 / PI, Dim::ANGLE),
];

/// `°` and `"`, which the lexer reads as tokens of their own.
const DEGREE_SIGN: UnitDef = unit_def("°", 1.0, Dim::ANGLE);
const INCH_MARK: UnitDef = unit_def("\"", 25.4, Dim::LENGTH);

const CONSTANTS: &[(&str, f64)] = &[("pi", PI)];

fn function(name: &str) -> Option<Func> {
    FUNCTIONS.iter().find(|(n, _)| *n == name).map(|(_, f)| *f)
}

fn unit(name: &str) -> Option<UnitDef> {
    UNITS.iter().find(|u| u.name == name).copied()
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
            Func::If => (3, Some(3)),
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
    Compare(BinOp),
    LParen,
    RParen,
    Comma,
    /// `°`
    Degree,
    /// `"` (inches)
    Inch,
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

impl Token {
    fn is_num(&self) -> bool {
        matches!(self.tok, Tok::Num(_))
    }
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
                '<' | '>' | '=' | '!' => {
                    let next = chars.get(i).copied();
                    let op = match (c, next) {
                        ('<', Some('=')) => BinOp::Le,
                        ('>', Some('=')) => BinOp::Ge,
                        ('=', Some('=')) => BinOp::Eq,
                        ('!', Some('=')) | ('<', Some('>')) => BinOp::Ne,
                        ('<', _) => BinOp::Lt,
                        ('>', _) => BinOp::Gt,
                        ('=', _) => BinOp::Eq,
                        _ => return Err(ExprError::new(format!("expected '!=' at column {col}"))),
                    };
                    if matches!(next, Some('=')) || (c == '<' && next == Some('>')) {
                        i += 1;
                    }
                    Tok::Compare(op)
                }
                '(' => Tok::LParen,
                ')' => Tok::RParen,
                ',' => Tok::Comma,
                '°' => Tok::Degree,
                '"' => Tok::Inch,
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

/// The unit a token names, if it is one.
fn unit_of(tok: &Tok) -> Option<UnitDef> {
    match tok {
        Tok::Degree => Some(DEGREE_SIGN),
        Tok::Inch => Some(INCH_MARK),
        Tok::Ident(name) => unit(name),
        _ => None,
    }
}

/// Whether `src` is just a number, optionally signed and optionally with a unit (`12`,
/// `-3.5`, `2in`, `45°`): a value rather than an expression.
fn is_literal(src: &str) -> bool {
    let Ok(tokens) = lex(src) else { return false };
    let mut rest = tokens.as_slice();
    if let [first, tail @ ..] = rest
        && matches!(first.tok, Tok::Op('+' | '-'))
    {
        rest = tail;
    }
    match rest {
        [num, end] => num.is_num() && end.tok == Tok::End,
        [num, suffix, end] => num.is_num() && unit_of(&suffix.tok).is_some() && end.tok == Tok::End,
        _ => false,
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
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

#[derive(Clone, Debug, PartialEq)]
enum Node {
    Num(f64),
    Name {
        name: String,
        col: usize,
    },
    Neg(Box<Node>),
    /// A value with a unit suffix: `2in`, `(a + b) mm`, `4mm^2` (`power` 2).
    Unit {
        value: Box<Node>,
        unit: UnitDef,
        power: i8,
        col: usize,
    },
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

    fn compare(&mut self) -> Result<Node, ExprError> {
        let lhs = self.sum()?;
        if let Tok::Compare(op) = self.peek().tok {
            let col = self.next().col;
            let rhs = self.sum()?;
            if matches!(self.peek().tok, Tok::Compare(_)) {
                return Err(ExprError::new(format!(
                    "chained comparisons at column {} need separate conditions",
                    self.peek().col
                )));
            }
            return Ok(Node::Bin {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                col,
            });
        }
        Ok(lhs)
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
                let inner = self.compare()?;
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
        let Some(unit) = unit_of(&self.peek().tok) else {
            return Ok(value);
        };
        let col = self.next().col;
        Ok(Node::Unit {
            value: Box::new(value),
            unit,
            power: self.unit_power(),
            col,
        })
    }

    /// A whole power right after a unit (`mm^2`, `mm^-1`) belongs to the unit. Anything
    /// else after `^` is left to [`Self::power`].
    fn unit_power(&mut self) -> i8 {
        if self.peek().tok != Tok::Op('^') {
            return 1;
        }
        let at = |i: usize| self.tokens.get(self.pos + i).map(|t| &t.tok);
        let (sign, len) = match at(1) {
            Some(Tok::Op('-')) => (-1, 3),
            _ => (1, 2),
        };
        let Some(Tok::Num(v)) = at(len - 1) else {
            return 1;
        };
        if v.fract() != 0.0 || !(1.0..=f64::from(MAX_POWER)).contains(v) {
            return 1;
        }
        let power = sign * *v as i8;
        self.pos += len;
        power
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
                args.push(self.compare()?);
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
        let root = p.compare()?;
        let t = p.next();
        if t.tok != Tok::End {
            return Err(p.unexpected(&t, "the end of the expression"));
        }
        Ok(Self { root })
    }

    /// Evaluates with `lookup` resolving names to values, **without units**: unit suffixes
    /// are scale factors to mm and degrees and nothing is checked (`1in + 30deg` is 55.4).
    /// Use [`Self::eval_quantity`] for anything a user typed.
    pub fn eval(&self, lookup: &dyn Fn(&str) -> Option<f64>) -> Result<f64, ExprError> {
        let eval = Eval {
            units: Units::default(),
            checked: false,
            lookup: &|name| lookup(name).map(Quantity::number),
        };
        eval.node(&self.root).map(|q| q.value)
    }

    /// Evaluates with units: the result carries its dimension, plain numbers adopt
    /// `units` where they meet a quantity, and mixing dimensions is an error. Use
    /// [`Quantity::expect`] on the result to get a length or an angle.
    pub fn eval_quantity(
        &self,
        units: &Units,
        lookup: &dyn Fn(&str) -> Option<Quantity>,
    ) -> Result<Quantity, ExprError> {
        let eval = Eval {
            units: *units,
            checked: true,
            lookup,
        };
        eval.node(&self.root)
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
                Node::Neg(a) | Node::Unit { value: a, .. } => walk(a, out),
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

struct Eval<'a> {
    units: Units,
    /// Whether dimensions are tracked. If not, every value is a plain number.
    checked: bool,
    lookup: &'a dyn Fn(&str) -> Option<Quantity>,
}

impl Eval<'_> {
    fn dim(&self, dim: Dim) -> Dim {
        if self.checked { dim } else { Dim::NONE }
    }

    /// Brings two values to one dimension: a plain number adopts the dimension of the
    /// other, in document units. `None` if they have different dimensions.
    fn unify(&self, a: Quantity, b: Quantity) -> Option<(f64, f64, Dim)> {
        if a.dim == b.dim {
            Some((a.value, b.value, a.dim))
        } else if a.dim.is_none() {
            Some((a.value * self.units.scale(b.dim), b.value, b.dim))
        } else if b.dim.is_none() {
            Some((a.value, b.value * self.units.scale(a.dim), a.dim))
        } else {
            None
        }
    }

    fn node(&self, n: &Node) -> Result<Quantity, ExprError> {
        let units = &self.units;
        match n {
            Node::Num(v) => Ok(Quantity::number(*v)),
            Node::Name { name, col } => (self.lookup)(name)
                .ok_or_else(|| ExprError::new(format!("unknown name '{name}' at column {col}"))),
            Node::Neg(a) => {
                let q = self.node(a)?;
                Ok(Quantity {
                    value: -q.value,
                    dim: q.dim,
                })
            }
            Node::Unit {
                value,
                unit,
                power,
                col,
            } => {
                let q = self.node(value)?;
                if !q.dim.is_none() {
                    return Err(ExprError::new(format!(
                        "'{}' at column {col}: the value already has a unit, it is {}",
                        unit.name,
                        q.describe(units)
                    )));
                }
                let v = q.value * unit.factor.powi(i32::from(*power));
                if !v.is_finite() {
                    return Err(ExprError::new(format!(
                        "the result at column {col} is too large"
                    )));
                }
                let dim = unit
                    .dim
                    .powf(f64::from(*power))
                    .expect("the parser bounds unit powers");
                Ok(Quantity {
                    value: v,
                    dim: self.dim(dim),
                })
            }
            Node::Bin { op, lhs, rhs, col } => {
                let a = self.node(lhs)?;
                let b = self.node(rhs)?;
                let too_high = || {
                    ExprError::new(format!(
                        "the units at column {col} are raised to too high a power"
                    ))
                };
                let q = match op {
                    BinOp::Add | BinOp::Sub => {
                        let add = *op == BinOp::Add;
                        let Some((x, y, dim)) = self.unify(a, b) else {
                            let (a, b) = (a.describe(units), b.describe(units));
                            return Err(ExprError::new(if add {
                                format!("can't add {a} and {b}")
                            } else {
                                format!("can't subtract {b} from {a}")
                            }));
                        };
                        Quantity {
                            value: if add { x + y } else { x - y },
                            dim,
                        }
                    }
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne => {
                        let Some((x, y, _)) = self.unify(a, b) else {
                            return Err(ExprError::new(format!(
                                "can't compare {} and {} at column {col}",
                                a.describe(units),
                                b.describe(units)
                            )));
                        };
                        if !x.is_finite() || !y.is_finite() {
                            return Err(ExprError::new(format!(
                                "comparison at column {col} needs finite values"
                            )));
                        }
                        Quantity::number(
                            if match op {
                                BinOp::Lt => x < y,
                                BinOp::Le => x <= y,
                                BinOp::Gt => x > y,
                                BinOp::Ge => x >= y,
                                BinOp::Eq => x == y,
                                BinOp::Ne => x != y,
                                _ => unreachable!(),
                            } {
                                1.0
                            } else {
                                0.0
                            },
                        )
                    }
                    BinOp::Mul => Quantity {
                        value: a.value * b.value,
                        dim: a.dim.plus(b.dim).ok_or_else(too_high)?,
                    },
                    BinOp::Div => {
                        if b.value == 0.0 {
                            return Err(ExprError::new(format!(
                                "division by zero at column {col}"
                            )));
                        }
                        Quantity {
                            value: a.value / b.value,
                            dim: a.dim.plus(b.dim.negated()).ok_or_else(too_high)?,
                        }
                    }
                    BinOp::Pow => {
                        if !b.dim.is_none() {
                            return Err(ExprError::new(format!(
                                "the exponent at column {col} must be a plain number, but it is {}",
                                b.describe(units)
                            )));
                        }
                        let (x, e) = (a.value, b.value);
                        if x < 0.0 && e.fract() != 0.0 {
                            return Err(ExprError::new(format!(
                                "can't raise a negative number ({}) to a fractional power ({}) at column {col}",
                                fmt_num(x),
                                fmt_num(e)
                            )));
                        }
                        if x == 0.0 && e < 0.0 {
                            return Err(ExprError::new(format!(
                                "division by zero (0 to a negative power) at column {col}"
                            )));
                        }
                        let dim = a.dim.powf(e).ok_or_else(|| {
                            if e.fract() == 0.0 {
                                too_high()
                            } else {
                                ExprError::new(format!(
                                    "can't raise {} to the power {} at column {col}: the result has no whole unit",
                                    a.describe(units),
                                    fmt_num(e)
                                ))
                            }
                        })?;
                        Quantity {
                            value: x.powf(e),
                            dim,
                        }
                    }
                };
                if !q.value.is_finite() {
                    return Err(ExprError::new(format!(
                        "the result at column {col} is too large"
                    )));
                }
                Ok(q)
            }
            Node::Call {
                func: Func::If,
                args,
                col,
            } => {
                let condition = self.node(&args[0])?;
                if !condition.dim.is_none() || !condition.value.is_finite() {
                    return Err(ExprError::new(format!(
                        "if at column {col} needs a finite plain number as its condition"
                    )));
                }
                self.node(&args[if condition.value != 0.0 { 1 } else { 2 }])
            }
            Node::Call { func, args, col } => {
                let args = args
                    .iter()
                    .map(|a| self.node(a))
                    .collect::<Result<Vec<Quantity>, ExprError>>()?;
                let q = self.call(*func, &args, *col)?;
                if !q.value.is_finite() {
                    return Err(ExprError::new(format!(
                        "the result of {} at column {col} is not a finite number",
                        func.name()
                    )));
                }
                Ok(q)
            }
        }
    }

    fn call(&self, func: Func, args: &[Quantity], col: usize) -> Result<Quantity, ExprError> {
        let units = &self.units;
        let name = func.name();
        let arg = args[0];
        let x = arg.value;
        let domain = |what: &str| {
            Err(ExprError::new(format!(
                "{name}({}) at column {col}: {what}",
                fmt_num(x)
            )))
        };
        let needs = |what: &str| {
            Err(ExprError::new(format!(
                "{name} at column {col} needs {what}, but got {}",
                arg.describe(units)
            )))
        };
        Ok(match func {
            Func::If => unreachable!("conditionals evaluate only the selected branch"),
            Func::Sin | Func::Cos | Func::Tan => {
                // A plain number is an angle in degrees.
                if !arg.dim.is_none() && arg.dim != Dim::ANGLE {
                    return needs("an angle");
                }
                Quantity::number(match func {
                    Func::Sin => sin_deg(x),
                    Func::Cos => cos_deg(x),
                    _ => {
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
                })
            }
            Func::Asin | Func::Acos | Func::Atan => {
                if !arg.dim.is_none() {
                    return needs("a plain number (a ratio)");
                }
                if func != Func::Atan && !(-1.0..=1.0).contains(&x) {
                    return domain("needs a value between -1 and 1");
                }
                let radians = match func {
                    Func::Asin => x.asin(),
                    Func::Acos => x.acos(),
                    _ => x.atan(),
                };
                Quantity {
                    value: radians.to_degrees(),
                    dim: self.dim(Dim::ANGLE),
                }
            }
            Func::Atan2 => {
                let Some((y, x, _)) = self.unify(args[0], args[1]) else {
                    return Err(ExprError::new(format!(
                        "atan2 at column {col} needs two values of the same kind, but got {} and {}",
                        args[0].describe(units),
                        args[1].describe(units)
                    )));
                };
                if y == 0.0 && x == 0.0 {
                    return Err(ExprError::new(format!(
                        "atan2(0, 0) at column {col} is undefined"
                    )));
                }
                Quantity {
                    value: y.atan2(x).to_degrees(),
                    dim: self.dim(Dim::ANGLE),
                }
            }
            Func::Sqrt => {
                if x < 0.0 {
                    return domain("square root of a negative number");
                }
                let Some(dim) = arg.dim.powf(0.5) else {
                    return needs("a plain number or an area");
                };
                Quantity {
                    value: x.sqrt(),
                    dim,
                }
            }
            Func::Abs => Quantity {
                value: x.abs(),
                dim: arg.dim,
            },
            Func::Round | Func::Floor | Func::Ceil | Func::Int => {
                // In document units: round(0.6in) is 1 in in an inch document.
                let scale = units.scale(arg.dim);
                let mut v = x / scale;
                // The division must not push a whole number just past itself.
                let nearest = v.round();
                if (v - nearest).abs() <= 1e-9 * nearest.abs().max(1.0) {
                    v = nearest;
                }
                let v = match func {
                    Func::Round => v.round(),
                    Func::Floor | Func::Int => v.floor(),
                    _ => v.ceil(),
                };
                Quantity {
                    value: v * scale,
                    dim: arg.dim,
                }
            }
            Func::Min | Func::Max => {
                // The first argument with a unit decides; plain numbers adopt it.
                let first = args.iter().find(|a| !a.dim.is_none()).unwrap_or(&arg);
                let mut best: Option<f64> = None;
                for a in args {
                    let like = Quantity {
                        value: 0.0,
                        dim: first.dim,
                    };
                    let Some((v, _, _)) = self.unify(*a, like) else {
                        return Err(ExprError::new(format!(
                            "can't compare {} and {} in {name}",
                            first.describe(units),
                            a.describe(units)
                        )));
                    };
                    best = Some(match best {
                        None => v,
                        Some(b) if func == Func::Min => b.min(v),
                        Some(b) => b.max(v),
                    });
                }
                Quantity {
                    value: best.expect("min and max take at least one argument"),
                    dim: first.dim,
                }
            }
        })
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
    Value(Quantity),
    /// Known but unusable; the message says why.
    Failed(String),
    Unknown,
}

/// Evaluates a set of named expressions that may refer to each other, in dependency order.
/// Names that aren't nodes go to `external`. `check` validates each node's result and turns
/// it into the node's value (so that dependents of an invalid value fail too, and see a
/// length where the node is a length).
struct Graph<'a> {
    names: &'a [&'a str],
    exprs: &'a [Result<Expr, ExprError>],
    external: &'a dyn Fn(&str) -> External,
    candidates: &'a dyn Fn() -> Vec<String>,
    check: &'a dyn Fn(usize, Quantity) -> Result<Quantity, ExprError>,
    units: Units,
    state: Vec<u8>, // 0 = unvisited, 1 = on the stack, 2 = done
    stack: Vec<usize>,
    cycle: Vec<Option<ExprError>>,
    results: Vec<Option<Result<Quantity, ExprError>>>,
}

impl<'a> Graph<'a> {
    fn run(
        names: &'a [&'a str],
        exprs: &'a [Result<Expr, ExprError>],
        external: &'a dyn Fn(&str) -> External,
        candidates: &'a dyn Fn() -> Vec<String>,
        check: &'a dyn Fn(usize, Quantity) -> Result<Quantity, ExprError>,
        units: Units,
    ) -> Vec<Result<Quantity, ExprError>> {
        let n = names.len();
        let mut g = Graph {
            names,
            exprs,
            external,
            candidates,
            check,
            units,
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

    fn evaluate(&self, i: usize) -> Result<Quantity, ExprError> {
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
        let value = expr.eval_quantity(&self.units, &|name| match self.node(name) {
            Some(j) => self.results[j]
                .as_ref()
                .and_then(|r| r.as_ref().ok())
                .copied(),
            None => match (self.external)(name) {
                External::Value(v) => Some(v),
                _ => None,
            },
        })?;
        (self.check)(i, value)
    }
}

// ---- Parameters ----

/// A named, user-defined parameter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub expression: String,
    /// Last evaluated value in base units: mm for a length, degrees for an angle, or the
    /// plain number (NaN if evaluation failed).
    pub value: f64,
    /// What the value is. `width = 100` is a number, which adopts a unit where it is used;
    /// `width = 100mm` is a length.
    #[serde(default)]
    pub kind: QuantityKind,
}

impl Parameter {
    /// A parameter that has not been evaluated yet (see [`Parameters::evaluate`]).
    pub fn new(name: impl Into<String>, expression: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            expression: expression.into(),
            value: f64::NAN,
            kind: QuantityKind::Number,
        }
    }

    /// The value with its dimension, or `None` if evaluation failed.
    pub fn quantity(&self) -> Option<Quantity> {
        self.value
            .is_finite()
            .then(|| Quantity::new(self.value, self.kind))
    }

    /// The value for display in document units: "100", "101.6 mm" / "4 in", "45°", or
    /// "error" if evaluation failed.
    pub fn display(&self, units: &Units) -> String {
        match self.quantity() {
            Some(q) => q.display(units),
            None => "error".to_owned(),
        }
    }
}

/// The document's table of named parameters, with the document units. Parameters may refer
/// to each other; cycles are errors.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Parameters {
    pub entries: Vec<Parameter>,
    /// The document units: what plain numbers mean where a length is needed, and how
    /// values are displayed. Change them with [`Self::set_units`].
    #[serde(default)]
    pub units: Units,
}

impl Parameters {
    /// The parameter's value in base units (mm, degrees or the plain number), or `None` if
    /// it doesn't exist or failed to evaluate.
    pub fn get(&self, name: &str) -> Option<f64> {
        self.quantity(name).map(|q| q.value)
    }

    /// The parameter's value with its dimension, or `None` if it doesn't exist or failed
    /// to evaluate.
    pub fn quantity(&self, name: &str) -> Option<Quantity> {
        self.entries
            .iter()
            .find(|p| p.name == name)
            .and_then(Parameter::quantity)
    }

    /// Adds or replaces a parameter and re-evaluates the table. Returns the value in base
    /// units. The table is left unchanged if the new expression fails, or if it would
    /// break another parameter that evaluated fine before.
    pub fn set(&mut self, name: &str, expression: &str) -> Result<f64, ExprError> {
        check_name(name)?;
        Expr::parse(expression)?;
        let before = self.clone();
        let expression = expression.trim().to_owned();
        match self.entries.iter_mut().find(|p| p.name == name) {
            Some(p) => p.expression = expression,
            None => self.entries.push(Parameter::new(name, expression)),
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

    /// Changes the document units and re-evaluates. Returns the errors, per name.
    ///
    /// Values with explicit units keep their size; plain numbers that adopt the document
    /// unit (`5mm + 3`) change. Sketch dimensions with expressions need
    /// [`apply_expressions`] afterwards.
    pub fn set_units(&mut self, units: Units) -> Vec<(String, ExprError)> {
        self.units = units;
        self.evaluate()
    }

    /// Re-evaluates all parameters in dependency order. Returns the errors, per name.
    pub fn evaluate(&mut self) -> Vec<(String, ExprError)> {
        let units = self.units;
        let names: Vec<&str> = self.entries.iter().map(|p| p.name.as_str()).collect();
        let exprs: Vec<Result<Expr, ExprError>> = self
            .entries
            .iter()
            .map(|p| Expr::parse(&p.expression))
            .collect();
        let check = |_: usize, q: Quantity| match q.kind() {
            Some(_) => Ok(q),
            None => Err(ExprError::new(format!(
                "a parameter must be a number, a length or an angle, but the expression gives {}",
                q.describe(&units)
            ))),
        };
        let results = Graph::run(
            &names,
            &exprs,
            &|_| External::Unknown,
            &|| vec!["pi".to_owned()],
            &check,
            units,
        );
        let mut errors = Vec::new();
        for (p, r) in self.entries.iter_mut().zip(results) {
            match r {
                Ok(q) => {
                    p.value = q.value;
                    p.kind = q.kind().expect("checked above");
                }
                Err(e) => {
                    p.value = f64::NAN;
                    p.kind = QuantityKind::Number;
                    errors.push((p.name.clone(), e));
                }
            }
        }
        errors
    }

    /// Evaluates an expression against the table (for inputs that aren't sketch
    /// dimensions, such as an extrusion depth).
    pub fn evaluate_expression(&self, source: &str) -> Result<Quantity, ExprError> {
        let expr = Expr::parse(source)?;
        for (name, col) in expr.refs() {
            match self.entries.iter().find(|p| p.name == name) {
                Some(p) if p.quantity().is_some() => {}
                Some(_) => {
                    return Err(ExprError::new(format!(
                        "'{name}' at column {col} is a parameter with an error"
                    )));
                }
                None => {
                    let candidates: Vec<&str> = self
                        .entries
                        .iter()
                        .map(|p| p.name.as_str())
                        .chain(std::iter::once("pi"))
                        .collect();
                    return Err(unknown_name(name, col, &candidates));
                }
            }
        }
        expr.eval_quantity(&self.units, &|name| self.quantity(name))
    }

    /// [`Self::evaluate_expression`] where a `kind` is expected: the value in base units
    /// (mm, degrees or the plain number). A plain number is taken in document units.
    pub fn evaluate_as(&self, source: &str, kind: QuantityKind) -> Result<f64, ExprError> {
        self.evaluate_expression(source)?.expect(kind, &self.units)
    }
}

// ---- Dimensions ----

/// What a dimension measures: an angle or a length.
fn dimension_kind(sketch: &Sketch, id: ConstraintId) -> QuantityKind {
    match sketch.constraint(id) {
        Some(c) if c.kind.is_angular() => QuantityKind::Angle,
        _ => QuantityKind::Length,
    }
}

/// Checks that a value suits the dimension: lengths must be positive, angles in (0, 180].
fn check_dimension_value(
    sketch: &Sketch,
    id: ConstraintId,
    value: f64,
    units: &Units,
) -> Result<(), ExprError> {
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
            "{label} must be greater than zero, got {}",
            units.format_length(value)
        )));
    }
    Ok(())
}

/// Takes an evaluated input as the dimension's value (mm or degrees): a plain number is in
/// document units, a quantity must be of the dimension's kind and in its valid range.
fn dimension_value(
    sketch: &Sketch,
    id: ConstraintId,
    q: Quantity,
    units: &Units,
) -> Result<Quantity, ExprError> {
    let kind = dimension_kind(sketch, id);
    let value = q.expect_for("this dimension", kind, units)?;
    check_dimension_value(sketch, id, value, units)?;
    Ok(Quantity::new(value, kind))
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
/// the node ids and their results in mm or degrees.
fn evaluate_dimensions(
    sketch: &Sketch,
    params: &Parameters,
    overrides: Option<(ConstraintId, &str)>,
) -> Vec<(ConstraintId, Result<f64, ExprError>)> {
    let units = params.units;
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
            return External::Value(Quantity::new(value, dimension_kind(sketch, id)));
        }
        match params.entries.iter().find(|p| p.name == name) {
            Some(p) => match p.quantity() {
                Some(q) => External::Value(q),
                None => External::Failed("is a parameter with an error".to_owned()),
            },
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
    let check = |i: usize, q: Quantity| dimension_value(sketch, ids[i], q, &units);
    let results = Graph::run(&names, &exprs, &external, &candidates, &check, units);
    ids.iter()
        .copied()
        .zip(results.into_iter().map(|r| r.map(|q| q.value)))
        .collect()
}

/// Sets a dimension from user input and returns the new value (mm or degrees). The
/// dimension is left unchanged on error.
///
/// A number, with or without a unit (`12`, `2in`, `45°`), is stored as a plain value and
/// clears the expression; a number without a unit is in the document units of `params`.
/// Anything else is stored as an expression, evaluated against `params` and the sketch's
/// dimension names. A length dimension needs a length and an angle dimension an angle (or
/// a plain number, see the module docs).
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
    let (value, expression) = if is_literal(input) {
        let q = Expr::parse(input)?.eval_quantity(&params.units, &|_| None)?;
        let q = dimension_value(sketch, dimension, q, &params.units)?;
        (q.value, None)
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
/// may refer to each other), in the document units of `params`. Returns the failures;
/// failed dimensions keep their value.
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

    const MM: Units = Units {
        length: LengthUnit::Mm,
    };
    const INCH: Units = Units {
        length: LengthUnit::Inch,
    };
    const AREA: Dim = Dim::new(2, 0);

    /// Unit-less evaluation ([`Expr::eval`]).
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

    /// Unit-aware evaluation: `width` is a plain 10, `len` 20 mm, `ang` 30°.
    fn try_q(units: Units, src: &str) -> Result<Quantity, ExprError> {
        Expr::parse(src)?.eval_quantity(&units, &|n| match n {
            "width" => Some(Quantity::number(10.0)),
            "len" => Some(Quantity::length(20.0)),
            "ang" => Some(Quantity::angle(30.0)),
            _ => None,
        })
    }

    fn q_in(units: Units, src: &str) -> Quantity {
        try_q(units, src).unwrap_or_else(|e| panic!("{src}: {e}"))
    }

    fn q(src: &str) -> Quantity {
        q_in(MM, src)
    }

    fn qerr_in(units: Units, src: &str) -> String {
        try_q(units, src).expect_err(src).message
    }

    fn qerr(src: &str) -> String {
        qerr_in(MM, src)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[track_caller]
    fn assert_q(got: Quantity, value: f64, dim: Dim) {
        assert!(
            close(got.value, value) && got.dim == dim,
            "got {got:?}, expected {value} {dim:?}"
        );
    }

    #[test]
    fn comparisons_and_conditionals() {
        for (src, expected) in [
            ("1 + 2 * 3 > 6", 1.0),
            ("1 < 1", 0.0),
            ("1 <= 1", 1.0),
            ("2 >= 3", 0.0),
            ("2 == 2", 1.0),
            ("2 = 3", 0.0),
            ("2 != 3", 1.0),
            ("2 <> 2", 0.0),
            ("if(width > 9, 3, 2)", 3.0),
            ("iif(0, 1 / 0, if(-2, 7, sqrt(-1)))", 7.0),
            ("int(-2.3)", -3.0),
            ("int(2.9)", 2.0),
        ] {
            assert_eq!(ev(src), expected, "{src}");
        }
        assert_q(q("if(len > 19mm, 2in, 1 / 0)"), 50.8, Dim::LENGTH);
        assert_q(q("if(0, 30deg, 3mm)"), 3.0, Dim::LENGTH);
        assert_q(q("1in == 25.4mm"), 1.0, Dim::NONE);
        assert_eq!(q_in(INCH, "20mm < 1").value, 1.0);
        assert_eq!(q_in(MM, "20mm < 1").value, 0.0);
        assert!(qerr("1mm < 2deg").contains("can't compare"));
        assert!(qerr("if(1mm, 2, 3)").contains("plain number"));
        assert!(err("if(1, 2)").contains("3 arguments"));
        assert!(err("1 < 2 < 3").contains("chained comparisons"));
        assert!(qerr("1e309 > 0").contains("finite values"));
        assert!(err("if(1, 1 / 0, 2)").contains("division by zero"));
        for name in ["if", "iif", "int"] {
            assert!(check_name(name).is_err());
        }
    }

    #[test]
    fn conditional_parameters_follow_dependencies() {
        let mut p = Parameters::default();
        p.set("width", "299mm").unwrap();
        p.set("copies", "iif(width > 299, 3, 2)").unwrap();
        assert_eq!(p.get("copies"), Some(2.0));
        p.set("width", "300mm").unwrap();
        assert_eq!(p.get("copies"), Some(3.0));
        p.set("safe", "if(width > 0, 5mm, 1 / 0)").unwrap();
        assert_eq!(p.get("safe"), Some(5.0));
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
    fn unitless_eval_scales_units() {
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
        assert_eq!(ev("2\""), 50.8);
        // Nothing is checked: everything is a number in mm or degrees.
        assert!(close(ev("1in + 30deg"), 55.4));
        assert!(close(ev("asin(0.5) + 1mm"), 31.0));
        assert!(close(ev("sqrt(2mm)"), 2f64.sqrt()));
        assert_eq!(ev("width * 1cm"), 100.0);
        // A power after a unit belongs to the unit.
        assert!(close(ev("2in^2"), 2.0 * 25.4 * 25.4));
        assert!(close(ev("(2in)^2"), 50.8 * 50.8));
    }

    #[test]
    fn quantities_carry_dimensions() {
        assert_q(q("5"), 5.0, Dim::NONE);
        assert_q(q("pi"), std::f64::consts::PI, Dim::NONE);
        assert_q(q("5mm"), 5.0, Dim::LENGTH);
        assert_q(q("1in"), 25.4, Dim::LENGTH);
        assert_q(q("2\""), 50.8, Dim::LENGTH);
        assert_q(q("2 cm"), 20.0, Dim::LENGTH);
        assert_q(q("1.5m"), 1500.0, Dim::LENGTH);
        assert_q(q("1ft + 1in"), 330.2, Dim::LENGTH);
        assert_q(q("(1 + 1)in"), 50.8, Dim::LENGTH);
        assert_q(q("-2in"), -50.8, Dim::LENGTH);
        assert_q(q("90deg"), 90.0, Dim::ANGLE);
        assert_q(q("45°"), 45.0, Dim::ANGLE);
        assert_q(q("pi rad"), 180.0, Dim::ANGLE);
        // Names carry their dimension.
        assert_q(q("2 * len"), 40.0, Dim::LENGTH);
        assert_q(q("len - 5mm"), 15.0, Dim::LENGTH);
        assert_q(q("ang / 2"), 15.0, Dim::ANGLE);
        assert_q(q("width * 1cm"), 100.0, Dim::LENGTH);
        // Whatever the document units, a quantity is in mm.
        assert_q(q_in(INCH, "5mm + 1in"), 30.4, Dim::LENGTH);
        assert_q(q_in(INCH, "len / 2"), 10.0, Dim::LENGTH);
    }

    #[test]
    fn areas_and_ratios() {
        assert_q(q("5mm * 2mm"), 10.0, AREA);
        assert_q(q("10mm / 2mm"), 5.0, Dim::NONE);
        assert_q(q("1in / 1mm"), 25.4, Dim::NONE);
        assert_q(q("len / 4mm"), 5.0, Dim::NONE);
        assert_q(q("len * len / len"), 20.0, Dim::LENGTH);
        assert_q(q("len^2"), 400.0, AREA);
        assert_q(q("len^2 / 1cm"), 40.0, Dim::LENGTH);
        assert_q(q("1 / 2mm"), 0.5, Dim::new(-1, 0));
        assert_q(
            q("len * ang / 1rad"),
            20.0 * 30f64.to_radians(),
            Dim::LENGTH,
        );
        // A power after a unit belongs to the unit.
        assert_q(q("4mm^2"), 4.0, AREA);
        assert_q(q("2in^2"), 2.0 * 25.4 * 25.4, AREA);
        assert_q(q("(2in)^2"), 50.8 * 50.8, AREA);
        assert_q(q("1cm^3"), 1000.0, Dim::new(3, 0));
        assert_q(q("2mm^-1"), 2.0, Dim::new(-1, 0));
        assert_q(q("-4mm^2"), -4.0, AREA);
        assert_q(q("4mm^2^2"), 16.0, Dim::new(4, 0));
        // …and a unit belongs to the number before it.
        assert_eq!(
            qerr("2^2mm"),
            "the exponent at column 2 must be a plain number, but it is a length (2 mm)"
        );
    }

    #[test]
    fn sqrt_and_powers_of_quantities() {
        assert_q(q("sqrt(4mm^2)"), 2.0, Dim::LENGTH);
        assert_q(q("sqrt(len * 5mm)"), 10.0, Dim::LENGTH);
        assert_q(q("sqrt(3mm * 3mm + 4mm * 4mm)"), 5.0, Dim::LENGTH);
        assert_q(q("sqrt(16)"), 4.0, Dim::NONE);
        assert_q(q("(4mm^2)^0.5"), 2.0, Dim::LENGTH);
        assert_q(q("len^0"), 1.0, Dim::NONE);
        assert_q(q("len^-1"), 0.05, Dim::new(-1, 0));
        assert_eq!(
            qerr("sqrt(2mm)"),
            "sqrt at column 1 needs a plain number or an area, but got a length (2 mm)"
        );
        assert_eq!(
            qerr("(2mm)^0.5"),
            "can't raise a length (2 mm) to the power 0.5 at column 6: the result has no whole unit"
        );
        assert_eq!(
            qerr("2^(1mm)"),
            "the exponent at column 2 must be a plain number, but it is a length (1 mm)"
        );
        assert_eq!(
            qerr("1mm^9 * len"),
            "the units at column 7 are raised to too high a power"
        );
        assert_eq!(
            qerr("len^10"),
            "the units at column 4 are raised to too high a power"
        );
    }

    #[test]
    fn mixing_dimensions_is_an_error() {
        assert_eq!(
            qerr("10mm + 30deg"),
            "can't add a length (10 mm) and an angle (30°)"
        );
        assert_eq!(
            qerr("30deg - 10mm"),
            "can't subtract a length (10 mm) from an angle (30°)"
        );
        assert_eq!(
            qerr("len + ang"),
            "can't add a length (20 mm) and an angle (30°)"
        );
        assert_eq!(
            qerr("5mm + 2mm^2"),
            "can't add a length (5 mm) and an area (2 mm²)"
        );
        assert_eq!(
            qerr("len - 1 / 2mm"),
            "can't subtract a compound quantity (0.5 mm⁻¹) from a length (20 mm)"
        );
        // Values are shown in document units.
        assert_eq!(
            qerr_in(INCH, "1in + 30deg"),
            "can't add a length (1 in) and an angle (30°)"
        );
        assert_eq!(
            qerr_in(INCH, "2in^2 + ang"),
            "can't add an area (2 in²) and an angle (30°)"
        );
        assert_eq!(
            qerr("min(1mm, 2, 30deg)"),
            "can't compare a length (1 mm) and an angle (30°) in min"
        );
        assert_eq!(
            qerr("max(1, ang, len)"),
            "can't compare an angle (30°) and a length (20 mm) in max"
        );
        assert_eq!(
            qerr("(5mm) in"),
            "'in' at column 7: the value already has a unit, it is a length (5 mm)"
        );
        assert_eq!(
            qerr("atan2(1mm, 1deg)"),
            "atan2 at column 1 needs two values of the same kind, but got a length (1 mm) and an angle (1°)"
        );
    }

    #[test]
    fn plain_numbers_adopt_the_other_unit() {
        // Lengths: the document unit.
        assert_q(q("5mm + 3"), 8.0, Dim::LENGTH);
        assert_q(q("3 + 5mm"), 8.0, Dim::LENGTH);
        assert_q(q("5mm - 3"), 2.0, Dim::LENGTH);
        assert_q(q_in(INCH, "5mm + 3"), 81.2, Dim::LENGTH);
        assert_q(q_in(INCH, "3 + 5mm"), 81.2, Dim::LENGTH);
        assert_q(q_in(INCH, "3 - 5mm"), 71.2, Dim::LENGTH);
        assert_q(q_in(INCH, "width + 5mm"), 259.0, Dim::LENGTH);
        assert_q(
            q_in(Units::new(LengthUnit::Cm), "5mm + 3"),
            35.0,
            Dim::LENGTH,
        );
        // A ratio is a plain number too.
        assert_q(q_in(INCH, "10mm / 2mm + 5mm"), 132.0, Dim::LENGTH);
        // Areas: the document unit squared.
        assert_q(q_in(INCH, "5mm^2 + 1"), 5.0 + 25.4 * 25.4, AREA);
        // Angles: degrees, in every document.
        assert_q(q("30deg + 15"), 45.0, Dim::ANGLE);
        assert_q(q_in(INCH, "30deg + 15"), 45.0, Dim::ANGLE);
        assert_q(q_in(INCH, "100 - ang"), 70.0, Dim::ANGLE);
        // Plain numbers together stay plain; products don't adopt anything.
        assert_q(q_in(INCH, "2 + 3"), 5.0, Dim::NONE);
        assert_q(q_in(INCH, "2 * 5mm"), 10.0, Dim::LENGTH);
        assert_q(q_in(INCH, "5mm / 2"), 2.5, Dim::LENGTH);
        // min and max compare like + does.
        assert_q(q("min(3, 5mm)"), 3.0, Dim::LENGTH);
        assert_q(q_in(INCH, "min(3, 5mm)"), 5.0, Dim::LENGTH);
        assert_q(q_in(INCH, "max(5mm, 3, 1ft)"), 304.8, Dim::LENGTH);
        assert_q(q_in(INCH, "max(1, 2, 20mm)"), 50.8, Dim::LENGTH);
        assert_q(q("max(20, ang)"), 30.0, Dim::ANGLE);
        assert_q(q_in(INCH, "min(3, 1, 2)"), 1.0, Dim::NONE);
    }

    #[test]
    fn expected_kinds() {
        use QuantityKind::{Angle, Length, Number};
        // A plain number is taken in document units.
        assert_eq!(q("12").expect(Length, &MM), Ok(12.0));
        assert_eq!(q("2").expect(Length, &INCH), Ok(50.8));
        assert_eq!(q("2").expect(Angle, &INCH), Ok(2.0));
        assert_eq!(q("2").expect(Number, &INCH), Ok(2.0));
        // A quantity of the right kind is taken as it is.
        assert_eq!(q("2in").expect(Length, &MM), Ok(50.8));
        assert_eq!(q("12mm").expect(Length, &INCH), Ok(12.0));
        assert_eq!(q("45deg").expect(Angle, &INCH), Ok(45.0));
        assert_eq!(q("len / 5mm").expect(Number, &MM), Ok(4.0));

        let e = |src: &str, kind, units: &Units| q(src).expect(kind, units).unwrap_err().message;
        assert_eq!(
            e("30deg", Length, &MM),
            "this value needs a length, but the expression gives an angle (30°)"
        );
        assert_eq!(
            e("5mm * 2mm", Length, &MM),
            "this value needs a length, but the expression gives an area (10 mm²): divide by a length"
        );
        assert_eq!(
            e("1in^3", Length, &INCH),
            "this value needs a length, but the expression gives a volume (1 in³): divide by an area"
        );
        assert_eq!(
            e("1 / 2mm", Length, &MM),
            "this value needs a length, but the expression gives a compound quantity (0.5 mm⁻¹): multiply by an area"
        );
        assert_eq!(
            e("len * ang", Length, &MM),
            "this value needs a length, but the expression gives a compound quantity (600 mm·deg): divide by an angle"
        );
        assert_eq!(
            e("10mm", Angle, &MM),
            "this value needs an angle, but the expression gives a length (10 mm)"
        );
        assert_eq!(
            e("10mm", Number, &MM),
            "this value needs a plain number, but the expression gives a length (10 mm): divide by a length"
        );
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
    fn trigonometry_with_and_without_units() {
        // A plain number is degrees; the result is a plain number.
        assert_q(q("sin(30)"), 0.5, Dim::NONE);
        assert_q(q("sin(30deg)"), 0.5, Dim::NONE);
        assert_q(q("sin(30°)"), 0.5, Dim::NONE);
        assert_q(q("sin(pi rad / 6)"), 0.5, Dim::NONE);
        assert_q(q("sin(ang)"), 0.5, Dim::NONE);
        assert_q(q_in(INCH, "sin(30)"), 0.5, Dim::NONE);
        assert_q(q("cos(60deg)"), 0.5, Dim::NONE);
        assert_q(q("cos(90deg)"), 0.0, Dim::NONE);
        assert_q(q("tan(45deg)"), 1.0, Dim::NONE);
        assert_q(q("tan(45)"), 1.0, Dim::NONE);
        assert_q(q("len * sin(ang)"), 10.0, Dim::LENGTH);
        // Inverse functions give angles.
        assert_q(q("asin(0.5)"), 30.0, Dim::ANGLE);
        assert_q(q("acos(0)"), 90.0, Dim::ANGLE);
        assert_q(q("atan(1)"), 45.0, Dim::ANGLE);
        assert_q(q("atan2(1, 1)"), 45.0, Dim::ANGLE);
        assert_q(q("atan2(1, -1)"), 135.0, Dim::ANGLE);
        assert_q(q("atan2(5mm, 5mm)"), 45.0, Dim::ANGLE);
        assert_q(q("asin(5mm / 1cm)"), 30.0, Dim::ANGLE);
        assert_q(q("atan(1) + 15"), 60.0, Dim::ANGLE);
        assert_q(q("sin(asin(0.5))"), 0.5, Dim::NONE);
        assert_q(q("asin(0.5) / 1deg"), 30.0, Dim::NONE);
        // atan2 unifies its arguments like + does.
        assert_q(q_in(INCH, "atan2(1, 1in)"), 45.0, Dim::ANGLE);
        assert_q(
            q("atan2(1, 1in)"),
            (1f64).atan2(25.4).to_degrees(),
            Dim::ANGLE,
        );

        assert_eq!(
            qerr("sin(5mm)"),
            "sin at column 1 needs an angle, but got a length (5 mm)"
        );
        assert_eq!(
            qerr("2 * cos(len)"),
            "cos at column 5 needs an angle, but got a length (20 mm)"
        );
        assert_eq!(
            qerr("asin(5mm)"),
            "asin at column 1 needs a plain number (a ratio), but got a length (5 mm)"
        );
        assert_eq!(
            qerr("atan(ang)"),
            "atan at column 1 needs a plain number (a ratio), but got an angle (30°)"
        );
        assert!(qerr("tan(90deg)").contains("undefined"));
        assert!(qerr("asin(2)").contains("between -1 and 1"));
        assert!(qerr("sqrt(-4mm^2)").contains("square root of a negative number"));
    }

    #[test]
    fn rounding_keeps_the_dimension_and_works_in_document_units() {
        assert_q(q("abs(-3mm)"), 3.0, Dim::LENGTH);
        assert_q(q("abs(-ang)"), 30.0, Dim::ANGLE);
        assert_q(q("round(2.5)"), 3.0, Dim::NONE);
        assert_q(q("round(12.4mm)"), 12.0, Dim::LENGTH);
        assert_q(q("round(0.6in)"), 15.0, Dim::LENGTH);
        assert_q(q_in(INCH, "round(0.6in)"), 25.4, Dim::LENGTH);
        assert_q(q_in(INCH, "floor(60mm)"), 50.8, Dim::LENGTH);
        assert_q(q_in(INCH, "ceil(60mm)"), 76.2, Dim::LENGTH);
        assert_q(q_in(INCH, "round(22.4deg)"), 22.0, Dim::ANGLE);
        // Whole numbers of document units survive the conversion from mm.
        for n in 1..200 {
            let n = f64::from(n);
            for f in ["floor", "ceil", "round"] {
                assert_q(q_in(INCH, &format!("{f}({n}in)")), n * 25.4, Dim::LENGTH);
                assert_q(
                    q_in(Units::new(LengthUnit::Ft), &format!("{f}({n}ft)")),
                    n * 304.8,
                    Dim::LENGTH,
                );
            }
        }
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
        // Units follow a value, and only one of them.
        assert_eq!(err("\"5"), "unexpected '\"' at column 1");
        assert_eq!(err("5mm\""), "unexpected '\"' at column 4");
        assert!(err("5 mm in").starts_with("unexpected name 'in' at column 6"));
        assert_eq!(
            err("2mm^"),
            "unexpected end of expression, expected a number, a name or '('"
        );
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
        assert_eq!(qerr("len / (2mm - 2mm)"), "division by zero at column 5");
        assert_eq!(qerr("nope * 2mm"), "unknown name 'nope' at column 1");
    }

    #[test]
    fn names_are_unique_in_order() {
        let e = Expr::parse("b + a * b + sin(c) + pi + (d)mm").unwrap();
        assert_eq!(e.names(), vec!["b", "a", "c", "d"]);
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
        // Every document unit is a unit of the expression language, with the same factor.
        for u in LengthUnit::ALL {
            assert!(check_name(u.suffix()).unwrap_err().message.contains("unit"));
            assert_q(q(&format!("1{}", u.suffix())), u.mm_per_unit(), Dim::LENGTH);
        }
    }

    #[test]
    fn literals() {
        for src in [
            " 12.5 ", "-3", "+3", "12mm", "2 in", "-2in", "45°", "90deg", "5\"", "1e3m",
        ] {
            assert!(is_literal(src), "{src}");
        }
        for src in [
            "1+1", "inf", "NaN", "pi", "pi rad", "2mm^2", "(2)", "2 x", "--3", "mm", "",
        ] {
            assert!(!is_literal(src), "{src}");
        }
    }

    #[test]
    fn length_units() {
        assert_eq!(Units::default(), MM);
        assert_eq!(LengthUnit::ALL.len(), 5);
        assert_eq!(LengthUnit::Inch.suffix(), "in");
        assert_eq!(LengthUnit::Mm.suffix(), "mm");
        assert_eq!(LengthUnit::Inch.label(), "Inches");
        assert_eq!(LengthUnit::Ft.mm_per_unit(), 304.8);
        assert_eq!(INCH.to_mm(2.0), 50.8);
        assert_eq!(INCH.from_mm(50.8), 2.0);
        assert_eq!(MM.to_mm(2.0), 2.0);
        for u in LengthUnit::ALL {
            let units = Units::new(u);
            assert!(close(units.from_mm(units.to_mm(1.25)), 1.25));
        }
    }

    #[test]
    fn length_formatting() {
        assert_eq!(MM.format_length(12.5), "12.5 mm");
        assert_eq!(MM.format_length(12.0), "12 mm");
        assert_eq!(MM.format_length(1234.5), "1234.5 mm");
        assert_eq!(MM.format_length(100.0), "100 mm");
        assert_eq!(MM.format_length(1.0 / 3.0), "0.333 mm");
        assert_eq!(MM.format_length(2.0 / 3.0), "0.667 mm");
        assert_eq!(MM.format_length(0.1 + 0.2), "0.3 mm");
        assert_eq!(MM.format_length(-2.5), "-2.5 mm");
        // No "-0".
        assert_eq!(MM.format_length(-0.0), "0 mm");
        assert_eq!(MM.format_length(-0.0001), "0 mm");
        assert_eq!(MM.format_length(0.0), "0 mm");

        assert_eq!(INCH.format_length(19.05), "0.75 in");
        assert_eq!(INCH.format_length(25.4), "1 in");
        assert_eq!(INCH.format_length(101.6), "4 in");
        assert_eq!(INCH.format_length(3.0 * 25.4), "3 in");
        assert_eq!(INCH.format_length(1.0), "0.0394 in");
        assert_eq!(INCH.format_length(25.4 / 3.0), "0.3333 in");
        assert_eq!(INCH.format_length(-0.001), "0 in");
        assert_eq!(INCH.format_length(254.0), "10 in");

        assert_eq!(Units::new(LengthUnit::Cm).format_length(125.0), "12.5 cm");
        assert_eq!(Units::new(LengthUnit::M).format_length(1500.0), "1.5 m");
        assert_eq!(Units::new(LengthUnit::M).format_length(1.0), "0.001 m");
        assert_eq!(Units::new(LengthUnit::Ft).format_length(304.8), "1 ft");
        assert_eq!(Units::new(LengthUnit::Ft).format_length(152.4), "0.5 ft");

        assert_eq!(MM.format_length_value(12.5), "12.5");
        assert_eq!(INCH.format_length_value(12.7), "0.5");
        assert_eq!(INCH.format_length_value(-0.0), "0");

        assert_eq!(MM.format_angle(45.0), "45°");
        assert_eq!(MM.format_angle(22.5), "22.5°");
        assert_eq!(MM.format_angle(-0.0), "0°");
        assert_eq!(MM.format_angle(100.0 / 3.0), "33.333°");
    }

    #[test]
    fn quantity_display() {
        assert_eq!(Quantity::number(100.0).display(&INCH), "100");
        assert_eq!(Quantity::number(0.1 + 0.2).display(&MM), "0.3");
        assert_eq!(Quantity::length(101.6).display(&MM), "101.6 mm");
        assert_eq!(Quantity::length(101.6).display(&INCH), "4 in");
        assert_eq!(Quantity::angle(45.0).display(&INCH), "45°");
        assert_eq!(q("5mm * 2mm").display(&MM), "10 mm²");
        assert_eq!(q("1in * 2in").display(&INCH), "2 in²");
        assert_eq!(q("1cm^3").display(&MM), "1000 mm³");
        assert_eq!(q("2mm^-1").display(&MM), "2 mm⁻¹");
        assert_eq!(q("ang * ang").display(&MM), "900 deg²");
        assert_eq!(Quantity::length(5.0).kind(), Some(QuantityKind::Length));
        assert_eq!(Quantity::angle(5.0).kind(), Some(QuantityKind::Angle));
        assert_eq!(Quantity::number(5.0).kind(), Some(QuantityKind::Number));
        assert_eq!(q("5mm * 2mm").kind(), None);
    }

    #[test]
    fn parameters_evaluate_in_dependency_order() {
        let mut p = Parameters::default();
        // Declared before the parameters it uses.
        p.entries.push(Parameter::new("area", "width * height"));
        p.entries.push(Parameter::new("width", "2 * height"));
        p.entries.push(Parameter::new("height", "5"));
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
    fn parameter_kinds() {
        use QuantityKind::{Angle, Length, Number};
        let mut p = Parameters::default();
        let entry = |p: &Parameters, name: &str| {
            p.entries
                .iter()
                .find(|e| e.name == name)
                .unwrap_or_else(|| panic!("{name}"))
                .clone()
        };
        assert_eq!(p.set("n", "100").unwrap(), 100.0);
        assert_eq!(entry(&p, "n").kind, Number);
        assert_eq!(entry(&p, "n").display(&MM), "100");
        assert_eq!(entry(&p, "n").display(&INCH), "100");

        assert!(close(p.set("w", "4in").unwrap(), 101.6));
        assert_eq!(entry(&p, "w").kind, Length);
        assert_eq!(entry(&p, "w").display(&MM), "101.6 mm");
        assert_eq!(entry(&p, "w").display(&INCH), "4 in");
        assert_q(p.quantity("w").unwrap(), 101.6, Dim::LENGTH);

        assert_eq!(p.set("len", "100mm").unwrap(), 100.0);
        assert_eq!(entry(&p, "len").kind, Length);

        assert_eq!(p.set("a", "45deg").unwrap(), 45.0);
        assert_eq!(entry(&p, "a").kind, Angle);
        assert_eq!(entry(&p, "a").display(&INCH), "45°");
        assert_eq!(p.quantity("a"), Some(Quantity::angle(45.0)));

        // Kinds follow from the expression.
        assert!(close(p.set("half", "w / 2").unwrap(), 50.8));
        assert_eq!(entry(&p, "half").kind, Length);
        assert!(close(p.set("ratio", "w / 1in").unwrap(), 4.0));
        assert_eq!(entry(&p, "ratio").kind, Number);
        assert!(close(p.set("t", "atan2(1, 1)").unwrap(), 45.0));
        assert_eq!(entry(&p, "t").kind, Angle);
        // A number adopts the unit where it is used.
        assert!(close(p.set("sum", "n + w").unwrap(), 201.6));
        assert_eq!(entry(&p, "sum").kind, Length);
        // Changing the expression changes the kind.
        assert_eq!(p.set("len", "7").unwrap(), 7.0);
        assert_eq!(entry(&p, "len").kind, Number);

        let count = p.entries.len();
        let e = p.set("area", "w * len * 1mm").unwrap_err();
        assert_eq!(
            e.message,
            "a parameter must be a number, a length or an angle, but the expression gives an area (711.2 mm²)"
        );
        let e = p.set("bad", "w + a").unwrap_err();
        assert_eq!(
            e.message,
            "can't add a length (101.6 mm) and an angle (45°)"
        );
        assert_eq!(p.entries.len(), count);
        assert_eq!(p.quantity("area"), None);

        // A failed parameter loaded from a file.
        p.entries.push(Parameter::new("broken", "1 / 0"));
        assert_eq!(p.evaluate().len(), 1);
        assert_eq!(entry(&p, "broken").display(&MM), "error");
        assert_eq!(entry(&p, "broken").quantity(), None);
        assert_eq!(p.get("broken"), None);
    }

    #[test]
    fn parameters_in_an_inch_document() {
        let mut p = Parameters {
            units: INCH,
            ..Parameters::default()
        };
        assert_eq!(p.set("n", "2").unwrap(), 2.0, "a number stays a number");
        assert!(close(p.set("a", "5mm + 3").unwrap(), 81.2));
        assert!(close(p.set("b", "n + 1in").unwrap(), 76.2));
        let e = p.set("area", "1in * 2in").unwrap_err();
        assert!(e.message.ends_with("an area (2 in²)"), "{}", e.message);
    }

    #[test]
    fn set_units_reevaluates() {
        let mut p = Parameters::default();
        p.set("n", "100").unwrap();
        p.set("w", "4in").unwrap();
        p.set("a", "5mm + 3").unwrap();
        p.set("ang", "30deg + 15").unwrap();
        assert_eq!(p.get("a"), Some(8.0));

        assert!(p.set_units(INCH).is_empty());
        assert_eq!(p.units, INCH);
        // Explicit units and plain numbers keep their value…
        assert!(close(p.get("w").unwrap(), 101.6));
        assert_eq!(p.get("n"), Some(100.0));
        assert_eq!(p.get("ang"), Some(45.0));
        assert_eq!(p.entries[1].display(&p.units), "4 in");
        // …a bare number next to a length follows the document unit.
        assert!(close(p.get("a").unwrap(), 81.2));

        assert!(p.set_units(MM).is_empty());
        assert_eq!(p.get("a"), Some(8.0));
    }

    #[test]
    fn evaluate_against_the_table() {
        use QuantityKind::{Angle, Length, Number};
        let mut p = Parameters::default();
        p.set("n", "100").unwrap();
        p.set("w", "4in").unwrap();
        assert_eq!(p.evaluate_as("n / 2", Length), Ok(50.0));
        assert_eq!(p.evaluate_as("w / 2", Length), Ok(50.8));
        assert_eq!(p.evaluate_as("n / 2", Angle), Ok(50.0));
        assert_eq!(p.evaluate_as("w / 1mm", Number), Ok(101.6));
        assert_q(p.evaluate_expression("w * 2").unwrap(), 203.2, Dim::LENGTH);
        p.set_units(INCH);
        assert_eq!(p.evaluate_as("n / 2", Length), Ok(1270.0));
        assert_eq!(p.evaluate_as("w / 2", Length), Ok(50.8));
        assert_eq!(
            p.evaluate_as("w", Angle).unwrap_err().message,
            "this value needs an angle, but the expression gives a length (4 in)"
        );
        assert_eq!(
            p.evaluate_as("wx", Length).unwrap_err().message,
            "unknown name 'wx' at column 1 — did you mean 'w'?"
        );
        p.entries.push(Parameter::new("bad", "1 / 0"));
        p.evaluate();
        assert_eq!(
            p.evaluate_as("2 * bad", Length).unwrap_err().message,
            "'bad' at column 5 is a parameter with an error"
        );
    }

    #[test]
    fn parameters_serde_round_trip() {
        let mut p = Parameters {
            units: INCH,
            ..Parameters::default()
        };
        p.set("n", "100").unwrap();
        p.set("w", "4in").unwrap();
        p.set("a", "45deg").unwrap();
        p.set("d", "w / 2 + 1").unwrap();
        let text = ron::to_string(&p).unwrap();
        let mut back: Parameters = ron::from_str(&text).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.units.length, LengthUnit::Inch);
        assert_eq!(back.entries[1].kind, QuantityKind::Length);
        assert_eq!(back.entries[2].kind, QuantityKind::Angle);
        // Evaluating what was loaded changes nothing.
        assert!(back.evaluate().is_empty());
        assert_eq!(back, p);

        for u in LengthUnit::ALL {
            let units = Units::new(u);
            let text = ron::to_string(&units).unwrap();
            assert_eq!(ron::from_str::<Units>(&text).unwrap(), units);
        }

        // A table written before there were units: mm, and kinds come from evaluating.
        let old = r#"(entries:[(name:"w",expression:"5mm",value:5.0)])"#;
        let mut loaded: Parameters = ron::from_str(old).unwrap();
        assert_eq!(loaded.units, MM);
        assert_eq!(loaded.entries[0].kind, QuantityKind::Number);
        assert!(loaded.evaluate().is_empty());
        assert_eq!(loaded.entries[0].kind, QuantityKind::Length);
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
                value: 0.0,
                ..Parameter::new(n, e)
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

    /// d1: the length of a 10 mm line, d2: a 2 mm radius, d3: an angle.
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

    fn inch_document() -> Parameters {
        Parameters {
            units: INCH,
            ..Parameters::default()
        }
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
    fn dimension_number_with_unit_is_a_plain_value() {
        let (mut s, d1, _, d3) = sketch_with_dims();
        let p = Parameters::default();
        assert_eq!(set_dimension_input(&mut s, &p, d1, "2in").unwrap(), 50.8);
        assert_eq!(dim(&s, d1), (50.8, None));
        assert_eq!(set_dimension_input(&mut s, &p, d1, "50 mm").unwrap(), 50.0);
        assert_eq!(dim(&s, d1), (50.0, None));
        assert_eq!(set_dimension_input(&mut s, &p, d1, "5\"").unwrap(), 127.0);
        assert_eq!(dim(&s, d1), (127.0, None));
        assert_eq!(set_dimension_input(&mut s, &p, d1, "+1.5cm").unwrap(), 15.0);
        assert_eq!(dim(&s, d1), (15.0, None));
        assert_eq!(set_dimension_input(&mut s, &p, d3, "45°").unwrap(), 45.0);
        assert_eq!(dim(&s, d3), (45.0, None));
        assert_eq!(set_dimension_input(&mut s, &p, d3, "60 deg").unwrap(), 60.0);
        assert_eq!(dim(&s, d3), (60.0, None));
        // Anything more is an expression.
        assert_eq!(
            set_dimension_input(&mut s, &p, d1, "2 * 1in").unwrap(),
            50.8
        );
        assert_eq!(dim(&s, d1), (50.8, Some("2 * 1in".into())));
        assert_eq!(set_dimension_input(&mut s, &p, d1, "(12)").unwrap(), 12.0);
        assert_eq!(dim(&s, d1), (12.0, Some("(12)".into())));
    }

    #[test]
    fn dimension_inputs_in_an_inch_document() {
        let (mut s, d1, d2, d3) = sketch_with_dims();
        let p = inch_document();
        // A plain number is in document units and stored in mm.
        assert_eq!(set_dimension_input(&mut s, &p, d1, "2").unwrap(), 50.8);
        assert_eq!(dim(&s, d1), (50.8, None));
        assert!(close(
            set_dimension_input(&mut s, &p, d1, "0.75").unwrap(),
            19.05
        ));
        // A unit suffix overrides the document unit; still a plain value.
        assert_eq!(set_dimension_input(&mut s, &p, d1, "50 mm").unwrap(), 50.0);
        assert_eq!(dim(&s, d1), (50.0, None));
        assert_eq!(set_dimension_input(&mut s, &p, d1, "5in").unwrap(), 127.0);
        assert_eq!(dim(&s, d1), (127.0, None));
        // Expressions: dimension names are lengths in mm, bare numbers are inches.
        assert_eq!(
            set_dimension_input(&mut s, &p, d2, "d1 / 4").unwrap(),
            31.75
        );
        assert_eq!(dim(&s, d2), (31.75, Some("d1 / 4".into())));
        assert!(close(
            set_dimension_input(&mut s, &p, d2, "d1 / 4 + 1").unwrap(),
            57.15
        ));
        assert!(close(
            set_dimension_input(&mut s, &p, d2, "1 + 1").unwrap(),
            50.8
        ));
        assert!(close(
            set_dimension_input(&mut s, &p, d2, "d1 / 4 + 1mm").unwrap(),
            32.75
        ));
        // Angles are degrees in every document.
        assert_eq!(set_dimension_input(&mut s, &p, d3, "45").unwrap(), 45.0);
        assert_eq!(dim(&s, d3), (45.0, None));
        assert_eq!(
            set_dimension_input(&mut s, &p, d3, "30deg + 15").unwrap(),
            45.0
        );
        assert_eq!(dim(&s, d3), (45.0, Some("30deg + 15".into())));
        // Messages are in document units.
        let e = set_dimension_input(&mut s, &p, d1, "-5").unwrap_err();
        assert!(
            e.message.ends_with("must be greater than zero, got -5 in"),
            "{}",
            e.message
        );
        let e = set_dimension_input(&mut s, &p, d2, "d1 * 1in").unwrap_err();
        assert!(e.message.contains("an area (5 in²)"), "{}", e.message);
    }

    #[test]
    fn dimension_inputs_must_be_of_the_right_kind() {
        let (mut s, d1, d2, d3) = sketch_with_dims();
        let p = Parameters::default();
        set_dimension_input(&mut s, &p, d3, "30").unwrap();
        let before = s.clone();
        let mut e = |id, input: &str| {
            set_dimension_input(&mut s, &p, id, input)
                .unwrap_err()
                .message
        };
        assert_eq!(
            e(d1, "30deg"),
            "this dimension needs a length, but the expression gives an angle (30°)"
        );
        assert_eq!(
            e(d1, "d3 * 2"),
            "this dimension needs a length, but the expression gives an angle (60°)"
        );
        assert_eq!(
            e(d1, "d2 * 5mm"),
            "this dimension needs a length, but the expression gives an area (10 mm²): divide by a length"
        );
        assert_eq!(
            e(d1, "d2 * d2 * d2"),
            "this dimension needs a length, but the expression gives a volume (8 mm³): divide by an area"
        );
        assert_eq!(
            e(d3, "d1"),
            "this dimension needs an angle, but the expression gives a length (10 mm)"
        );
        assert_eq!(
            e(d3, "2in"),
            "this dimension needs an angle, but the expression gives a length (50.8 mm)"
        );
        assert_eq!(
            e(d1, "d2 + d3"),
            "can't add a length (2 mm) and an angle (30°)"
        );
        assert_eq!(
            e(d2, "sin(d1)"),
            "sin at column 1 needs an angle, but got a length (10 mm)"
        );
        assert_eq!(s, before);

        // Dimension names are lengths or angles according to the dimension.
        assert!(close(
            set_dimension_input(&mut s, &p, d2, "d1 * sin(d3)").unwrap(),
            5.0
        ));
        assert!(close(
            set_dimension_input(&mut s, &p, d2, "d1 / 1mm * 0.5mm").unwrap(),
            5.0
        ));
        assert!(close(
            set_dimension_input(&mut s, &p, d3, "atan2(d2, d1)").unwrap(),
            5f64.atan2(10.0).to_degrees()
        ));
        assert!(close(
            set_dimension_input(&mut s, &p, d3, "d1 / d2 * 10").unwrap(),
            20.0
        ));
        assert!(close(
            set_dimension_input(&mut s, &p, d2, "sqrt(d1 * 2.5mm)").unwrap(),
            5.0
        ));
    }

    #[test]
    fn dimensions_use_parameters_by_kind() {
        let (mut s, d1, d2, d3) = sketch_with_dims();
        let mut p = inch_document();
        p.set("n", "40").unwrap();
        p.set("w", "40mm").unwrap();
        p.set("a", "60deg").unwrap();
        // A number parameter adopts the document unit, a length is what it is.
        assert_eq!(set_dimension_input(&mut s, &p, d1, "n").unwrap(), 1016.0);
        assert_eq!(set_dimension_input(&mut s, &p, d1, "w").unwrap(), 40.0);
        assert_eq!(set_dimension_input(&mut s, &p, d2, "w / 2").unwrap(), 20.0);
        assert_eq!(set_dimension_input(&mut s, &p, d3, "a").unwrap(), 60.0);
        assert_eq!(set_dimension_input(&mut s, &p, d3, "n").unwrap(), 40.0);
        let e = set_dimension_input(&mut s, &p, d1, "a").unwrap_err();
        assert_eq!(
            e.message,
            "this dimension needs a length, but the expression gives an angle (60°)"
        );
        let e = set_dimension_input(&mut s, &p, d3, "w").unwrap_err();
        assert_eq!(
            e.message,
            "this dimension needs an angle, but the expression gives a length (1.5748 in)"
        );
    }

    #[test]
    fn switching_document_units_keeps_plain_values() {
        let (mut s, d1, d2, d3) = sketch_with_dims();
        let mut p = inch_document();
        p.set("gap", "1").unwrap();
        p.set("wall", "3mm").unwrap();
        set_dimension_input(&mut s, &p, d1, "2").unwrap();
        set_dimension_input(&mut s, &p, d3, "60").unwrap();
        set_dimension_input(&mut s, &p, d2, "d1 / 2 + gap").unwrap();
        assert_eq!(dim(&s, d1), (50.8, None));
        assert!(close(dim(&s, d2).0, 50.8));

        assert!(p.set_units(MM).is_empty());
        assert!(apply_expressions(&mut s, &p).is_empty());
        // Plain values and explicit units don't move.
        assert_eq!(dim(&s, d1), (50.8, None));
        assert_eq!(dim(&s, d3), (60.0, None));
        assert_eq!(p.get("wall"), Some(3.0));
        // The bare `gap = 1` is now 1 mm instead of 1 in.
        assert!(close(dim(&s, d2).0, 26.4));

        // Pinned with a unit, the expression survives the switch.
        set_dimension_input(&mut s, &p, d2, "d1 / 2 + wall + 1mm").unwrap();
        assert!(close(dim(&s, d2).0, 29.4));
        p.set_units(INCH);
        assert!(apply_expressions(&mut s, &p).is_empty());
        assert!(close(dim(&s, d2).0, 29.4));
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
        assert!(
            e.message.ends_with("must be greater than zero, got -5 mm"),
            "{}",
            e.message
        );
        let e = set_dimension_input(&mut s, &p, d1, "d2 - 2").unwrap_err();
        assert!(e.message.contains("greater than zero"), "{}", e.message);
        let e = set_dimension_input(&mut s, &p, d3, "200").unwrap_err();
        assert!(e.message.contains("at most 180°"), "{}", e.message);
        assert!(set_dimension_input(&mut s, &p, d3, "0deg").is_err());
        let e = set_dimension_input(&mut s, &p, d1, "1 / 0").unwrap_err();
        assert_eq!(e.message, "division by zero at column 3");
        let e = set_dimension_input(&mut s, &p, d1, "10 +").unwrap_err();
        assert!(e.message.starts_with("unexpected end"));
        let e = set_dimension_input(&mut s, &p, d1, "").unwrap_err();
        assert_eq!(e.message, "enter a value or an expression");
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
        p.entries.push(Parameter::new("bad", "1 / 0"));
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
    fn apply_expressions_in_an_inch_document() {
        let (mut s, d1, d2, _) = sketch_with_dims();
        let mut p = inch_document();
        p.set("w", "4").unwrap();
        // d2 is a length (in mm) by the time d1 uses it, although `w / 8` is a number.
        set_dimension_input(&mut s, &p, d2, "w / 8").unwrap();
        set_dimension_input(&mut s, &p, d1, "d2 * 4 + 1").unwrap();
        assert!(close(dim(&s, d2).0, 12.7));
        assert!(close(dim(&s, d1).0, 76.2));
        p.set("w", "8").unwrap();
        assert!(apply_expressions(&mut s, &p).is_empty());
        assert!(close(dim(&s, d2).0, 25.4));
        assert!(close(dim(&s, d1).0, 127.0));
        // A parameter that turns into the wrong kind fails the dimension, which keeps
        // its value.
        p.set("w", "8deg").unwrap();
        let failures = apply_expressions(&mut s, &p);
        assert_eq!(failures.len(), 2);
        assert_eq!(
            failures.iter().find(|(id, _)| *id == d2).unwrap().1.message,
            "this dimension needs a length, but the expression gives an angle (1°)"
        );
        assert!(close(dim(&s, d2).0, 25.4));
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
