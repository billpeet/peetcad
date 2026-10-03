//! STEP export: the files are well-formed Part 21 (checked with a small parser below), and
//! the B-rep read back from them matches the kernel's solids: face counts, manifold edges,
//! vertices on their curves at the right ends, loop orientation and enclosed volume.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::f64::consts::{PI, TAU};

use peet_io::step::{self, StepOptions, StepSchema};
use peet_kernel::boolean::{BooleanOp, boolean};
use peet_kernel::extrude::extrude;
use peet_kernel::validate::{measure, validate};
use peet_kernel::{Curve3, Solid};
use peet_math::{DQuat, DVec2, DVec3, Frame, Plane};
use peet_sheetmetal::{
    BendModel, EdgeFlangeSpec, EdgeSite, FlangePosition, Layout, ReliefType, SheetSettings,
};
use peet_sketch::region::find_regions;
use peet_sketch::{Sketch, shapes};

// ---- A minimal Part 21 reader ----

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Ref(u32),
    Int(i64),
    Real(f64),
    Str(String),
    Enum(String),
    List(Vec<Value>),
    /// A typed parameter such as `LENGTH_MEASURE(1.0E-06)`.
    Typed(String, Vec<Value>),
    Unset,
    Derived,
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Open,
    Close,
    Comma,
    Semi,
    Equals,
    Ref(u32),
    Int(i64),
    Real(f64),
    Str(String),
    Enum(String),
    Keyword(String),
    Dollar,
    Star,
}

fn decode_string(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(i) = rest.find('\\') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if let Some(r) = rest.strip_prefix("\\\\") {
            out.push('\\');
            rest = r;
        } else if let Some(r) = rest.strip_prefix("\\X2\\").or(rest.strip_prefix("\\X4\\")) {
            let width = if rest.starts_with("\\X2\\") { 4 } else { 8 };
            let end = r.find("\\X0\\").expect("unterminated \\X2\\ or \\X4\\ run");
            let hex = &r[..end];
            assert_eq!(hex.len() % width, 0, "bad hex run {hex}");
            for k in (0..hex.len()).step_by(width) {
                let code = u32::from_str_radix(&hex[k..k + width], 16).unwrap();
                out.push(char::from_u32(code).unwrap());
            }
            rest = &r[end + 4..];
        } else {
            panic!("unknown escape in string: {rest}");
        }
    }
    out.push_str(rest);
    out
}

fn tokenize(text: &str) -> Vec<Token> {
    let b = text.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i] as char;
        match c {
            _ if c.is_ascii_whitespace() => i += 1,
            '/' if b.get(i + 1) == Some(&b'*') => {
                i += text[i..].find("*/").expect("unterminated comment") + 2;
            }
            '(' | ')' | ',' | ';' | '=' | '$' | '*' => {
                out.push(match c {
                    '(' => Token::Open,
                    ')' => Token::Close,
                    ',' => Token::Comma,
                    ';' => Token::Semi,
                    '=' => Token::Equals,
                    '$' => Token::Dollar,
                    _ => Token::Star,
                });
                i += 1;
            }
            '#' => {
                let start = i + 1;
                i = start;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                out.push(Token::Ref(text[start..i].parse().unwrap()));
            }
            '\'' => {
                let mut raw = String::new();
                i += 1;
                loop {
                    let ch = b[i] as char;
                    assert!(
                        (' '..='~').contains(&ch),
                        "non-printable byte {ch:?} in a string"
                    );
                    if ch == '\'' {
                        if b.get(i + 1) == Some(&b'\'') {
                            raw.push('\'');
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    raw.push(ch);
                    i += 1;
                }
                out.push(Token::Str(decode_string(&raw)));
            }
            '.' => {
                let end = text[i + 1..].find('.').unwrap() + i + 1;
                let name = &text[i + 1..end];
                assert!(name.chars().all(|c| c.is_ascii_uppercase() || c == '_'));
                out.push(Token::Enum(name.to_owned()));
                i = end + 1;
            }
            _ if c.is_ascii_digit() || c == '-' || c == '+' => {
                let start = i;
                i += 1;
                while i < b.len()
                    && (b[i].is_ascii_digit() || matches!(b[i], b'.' | b'E' | b'+' | b'-'))
                {
                    i += 1;
                }
                let t = &text[start..i];
                if t.contains('.') {
                    // Part 21: digits, a point, digits, then an optional exponent.
                    let mantissa = t.split('E').next().unwrap();
                    let point = mantissa.find('.').unwrap();
                    assert!(
                        mantissa[..point]
                            .trim_start_matches(['-', '+'])
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_digit()),
                        "real {t} needs a digit before the point"
                    );
                    out.push(Token::Real(t.parse().unwrap()));
                } else {
                    assert!(!t.contains('E'), "real {t} without a decimal point");
                    out.push(Token::Int(t.parse().unwrap()));
                }
            }
            _ if c.is_ascii_alphabetic() => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'_' | b'-')) {
                    i += 1;
                }
                out.push(Token::Keyword(text[start..i].to_owned()));
            }
            _ => panic!("unexpected character {c:?} at byte {i}"),
        }
    }
    out
}

/// A complex instance has several `(name, parameters)` parts; a simple one has one.
type Instance = Vec<(String, Vec<Value>)>;

struct File {
    header: Vec<(String, Vec<Value>)>,
    data: BTreeMap<u32, Instance>,
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn next(&mut self) -> Token {
        let t = self
            .tokens
            .get(self.at)
            .cloned()
            .expect("unexpected end of file");
        self.at += 1;
        t
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn expect(&mut self, t: Token) {
        let got = self.next();
        assert_eq!(got, t, "at token {}", self.at - 1);
    }

    fn keyword(&mut self) -> String {
        match self.next() {
            Token::Keyword(k) => k,
            t => panic!("expected a keyword, got {t:?}"),
        }
    }

    /// `( value, value, ... )` after the opening parenthesis has been read.
    fn values(&mut self) -> Vec<Value> {
        let mut out = Vec::new();
        if self.peek() == Some(&Token::Close) {
            self.next();
            return out;
        }
        loop {
            out.push(self.value());
            match self.next() {
                Token::Comma => {}
                Token::Close => return out,
                t => panic!("expected , or ), got {t:?}"),
            }
        }
    }

    fn value(&mut self) -> Value {
        match self.next() {
            Token::Ref(r) => Value::Ref(r),
            Token::Int(v) => Value::Int(v),
            Token::Real(v) => Value::Real(v),
            Token::Str(s) => Value::Str(s),
            Token::Enum(e) => Value::Enum(e),
            Token::Dollar => Value::Unset,
            Token::Star => Value::Derived,
            Token::Open => Value::List(self.values()),
            Token::Keyword(k) => {
                self.expect(Token::Open);
                Value::Typed(k, self.values())
            }
            t => panic!("unexpected {t:?} as a value"),
        }
    }

    fn record(&mut self) -> (String, Vec<Value>) {
        let name = self.keyword();
        self.expect(Token::Open);
        (name, self.values())
    }
}

fn parse(text: &str) -> File {
    assert!(text.is_ascii(), "Part 21 files are plain ASCII");
    let mut p = Parser {
        tokens: tokenize(text),
        at: 0,
    };
    assert_eq!(p.keyword(), "ISO-10303-21");
    p.expect(Token::Semi);
    assert_eq!(p.keyword(), "HEADER");
    p.expect(Token::Semi);
    let mut header = Vec::new();
    while p.peek() != Some(&Token::Keyword("ENDSEC".into())) {
        header.push(p.record());
        p.expect(Token::Semi);
    }
    p.next();
    p.expect(Token::Semi);
    assert_eq!(p.keyword(), "DATA");
    p.expect(Token::Semi);
    let mut data = BTreeMap::new();
    loop {
        let id = match p.next() {
            Token::Ref(id) => id,
            Token::Keyword(k) if k == "ENDSEC" => break,
            t => panic!("expected an instance, got {t:?}"),
        };
        p.expect(Token::Equals);
        let instance = if p.peek() == Some(&Token::Open) {
            p.next();
            let mut parts = Vec::new();
            while p.peek() != Some(&Token::Close) {
                parts.push(p.record());
            }
            p.next();
            let names: Vec<&String> = parts.iter().map(|(n, _)| n).collect();
            assert!(
                names.is_sorted(),
                "complex instance #{id}: parts not in order"
            );
            parts
        } else {
            vec![p.record()]
        };
        p.expect(Token::Semi);
        assert!(data.insert(id, instance).is_none(), "#{id} defined twice");
    }
    p.expect(Token::Semi);
    assert_eq!(p.keyword(), "END-ISO-10303-21");
    p.expect(Token::Semi);
    assert!(p.peek().is_none(), "trailing tokens");
    File { header, data }
}

fn refs_in(values: &[Value], out: &mut Vec<u32>) {
    for v in values {
        match v {
            Value::Ref(r) => out.push(*r),
            Value::List(l) | Value::Typed(_, l) => refs_in(l, out),
            _ => {}
        }
    }
}

const ENTITIES: &[&str] = &[
    "APPLICATION_CONTEXT",
    "APPLICATION_PROTOCOL_DEFINITION",
    "PRODUCT_CONTEXT",
    "PRODUCT",
    "PRODUCT_RELATED_PRODUCT_CATEGORY",
    "PRODUCT_DEFINITION_FORMATION",
    "PRODUCT_DEFINITION_CONTEXT",
    "PRODUCT_DEFINITION",
    "PRODUCT_DEFINITION_SHAPE",
    "SHAPE_DEFINITION_REPRESENTATION",
    "ADVANCED_BREP_SHAPE_REPRESENTATION",
    "UNCERTAINTY_MEASURE_WITH_UNIT",
    "MANIFOLD_SOLID_BREP",
    "BREP_WITH_VOIDS",
    "ORIENTED_CLOSED_SHELL",
    "CLOSED_SHELL",
    "ADVANCED_FACE",
    "FACE_OUTER_BOUND",
    "FACE_BOUND",
    "EDGE_LOOP",
    "ORIENTED_EDGE",
    "EDGE_CURVE",
    "VERTEX_POINT",
    "LINE",
    "VECTOR",
    "CIRCLE",
    "ELLIPSE",
    "PLANE",
    "CYLINDRICAL_SURFACE",
    "AXIS2_PLACEMENT_3D",
    "CARTESIAN_POINT",
    "DIRECTION",
];

const COMPLEX: &[&[&str]] = &[
    &["LENGTH_UNIT", "NAMED_UNIT", "SI_UNIT"],
    &["NAMED_UNIT", "PLANE_ANGLE_UNIT", "SI_UNIT"],
    &["NAMED_UNIT", "SI_UNIT", "SOLID_ANGLE_UNIT"],
    &[
        "GEOMETRIC_REPRESENTATION_CONTEXT",
        "GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT",
        "GLOBAL_UNIT_ASSIGNED_CONTEXT",
        "REPRESENTATION_CONTEXT",
    ],
];

impl File {
    /// A simple instance's name and parameters.
    fn get(&self, id: u32) -> (&str, &[Value]) {
        let inst = &self.data[&id];
        assert_eq!(inst.len(), 1, "#{id} is a complex instance");
        (&inst[0].0, &inst[0].1)
    }

    /// The parameters of `id`, which must be a `name`.
    fn params(&self, id: u32, name: &str) -> &[Value] {
        let (n, p) = self.get(id);
        assert_eq!(n, name, "#{id}");
        p
    }

    fn all(&self, name: &str) -> Vec<u32> {
        self.data
            .iter()
            .filter(|(_, inst)| inst.len() == 1 && inst[0].0 == name)
            .map(|(&id, _)| id)
            .collect()
    }

    fn count(&self, name: &str) -> usize {
        self.all(name).len()
    }

    fn header(&self, name: &str) -> &[Value] {
        &self.header.iter().find(|(n, _)| n == name).unwrap().1
    }

    fn point(&self, id: u32) -> DVec3 {
        vec3(&self.params(id, "CARTESIAN_POINT")[1])
    }

    fn direction(&self, id: u32) -> DVec3 {
        vec3(&self.params(id, "DIRECTION")[1])
    }

    /// An `AXIS2_PLACEMENT_3D` as a frame.
    fn frame(&self, id: u32) -> Frame {
        let p = self.params(id, "AXIS2_PLACEMENT_3D");
        let (z, x) = (self.direction(r(&p[2])), self.direction(r(&p[3])));
        assert!((z.length() - 1.0).abs() < 1e-12 && (x.length() - 1.0).abs() < 1e-12);
        assert!(z.dot(x).abs() < 1e-12);
        Frame::from_origin_z_x(self.point(r(&p[1])), z, x).unwrap()
    }

    fn vertex(&self, id: u32) -> DVec3 {
        self.point(r(&self.params(id, "VERTEX_POINT")[1]))
    }
}

fn r(v: &Value) -> u32 {
    match v {
        Value::Ref(r) => *r,
        v => panic!("expected a reference, got {v:?}"),
    }
}

fn real(v: &Value) -> f64 {
    match v {
        Value::Real(x) => *x,
        v => panic!("expected a real, got {v:?}"),
    }
}

fn logical(v: &Value) -> bool {
    match v {
        Value::Enum(e) if e == "T" => true,
        Value::Enum(e) if e == "F" => false,
        v => panic!("expected .T. or .F., got {v:?}"),
    }
}

fn text(v: &Value) -> &str {
    match v {
        Value::Str(s) => s,
        v => panic!("expected a string, got {v:?}"),
    }
}

fn list(v: &Value) -> &[Value] {
    match v {
        Value::List(l) => l,
        v => panic!("expected a list, got {v:?}"),
    }
}

fn vec3(v: &Value) -> DVec3 {
    let l = list(v);
    assert_eq!(l.len(), 3);
    DVec3::new(real(&l[0]), real(&l[1]), real(&l[2]))
}

// ---- Geometry read back from the file ----

enum Curve {
    Line {
        origin: DVec3,
        dir: DVec3,
    },
    /// Circle (`a == b`) or ellipse: `centre + x·a·cos t + y·b·sin t`.
    Conic {
        frame: Frame,
        a: f64,
        b: f64,
    },
}

impl Curve {
    fn read(f: &File, id: u32) -> Self {
        match f.get(id) {
            ("LINE", p) => {
                let v = f.params(r(&p[2]), "VECTOR");
                assert_eq!(real(&v[2]), 1.0);
                Self::Line {
                    origin: f.point(r(&p[1])),
                    dir: f.direction(r(&v[1])),
                }
            }
            ("CIRCLE", p) => Self::Conic {
                frame: f.frame(r(&p[1])),
                a: real(&p[2]),
                b: real(&p[2]),
            },
            ("ELLIPSE", p) => Self::Conic {
                frame: f.frame(r(&p[1])),
                a: real(&p[2]),
                b: real(&p[3]),
            },
            (name, _) => panic!("#{id}: unexpected curve {name}"),
        }
    }

    fn point(&self, t: f64) -> DVec3 {
        match self {
            Self::Line { origin, dir } => *origin + *dir * t,
            Self::Conic { frame, a, b } => {
                frame.to_world(DVec3::new(a * t.cos(), b * t.sin(), 0.0))
            }
        }
    }

    /// Parameter of a point on the curve (conics: in `(-π, π]`).
    fn param(&self, p: DVec3) -> f64 {
        match self {
            Self::Line { origin, dir } => (p - *origin).dot(*dir),
            Self::Conic { frame, a, b } => {
                let l = frame.to_local(p);
                (l.y / b).atan2(l.x / a)
            }
        }
    }
}

struct EdgeCurve {
    start: u32,
    end: u32,
    curve: Curve,
    same_sense: bool,
}

impl EdgeCurve {
    fn read(f: &File, id: u32) -> Self {
        let p = f.params(id, "EDGE_CURVE");
        Self {
            start: r(&p[1]),
            end: r(&p[2]),
            curve: Curve::read(f, r(&p[3])),
            same_sense: logical(&p[4]),
        }
    }

    /// Points along the edge from its start vertex to its end vertex.
    fn samples(&self, f: &File) -> Vec<DVec3> {
        let (a, b) = (f.vertex(self.start), f.vertex(self.end));
        match self.curve {
            Curve::Line { .. } => vec![a, b],
            Curve::Conic { .. } => {
                // The edge is the arc from `start` to `end` in the curve's direction (or
                // against it without `same_sense`); a full turn when they coincide.
                let t0 = self.curve.param(a);
                let mut sweep = (self.curve.param(b) - t0).rem_euclid(TAU);
                if self.start == self.end {
                    sweep = TAU;
                }
                if !self.same_sense {
                    sweep -= TAU;
                }
                (0..=256)
                    .map(|i| self.curve.point(t0 + sweep * f64::from(i) / 256.0))
                    .collect()
            }
        }
    }
}

enum Surf {
    Plane(Frame),
    Cylinder(Frame, f64),
}

/// Results of reading one `ADVANCED_FACE`.
struct FaceInfo {
    area: f64,
    /// `∬ (p − ref) · n dA` over the face (n the oriented normal); a third of its sum over
    /// a closed shell is the enclosed volume.
    flux: f64,
}

/// Reads a face, checks its loops (connected, on the surface, outer loop counter-clockwise
/// and holes clockwise seen from outside) and integrates its area and volume flux.
fn read_face(f: &File, id: u32, reference: DVec3) -> FaceInfo {
    let p = f.params(id, "ADVANCED_FACE");
    let sense = if logical(&p[3]) { 1.0 } else { -1.0 };
    let surface = match f.get(r(&p[2])) {
        ("PLANE", s) => Surf::Plane(f.frame(r(&s[1]))),
        ("CYLINDRICAL_SURFACE", s) => Surf::Cylinder(f.frame(r(&s[1])), real(&s[2])),
        (name, _) => panic!("unexpected surface {name}"),
    };
    let bounds = list(&p[1]);
    let (mut area, mut flux) = (0.0, 0.0);
    for (k, b) in bounds.iter().enumerate() {
        let (kind, bp) = f.get(r(b));
        assert_eq!(
            kind,
            if k == 0 {
                "FACE_OUTER_BOUND"
            } else {
                "FACE_BOUND"
            }
        );
        assert!(logical(&bp[2]), "bounds are written with orientation .T.");
        let oriented = list(&f.params(r(&bp[1]), "EDGE_LOOP")[1]);
        // Walk the loop, checking that each edge starts where the previous one ended.
        let mut points: Vec<DVec3> = Vec::new();
        let mut ends = Vec::new();
        for o in oriented {
            let op = f.params(r(o), "ORIENTED_EDGE");
            assert_eq!((&op[1], &op[2]), (&Value::Derived, &Value::Derived));
            let edge = EdgeCurve::read(f, r(&op[3]));
            let forward = logical(&op[4]);
            let mut s = edge.samples(f);
            if !forward {
                s.reverse();
            }
            ends.push(if forward {
                (edge.start, edge.end)
            } else {
                (edge.end, edge.start)
            });
            points.extend(s);
        }
        for i in 0..ends.len() {
            assert_eq!(
                ends[i].1,
                ends[(i + 1) % ends.len()].0,
                "loop is not connected"
            );
        }
        // Parameter-space polygon (u to the right, v up, seen from the natural normal).
        let uv: Vec<DVec2> = match surface {
            Surf::Plane(frame) => points
                .iter()
                .map(|&q| {
                    let l = frame.to_local(q);
                    assert!(l.z.abs() < 1e-9, "loop point off its plane by {}", l.z);
                    DVec2::new(l.x, l.y)
                })
                .collect(),
            Surf::Cylinder(frame, radius) => {
                let mut prev: Option<f64> = None;
                points
                    .iter()
                    .map(|&q| {
                        let l = frame.to_local(q);
                        let d = DVec2::new(l.x, l.y).length() - radius;
                        assert!(d.abs() < 1e-9, "loop point off its cylinder by {d}");
                        let mut a = l.y.atan2(l.x);
                        if let Some(p) = prev {
                            a += ((p - a) / TAU).round() * TAU;
                        }
                        prev = Some(a);
                        DVec2::new(a, l.z)
                    })
                    .collect()
            }
        };
        let n = uv.len();
        let mut shoelace = 0.0;
        let (mut sin_dv, mut cos_dv) = (0.0, 0.0);
        for i in 0..n {
            let (a, b) = (uv[i], uv[(i + 1) % n]);
            shoelace += 0.5 * a.perp_dot(b);
            let dv = b.y - a.y;
            sin_dv += 0.5 * (a.x.sin() + b.x.sin()) * dv;
            cos_dv += 0.5 * (a.x.cos() + b.x.cos()) * dv;
        }
        // Green's theorem in parameter space. The loop runs counter-clockwise about the
        // oriented normal, so the line integrals already carry the face's sense.
        let (loop_area, loop_flux) = match surface {
            Surf::Plane(frame) => (
                shoelace,
                (frame.origin - reference).dot(frame.z_axis()) * shoelace,
            ),
            Surf::Cylinder(frame, radius) => {
                // ∬ n dA = r (x ∮ sin u dv − y ∮ cos u dv) and (p − origin) · n = r.
                let normal_integral = (frame.x_axis() * sin_dv - frame.y_axis() * cos_dv) * radius;
                (
                    radius * shoelace,
                    radius * radius * shoelace + (frame.origin - reference).dot(normal_integral),
                )
            }
        };
        let oriented_area = sense * loop_area;
        if k == 0 {
            assert!(oriented_area > 0.0, "outer loop of #{id} runs clockwise");
        } else {
            assert!(oriented_area < 0.0, "hole of #{id} runs counter-clockwise");
        }
        area += oriented_area;
        flux += loop_flux;
    }
    FaceInfo { area, flux }
}

/// Volume enclosed by a `CLOSED_SHELL` (positive when its faces point outwards), and its
/// faces' areas.
fn read_shell(f: &File, id: u32) -> (f64, Vec<f64>) {
    let p = f.params(id, "CLOSED_SHELL");
    let faces = list(&p[1]);
    let reference = {
        let face = f.params(r(&faces[0]), "ADVANCED_FACE");
        f.frame(r(&f.get(r(&face[2])).1[1])).origin
    };
    let infos: Vec<FaceInfo> = faces
        .iter()
        .map(|x| read_face(f, r(x), reference))
        .collect();
    (
        infos.iter().map(|i| i.flux).sum::<f64>() / 3.0,
        infos.iter().map(|i| i.area).collect(),
    )
}

fn options(schema: StepSchema) -> StepOptions {
    StepOptions {
        schema,
        product_name: "Test part".to_owned(),
        author: "Bill".to_owned(),
        organization: String::new(),
        timestamp: "2026-10-03T12:00:00".to_owned(),
    }
}

/// Exports `bodies`, then checks the file's syntax and structure and the B-rep against the
/// kernel's solids. Returns the parsed file for test-specific checks.
#[track_caller]
fn export_and_check(bodies: &[(&str, &Solid)], schema: StepSchema) -> File {
    for (name, s) in bodies {
        if let Err(p) = validate(s) {
            panic!("{name} is invalid: {p:#?}");
        }
    }
    let f = parse(&step::write(bodies, &options(schema)));

    // Header.
    let schema_text = match &f.header("FILE_SCHEMA")[0] {
        Value::List(l) => text(&l[0]),
        v => panic!("{v:?}"),
    };
    assert_eq!(schema_text, schema.file_schema());
    assert_eq!(text(&f.header("FILE_NAME")[0]), "Test part");
    assert_eq!(text(&f.header("FILE_NAME")[1]), "2026-10-03T12:00:00");
    assert_eq!(
        list(&f.header("FILE_NAME")[2]),
        &[Value::Str("Bill".into())]
    );
    let protocol = f.params(
        f.all("APPLICATION_PROTOCOL_DEFINITION")[0],
        "APPLICATION_PROTOCOL_DEFINITION",
    );
    assert_eq!(
        text(&protocol[1]),
        match schema {
            StepSchema::Ap214 => "automotive_design",
            StepSchema::Ap242 => "ap242_managed_model_based_3d_engineering",
        }
    );

    // Every reference resolves, every name is known, and everything hangs off the roots.
    let mut referenced: HashMap<u32, usize> = HashMap::new();
    for (id, inst) in &f.data {
        let names: Vec<&str> = inst.iter().map(|(n, _)| n.as_str()).collect();
        if names.len() == 1 {
            assert!(
                ENTITIES.contains(&names[0]),
                "#{id}: unexpected {}",
                names[0]
            );
        } else {
            assert!(COMPLEX.contains(&&names[..]), "#{id}: unexpected {names:?}");
        }
        let mut refs = Vec::new();
        for (_, params) in inst {
            refs_in(params, &mut refs);
        }
        for t in refs {
            assert!(f.data.contains_key(&t), "#{id} refers to undefined #{t}");
            *referenced.entry(t).or_default() += 1;
        }
    }
    let roots = [
        "SHAPE_DEFINITION_REPRESENTATION",
        "APPLICATION_PROTOCOL_DEFINITION",
        "PRODUCT_RELATED_PRODUCT_CATEGORY",
    ];
    for (id, inst) in &f.data {
        assert!(
            referenced.contains_key(id) || roots.contains(&inst[0].0.as_str()),
            "#{id} ({}) is not used",
            inst[0].0
        );
    }
    for name in roots {
        assert_eq!(f.count(name), 1);
    }

    // Product structure.
    let sdr = f.params(
        f.all("SHAPE_DEFINITION_REPRESENTATION")[0],
        "SHAPE_DEFINITION_REPRESENTATION",
    );
    let pds = f.params(r(&sdr[0]), "PRODUCT_DEFINITION_SHAPE");
    let pd = f.params(r(&pds[2]), "PRODUCT_DEFINITION");
    let pdf = f.params(r(&pd[2]), "PRODUCT_DEFINITION_FORMATION");
    let product = f.params(r(&pdf[2]), "PRODUCT");
    assert_eq!(
        (text(&product[0]), text(&product[1])),
        ("Test part", "Test part")
    );
    let absr = f.params(r(&sdr[1]), "ADVANCED_BREP_SHAPE_REPRESENTATION");
    let context = &f.data[&r(&absr[2])];
    assert_eq!(context[0].1, vec![Value::Int(3)]);
    let units = list(&context[2].1[0]);
    let mm = &f.data[&r(&units[0])];
    assert_eq!(
        mm[2].1,
        vec![Value::Enum("MILLI".into()), Value::Enum("METRE".into())]
    );
    assert_eq!(f.data[&r(&units[1])][2].1[1], Value::Enum("RADIAN".into()));
    assert_eq!(
        f.data[&r(&units[2])][1].1[1],
        Value::Enum("STERADIAN".into())
    );

    // Breps: one per lump, named after their bodies, with volumes matching the kernel's.
    let items = list(&absr[1]);
    assert_eq!(f.get(r(&items[0])).0, "AXIS2_PLACEMENT_3D");
    let mut volume = 0.0;
    let mut areas = Vec::new();
    let mut brep_names = Vec::new();
    for item in &items[1..] {
        let (kind, p) = f.get(r(item));
        brep_names.push(text(&p[0]).to_owned());
        let (v, a) = read_shell(&f, r(&p[1]));
        assert!(v > 0.0, "outer shell faces point inwards");
        volume += v;
        areas.extend(a);
        match kind {
            "MANIFOLD_SOLID_BREP" => assert_eq!(p.len(), 2),
            "BREP_WITH_VOIDS" => {
                for void in list(&p[2]) {
                    let o = f.params(r(void), "ORIENTED_CLOSED_SHELL");
                    assert_eq!(o[1], Value::Derived);
                    assert!(!logical(&o[3]), "voids are used reversed");
                    // The shell itself faces away from the cavity.
                    let (v, a) = read_shell(&f, r(&o[2]));
                    assert!(v > 0.0);
                    volume -= v;
                    areas.extend(a);
                }
            }
            _ => panic!("unexpected item {kind}"),
        }
    }
    let expected: f64 = bodies.iter().map(|(_, s)| measure::volume(s)).sum();
    assert!(
        (volume - expected).abs() < 1e-3 * expected,
        "volume {volume} != {expected}"
    );
    let mut expected_names: Vec<String> = Vec::new();
    for (name, s) in bodies {
        let lumps = (0..s.shells.len() as u32)
            .filter(|&k| measure::shell_volume(s, peet_kernel::ShellId(k)) > 0.0)
            .count();
        expected_names.extend(std::iter::repeat_n(name.to_string(), lumps));
    }
    assert_eq!(brep_names, expected_names);

    // Faces: one per kernel face, with the same areas.
    let faces: usize = bodies.iter().map(|(_, s)| s.faces.len()).sum();
    assert_eq!(f.count("ADVANCED_FACE"), faces);
    assert_eq!(areas.len(), faces);
    let mut expected_areas: Vec<f64> = bodies
        .iter()
        .flat_map(|(_, s)| s.face_ids().map(|id| measure::face_area(s, id)))
        .collect();
    areas.sort_by(f64::total_cmp);
    expected_areas.sort_by(f64::total_cmp);
    for (a, b) in areas.iter().zip(&expected_areas) {
        assert!((a - b).abs() < 1e-3 * b.max(1.0), "face area {a} != {b}");
    }

    // Edges: shared like the kernel's, used twice in opposite directions (manifold), with
    // their vertices on the curve at the right ends.
    let edges: usize = bodies.iter().map(|(_, s)| s.edges.len()).sum();
    let vertices: usize = bodies.iter().map(|(_, s)| s.vertices.len()).sum();
    assert_eq!(f.count("EDGE_CURVE"), edges);
    assert_eq!(f.count("VERTEX_POINT"), vertices);
    let mut uses: HashMap<u32, Vec<bool>> = HashMap::new();
    for o in f.all("ORIENTED_EDGE") {
        let p = f.params(o, "ORIENTED_EDGE");
        uses.entry(r(&p[3])).or_default().push(logical(&p[4]));
        assert_eq!(referenced[&o], 1, "each oriented edge is in one loop");
    }
    for id in f.all("EDGE_CURVE") {
        let u = &uses[&id];
        assert_eq!(u.len(), 2, "edge #{id} is used {} times", u.len());
        assert_ne!(u[0], u[1], "edge #{id} is used twice in the same direction");
        let e = EdgeCurve::read(&f, id);
        assert!(e.same_sense);
        let (a, b) = (f.vertex(e.start), f.vertex(e.end));
        for q in [a, b] {
            let d = e.curve.point(e.curve.param(q)).distance(q);
            assert!(d < 1e-9, "edge #{id}: vertex {d} off its curve");
        }
        if let Curve::Line { dir, .. } = e.curve {
            assert!((b - a).dot(dir) > 0.0, "line edge #{id} runs backwards");
        }
    }
    // Arcs sweep the kernel's angles: compare the total over all conic edges.
    let sweep = |e: &EdgeCurve| {
        if e.start == e.end {
            return TAU;
        }
        let t0 = e.curve.param(f.vertex(e.start));
        (e.curve.param(f.vertex(e.end)) - t0).rem_euclid(TAU)
    };
    let total: f64 = f
        .all("EDGE_CURVE")
        .into_iter()
        .map(|id| EdgeCurve::read(&f, id))
        .filter(|e| matches!(e.curve, Curve::Conic { .. }))
        .map(|e| sweep(&e))
        .sum();
    let expected_total: f64 = bodies
        .iter()
        .flat_map(|(_, s)| &s.edges)
        .filter(|e| !matches!(e.curve, Curve3::Line(_)))
        .map(|e| e.t1 - e.t0)
        .sum();
    assert!(
        (total - expected_total).abs() < 1e-6,
        "{total} != {expected_total}"
    );
    f
}

// ---- Test parts ----

fn regions(s: &Sketch) -> Vec<peet_sketch::region::Region> {
    find_regions(s).regions
}

fn block(plane: &Plane, a: DVec2, b: DVec2, from: f64, to: f64) -> Solid {
    let mut s = Sketch::new();
    shapes::rectangle(&mut s, a, b);
    extrude(plane, &regions(&s), from, to).unwrap()
}

fn plate_with_hole() -> Solid {
    let mut s = Sketch::new();
    shapes::rectangle(&mut s, DVec2::ZERO, DVec2::new(40.0, 20.0));
    s.add_circle(DVec2::new(10.0, 10.0), 3.0);
    let plate: Vec<_> = regions(&s)
        .into_iter()
        .filter(|r| r.holes.len() == 1)
        .collect();
    // On a tilted plane, so nothing lines up with the world axes.
    let plane = Plane {
        frame: Frame {
            origin: DVec3::new(5.0, -3.0, 2.0),
            rotation: DQuat::from_euler(peet_math::EulerRot::XYZ, 0.3, -0.7, 1.9),
        },
    };
    extrude(&plane, &plate, 0.0, 2.0).unwrap()
}

/// A 100 × 60 × 2 plate with a 20 mm edge flange (R3, K 0.44) and a hole in the plate.
fn sheet_metal_part() -> Solid {
    let settings = SheetSettings {
        thickness: 2.0,
        radius: 3.0,
        model: BendModel::KFactor(0.44),
        relief: ReliefType::Rectangular,
        relief_ratio: 0.5,
    };
    let mut s = Sketch::new();
    shapes::rectangle(&mut s, DVec2::ZERO, DVec2::new(100.0, 60.0));
    s.add_circle(DVec2::new(30.0, 30.0), 5.0);
    let plate: Vec<_> = regions(&s)
        .into_iter()
        .filter(|r| r.holes.len() == 1)
        .collect();
    let mut layout = Layout::plate(settings, 1, &Plane::TOP, &plate, false).unwrap();
    let site = EdgeSite {
        piece: 0,
        a: DVec2::new(100.0, 60.0),
        b: DVec2::new(0.0, 60.0),
        top: true,
    };
    let spec = EdgeFlangeSpec {
        length: 20.0,
        angle: 90.0,
        position: FlangePosition::MaterialInside,
        offsets: [10.0, 10.0],
        flip: false,
        radius: None,
    };
    layout.add_edge_flange(2, &site, &spec).unwrap();
    match peet_sheetmetal::build(layout) {
        Ok((solid, _)) => solid,
        Err(e) => panic!("{}", e.message(|o| format!("owner {o}"))),
    }
}

#[test]
fn box_exports() {
    let solid = block(&Plane::TOP, DVec2::ZERO, DVec2::new(40.0, 20.0), 0.0, 5.0);
    let f = export_and_check(&[("Box", &solid)], StepSchema::Ap214);
    assert_eq!(f.count("ADVANCED_FACE"), 6);
    assert_eq!(f.count("PLANE"), 6);
    assert_eq!(f.count("LINE"), 12);
    assert_eq!(f.count("MANIFOLD_SOLID_BREP"), 1);
    assert_eq!(f.count("FACE_BOUND"), 0);
}

#[test]
fn plate_with_round_hole_exports() {
    let solid = plate_with_hole();
    let f = export_and_check(&[("Plate", &solid)], StepSchema::Ap214);
    assert_eq!(f.count("CYLINDRICAL_SURFACE"), 1);
    assert_eq!(f.count("CIRCLE"), 2);
    assert_eq!(f.count("FACE_BOUND"), 2, "the hole in each cap");
    // The hole's wall faces into the hole: its face is against the cylinder's normal.
    let wall = f
        .all("ADVANCED_FACE")
        .into_iter()
        .find(|&id| f.get(r(&f.params(id, "ADVANCED_FACE")[2])).0 == "CYLINDRICAL_SURFACE")
        .unwrap();
    assert!(!logical(&f.params(wall, "ADVANCED_FACE")[3]));
    // The closed circle edges start and end at one vertex.
    let closed = f
        .all("EDGE_CURVE")
        .into_iter()
        .filter(|&id| {
            let p = f.params(id, "EDGE_CURVE");
            p[1] == p[2]
        })
        .count();
    assert_eq!(closed, 2);
}

#[test]
fn sheet_metal_part_exports() {
    let solid = sheet_metal_part();
    let f = export_and_check(&[("Bracket", &solid)], StepSchema::Ap242);
    // The bend's inner (R3) and outer (R5) surfaces and the hole's wall (R5, in pieces).
    let cylinders = solid
        .faces
        .iter()
        .filter(|f| matches!(f.surface, peet_kernel::Surface::Cylinder(_)))
        .count();
    assert!(cylinders >= 3);
    assert_eq!(f.count("CYLINDRICAL_SURFACE"), cylinders);
    let radii: Vec<f64> = f
        .all("CYLINDRICAL_SURFACE")
        .into_iter()
        .map(|id| real(&f.params(id, "CYLINDRICAL_SURFACE")[2]))
        .collect();
    assert!(radii.contains(&3.0) && radii.contains(&5.0), "{radii:?}");
    assert!(f.count("CIRCLE") >= 6, "bend arcs and the hole");
}

#[test]
fn chassis_exports() {
    // The Phase 5 exit part: mitred rims, a hem (a 180° bend), louvers and dimples. The
    // file must be a sound solid in both schemas: every face the right way out, every
    // edge used twice, areas and volume matching the kernel's.
    let (_, engine) = peet_model::samples::chassis();
    let body = &engine.evaluation().bodies[0];
    for schema in [StepSchema::Ap214, StepSchema::Ap242] {
        let f = export_and_check(&[("Chassis", &body.solid)], schema);
        assert_eq!(f.count("ADVANCED_FACE"), body.solid.faces.len());
        assert_eq!(f.count("MANIFOLD_SOLID_BREP"), 1);
    }
    // The flat pattern is a solid too, and exports the same way.
    let sheet = body.sheet.as_ref().unwrap();
    export_and_check(&[("Chassis flat", &sheet.flat)], StepSchema::Ap214);
}

#[test]
fn voids_and_lumps() {
    let outer = block(&Plane::TOP, DVec2::ZERO, DVec2::splat(10.0), 0.0, 10.0);
    let inner = block(&Plane::TOP, DVec2::splat(3.0), DVec2::splat(7.0), 3.0, 7.0);
    let hollow = boolean(&outer, &inner, BooleanOp::Subtract).unwrap();
    assert_eq!(hollow.shells.len(), 2);
    // Two disjoint regions: one body of two lumps.
    let mut s = Sketch::new();
    shapes::rectangle(&mut s, DVec2::ZERO, DVec2::splat(10.0));
    s.add_circle(DVec2::new(30.0, 5.0), 4.0);
    let two = extrude(&Plane::front(), &regions(&s), 0.0, 1.0).unwrap();
    assert_eq!(two.shells.len(), 2);
    let f = export_and_check(&[("Hollow", &hollow), ("Pair", &two)], StepSchema::Ap214);
    assert_eq!(f.count("BREP_WITH_VOIDS"), 1);
    assert_eq!(f.count("ORIENTED_CLOSED_SHELL"), 1);
    assert_eq!(f.count("MANIFOLD_SOLID_BREP"), 2);
    assert_eq!(f.count("CLOSED_SHELL"), 4);
}

#[test]
fn slanted_cut_gives_ellipses() {
    let mut s = Sketch::new();
    s.add_circle(DVec2::ZERO, 5.0);
    let rod = extrude(&Plane::TOP, &regions(&s), 0.0, 20.0).unwrap();
    // A block on a plane tilted 30° about X, cutting the top off the rod.
    let plane = Plane::from_origin_normal_x(
        DVec3::new(0.0, 0.0, 12.0),
        DQuat::from_rotation_x(PI / 6.0) * DVec3::Z,
        DVec3::X,
    )
    .unwrap();
    let cutter = block(&plane, DVec2::splat(-20.0), DVec2::splat(20.0), 0.0, 30.0);
    let cut = boolean(&rod, &cutter, BooleanOp::Subtract).unwrap();
    let f = export_and_check(&[("Rod", &cut)], StepSchema::Ap214);
    assert_eq!(f.count("ELLIPSE"), 1);
}

#[test]
fn names_are_escaped_and_files_are_deterministic() {
    let solid = block(&Plane::TOP, DVec2::ZERO, DVec2::ONE, 0.0, 1.0);
    let mut o = options(StepSchema::Ap214);
    o.product_name = "Bill's Größe".to_owned();
    let a = step::write(&[("Körper", &solid)], &o);
    assert_eq!(a, step::write(&[("Körper", &solid)], &o));
    assert!(a.contains("PRODUCT('Bill''s Gr\\X2\\00F600DF\\X0\\e'"));
    let f = parse(&a);
    let brep = f.all("MANIFOLD_SOLID_BREP")[0];
    assert_eq!(text(&f.params(brep, "MANIFOLD_SOLID_BREP")[0]), "Körper");
    // No bodies: still a valid file with an empty part.
    let empty = parse(&step::write(&[], &o));
    assert_eq!(empty.count("ADVANCED_BREP_SHAPE_REPRESENTATION"), 1);
    let unique: HashSet<u32> = empty.data.keys().copied().collect();
    assert_eq!(unique.len(), empty.data.len());
}
