//! The values operations take: numbers or expressions, features by name or id, and
//! selectors for planes, faces, edges and vertices.
//!
//! Each selector type has two forms. A *query* (`FaceQuery`, …) describes the geometry and
//! is resolved against the part when the operation is applied: this is what a script
//! writes. A *reference* (`FaceRef`, …) is already resolved: this is what the application
//! has after a click. Both are stored the same way.
//!
//! Coordinates are in document units, as they are in a script.

use peet_document::Document;
use peet_model::HoleFit;
use peet_model::{
    AxisRef, EdgeRef, FaceRef, FeatureId, PlaneRef, PointRef, Scalar, ScalarKind, StdAxis,
    StdPlane, VertexRef,
};
use serde_json::{Map, Value, json};

use crate::args::{coordinates, integer, list, number, text};

/// A value as entered: a number in document units (degrees for an angle), or an
/// expression such as `"2 * thickness"` or `"1in"`.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    Number(f64),
    Expr(String),
    /// A plain value in base units (mm, or degrees), exactly as a feature stores it. The
    /// application passes values it already holds this way, so that nothing is lost in
    /// converting to document units and back.
    Base(f64),
}

impl From<f64> for Input {
    fn from(v: f64) -> Self {
        Self::Number(v)
    }
}

impl From<i32> for Input {
    fn from(v: i32) -> Self {
        Self::Number(f64::from(v))
    }
}

impl From<&str> for Input {
    fn from(v: &str) -> Self {
        Self::Expr(v.to_owned())
    }
}

impl From<String> for Input {
    fn from(v: String) -> Self {
        Self::Expr(v)
    }
}

impl Input {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        match v {
            Value::Number(_) => Ok(Self::Number(number(v)?)),
            Value::String(s) => Ok(Self::Expr(s.clone())),
            other => Err(format!("expected a number or an expression, not {other}")),
        }
    }

    /// The value as the text a user would type.
    pub(crate) fn text(&self) -> String {
        match self {
            Self::Number(n) | Self::Base(n) => n.to_string(),
            Self::Expr(e) => e.clone(),
        }
    }

    /// The value as a feature stores it.
    pub(crate) fn scalar(&self, kind: ScalarKind, doc: &Document) -> Result<Scalar, String> {
        let params = &doc.model.parameters;
        match self {
            Self::Number(n) | Self::Base(n) if !n.is_finite() => {
                Err("the value is not a finite number".to_owned())
            }
            Self::Base(n) => Ok(Scalar::new(*n)),
            Self::Number(n) => Ok(Scalar::new(match kind {
                ScalarKind::Length => params.units.to_mm(*n),
                ScalarKind::Angle | ScalarKind::Number => *n,
            })),
            Self::Expr(input) => {
                let mut s = Scalar::new(0.0);
                s.set_input(input, kind, params)?;
                Ok(s)
            }
        }
    }
}

/// A feature, by name (`"Extrude1"`) or id.
#[derive(Clone, Debug, PartialEq)]
pub enum FeatureSel {
    Id(FeatureId),
    Name(String),
}

impl From<FeatureId> for FeatureSel {
    fn from(id: FeatureId) -> Self {
        Self::Id(id)
    }
}

impl From<&str> for FeatureSel {
    fn from(name: &str) -> Self {
        Self::Name(name.to_owned())
    }
}

impl From<String> for FeatureSel {
    fn from(name: String) -> Self {
        Self::Name(name)
    }
}

impl FeatureSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        match v {
            Value::Number(_) => Ok(Self::Id(FeatureId(integer(v)?))),
            Value::String(name) => Ok(Self::Name(name.clone())),
            other => Err(format!("expected a feature's name or id, not {other}")),
        }
    }

    /// The feature this means in `doc`, or why there is none (or more than one).
    pub fn resolve(&self, doc: &Document) -> Result<FeatureId, String> {
        match self {
            Self::Id(id) => doc
                .model
                .feature(*id)
                .map(|f| f.id)
                .ok_or_else(|| format!("There is no feature with the id {}.", id.0)),
            Self::Name(name) => {
                let mut found = doc.model.features().filter(|f| f.name == *name);
                match (found.next(), found.next()) {
                    (Some(f), None) => Ok(f.id),
                    (Some(_), Some(_)) => Err(format!(
                        "Several features are called '{name}': refer to the one you mean by its id."
                    )),
                    _ => {
                        let names: Vec<&str> =
                            doc.model.features().map(|f| f.name.as_str()).collect();
                        Err(format!(
                            "There is no feature called '{name}'. The features are: {}.",
                            if names.is_empty() {
                                "(none)".to_owned()
                            } else {
                                names.join(", ")
                            }
                        ))
                    }
                }
            }
        }
    }
}

/// Which face of a feature: of an extrusion (`Start`, `End`, `Side`) or of sheet metal
/// (`Top`, `Bottom`, `Bend`, `Wall`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Start,
    End,
    Side,
    Top,
    Bottom,
    Bend,
    Wall,
    /// A fillet's or a chamfer's own faces.
    Blend,
    /// The inside of a shell.
    Inner,
}

impl Side {
    pub const ALL: [(Self, &str); 9] = [
        (Self::Blend, "blend"),
        (Self::Inner, "inner"),
        (Self::Start, "start"),
        (Self::End, "end"),
        (Self::Side, "side"),
        (Self::Top, "top"),
        (Self::Bottom, "bottom"),
        (Self::Bend, "bend"),
        (Self::Wall, "wall"),
    ];

    fn parse(v: &Value) -> Result<Self, String> {
        let word = text(v)?;
        Self::ALL
            .iter()
            .find(|(_, w)| *w == word)
            .map(|(s, _)| *s)
            .ok_or_else(|| {
                format!(
                    "'{word}' is not a side. Sides are: start, end, side (extrusions, revolves, sweeps, holes); top, bottom, bend, wall (sheet metal); blend (fillets, chamfers); inner (shells)."
                )
            })
    }

    fn word(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(s, _)| *s == self)
            .map_or("", |(_, w)| *w)
    }
}

fn fields<'a>(
    v: &'a Value,
    what: &str,
    allowed: &[&str],
) -> Result<&'a Map<String, Value>, String> {
    let map = v
        .as_object()
        .ok_or_else(|| format!("expected {what} selector (an object), not {v}"))?;
    match map.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!(
            "{what} selector has no field '{k}'. Its fields are: {}.",
            allowed.join(", ")
        )),
        None if map.is_empty() => Err(format!(
            "{what} selector is empty. Its fields are: {}.",
            allowed.join(", ")
        )),
        None => Ok(map),
    }
}

fn field<T>(
    map: &Map<String, Value>,
    key: &str,
    parse: impl FnOnce(&Value) -> Result<T, String>,
) -> Result<Option<T>, String> {
    map.get(key)
        .map(|v| parse(v).map_err(|e| format!("{key}: {e}")))
        .transpose()
}

fn index(v: &Value) -> Result<usize, String> {
    integer(v).map(|i| i as usize)
}

/// A description of a face. Every field given must match, and exactly one face must.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaceQuery {
    /// A user geometry name assigned with name_face or name_edge.
    pub name: Option<String>,
    /// A point on the face.
    pub at: Option<[f64; 3]>,
    /// Its outward normal (at `at` for a curved face).
    pub normal: Option<[f64; 3]>,
    /// The feature that made it.
    pub feature: Option<FeatureSel>,
    /// Which face of that feature.
    pub side: Option<Side>,
    pub body: Option<usize>,
    /// Its number in the body, from the `faces` query. Only valid until the part changes.
    pub index: Option<u32>,
}

impl FaceQuery {
    fn parse(v: &Value) -> Result<Self, String> {
        let map = fields(
            v,
            "A face",
            &["name", "at", "normal", "feature", "side", "body", "index"],
        )?;
        Ok(Self {
            name: field(map, "name", |v| text(v).map(str::to_owned))?,
            at: field(map, "at", coordinates::<3>)?,
            normal: field(map, "normal", coordinates::<3>)?,
            feature: field(map, "feature", FeatureSel::parse)?,
            side: field(map, "side", Side::parse)?,
            body: field(map, "body", index)?,
            index: field(map, "index", integer)?,
        })
    }

    pub(crate) fn json(&self) -> Value {
        let mut m = Map::new();
        if let Some(n) = &self.name {
            m.insert("name".to_owned(), json!(n));
        }
        if let Some(p) = self.at {
            m.insert("at".to_owned(), json!(p));
        }
        if let Some(n) = self.normal {
            m.insert("normal".to_owned(), json!(n));
        }
        if let Some(f) = &self.feature {
            m.insert("feature".to_owned(), f.json());
        }
        if let Some(s) = self.side {
            m.insert("side".to_owned(), json!(s.word()));
        }
        if let Some(b) = self.body {
            m.insert("body".to_owned(), json!(b));
        }
        if let Some(i) = self.index {
            m.insert("index".to_owned(), json!(i));
        }
        Value::Object(m)
    }
}

impl FeatureSel {
    fn json(&self) -> Value {
        match self {
            Self::Id(id) => json!(id.0),
            Self::Name(n) => json!(n),
        }
    }
}

/// A face: described, or already resolved.
#[derive(Clone, Debug, PartialEq)]
pub enum FaceSel {
    Find(FaceQuery),
    Ref(FaceRef),
}

impl From<FaceQuery> for FaceSel {
    fn from(q: FaceQuery) -> Self {
        Self::Find(q)
    }
}

impl From<FaceRef> for FaceSel {
    fn from(r: FaceRef) -> Self {
        Self::Ref(r)
    }
}

impl FaceSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        FaceQuery::parse(v).map(Self::Find)
    }

    pub(crate) fn json(&self) -> Value {
        match self {
            Self::Find(q) => q.json(),
            Self::Ref(_) => json!("a face reference"),
        }
    }
}

/// A description of an edge. Every field given must match, and exactly one edge must.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdgeQuery {
    /// A user geometry name assigned with name_face or name_edge.
    pub name: Option<String>,
    /// Its two ends, in either order.
    pub between: Option<[[f64; 3]; 2]>,
    /// A point on it.
    pub at: Option<[f64; 3]>,
    /// The two faces it joins.
    pub faces: Option<Box<[FaceSel; 2]>>,
    pub body: Option<usize>,
    /// Its number in the body, from the `edges` query. Only valid until the part changes.
    pub index: Option<u32>,
}

impl EdgeQuery {
    fn parse(v: &Value) -> Result<Self, String> {
        let map = fields(
            v,
            "An edge",
            &["name", "between", "at", "faces", "body", "index"],
        )?;
        Ok(Self {
            name: field(map, "name", |v| text(v).map(str::to_owned))?,
            between: field(map, "between", |b| match list(b)? {
                [p, q] => Ok([coordinates::<3>(p)?, coordinates::<3>(q)?]),
                _ => Err(format!("expected two points, not {b}")),
            })?,
            at: field(map, "at", coordinates::<3>)?,
            faces: field(map, "faces", |f| match list(f)? {
                [a, b] => Ok(Box::new([FaceSel::parse(a)?, FaceSel::parse(b)?])),
                _ => Err(format!("expected two face selectors, not {f}")),
            })?,
            body: field(map, "body", index)?,
            index: field(map, "index", integer)?,
        })
    }

    pub(crate) fn json(&self) -> Value {
        let mut m = Map::new();
        if let Some(n) = &self.name {
            m.insert("name".to_owned(), json!(n));
        }
        if let Some(p) = self.between {
            m.insert("between".to_owned(), json!(p));
        }
        if let Some(p) = self.at {
            m.insert("at".to_owned(), json!(p));
        }
        if let Some(f) = &self.faces {
            m.insert("faces".to_owned(), json!([f[0].json(), f[1].json()]));
        }
        if let Some(b) = self.body {
            m.insert("body".to_owned(), json!(b));
        }
        if let Some(i) = self.index {
            m.insert("index".to_owned(), json!(i));
        }
        Value::Object(m)
    }
}

/// An edge: described, or already resolved.
#[derive(Clone, Debug, PartialEq)]
pub enum EdgeSel {
    Find(EdgeQuery),
    Ref(EdgeRef),
}

impl From<EdgeQuery> for EdgeSel {
    fn from(q: EdgeQuery) -> Self {
        Self::Find(q)
    }
}

impl From<EdgeRef> for EdgeSel {
    fn from(r: EdgeRef) -> Self {
        Self::Ref(r)
    }
}

impl EdgeSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        EdgeQuery::parse(v).map(Self::Find)
    }

    /// The edge between two points (document units).
    pub fn between(a: [f64; 3], b: [f64; 3]) -> Self {
        Self::Find(EdgeQuery {
            between: Some([a, b]),
            ..Default::default()
        })
    }
}

/// A description of a vertex.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VertexQuery {
    pub at: Option<[f64; 3]>,
    pub body: Option<usize>,
    pub index: Option<u32>,
}

impl VertexQuery {
    pub(crate) fn json(&self) -> Value {
        let mut m = Map::new();
        if let Some(p) = self.at {
            m.insert("at".to_owned(), json!(p));
        }
        if let Some(b) = self.body {
            m.insert("body".to_owned(), json!(b));
        }
        if let Some(i) = self.index {
            m.insert("index".to_owned(), json!(i));
        }
        Value::Object(m)
    }
}

/// A vertex: described, or already resolved.
#[derive(Clone, Debug, PartialEq)]
pub enum VertexSel {
    Find(VertexQuery),
    Ref(VertexRef),
}

impl From<VertexRef> for VertexSel {
    fn from(r: VertexRef) -> Self {
        Self::Ref(r)
    }
}

impl VertexSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        let map = fields(v, "A vertex", &["at", "body", "index"])?;
        Ok(Self::Find(VertexQuery {
            at: field(map, "at", coordinates::<3>)?,
            body: field(map, "body", index)?,
            index: field(map, "index", integer)?,
        }))
    }
}

/// Something flat: a standard plane, a reference plane (or coordinate system), or a
/// planar face.
#[derive(Clone, Debug, PartialEq)]
pub enum PlaneSel {
    Standard(StdPlane),
    Feature(FeatureSel),
    Face(FaceSel),
}

impl From<StdPlane> for PlaneSel {
    fn from(p: StdPlane) -> Self {
        Self::Standard(p)
    }
}

impl From<FaceQuery> for PlaneSel {
    fn from(q: FaceQuery) -> Self {
        Self::Face(FaceSel::Find(q))
    }
}

impl From<PlaneRef> for PlaneSel {
    fn from(r: PlaneRef) -> Self {
        match r {
            PlaneRef::Standard(p) => Self::Standard(p),
            PlaneRef::Feature(id) => Self::Feature(FeatureSel::Id(id)),
            PlaneRef::Face(f) => Self::Face(FaceSel::Ref(f)),
        }
    }
}

impl PlaneSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        if let Value::String(s) = v {
            match s.to_lowercase().trim_end_matches(" plane") {
                "front" => return Ok(Self::Standard(StdPlane::Front)),
                "top" => return Ok(Self::Standard(StdPlane::Top)),
                "right" => return Ok(Self::Standard(StdPlane::Right)),
                _ => {}
            }
        }
        if v.is_object() {
            return FaceSel::parse(v).map(Self::Face);
        }
        FeatureSel::parse(v).map(Self::Feature).map_err(|_| {
            format!(
                "expected \"top\", \"front\", \"right\", a reference plane or a face selector, not {v}"
            )
        })
    }
}

/// A direction: a standard axis, a reference axis (or coordinate system), or an edge.
#[derive(Clone, Debug, PartialEq)]
pub enum AxisSel {
    Standard(StdAxis),
    Feature(FeatureSel),
    Edge(EdgeSel),
}

impl From<StdAxis> for AxisSel {
    fn from(a: StdAxis) -> Self {
        Self::Standard(a)
    }
}

impl From<AxisRef> for AxisSel {
    fn from(r: AxisRef) -> Self {
        match r {
            AxisRef::Standard(a) => Self::Standard(a),
            AxisRef::Feature(id) => Self::Feature(FeatureSel::Id(id)),
            AxisRef::Edge(e) => Self::Edge(EdgeSel::Ref(e)),
        }
    }
}

impl AxisSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        if let Value::String(s) = v {
            match s.to_lowercase().trim_end_matches(" axis") {
                "x" => return Ok(Self::Standard(StdAxis::X)),
                "y" => return Ok(Self::Standard(StdAxis::Y)),
                "z" => return Ok(Self::Standard(StdAxis::Z)),
                _ => {}
            }
        }
        if v.is_object() {
            return EdgeSel::parse(v).map(Self::Edge);
        }
        FeatureSel::parse(v).map(Self::Feature).map_err(|_| {
            format!("expected \"x\", \"y\", \"z\", a reference axis or an edge selector, not {v}")
        })
    }
}

/// A position: the origin, a reference point (or coordinate system), or a vertex.
#[derive(Clone, Debug, PartialEq)]
pub enum PointSel {
    Origin,
    Feature(FeatureSel),
    Vertex(VertexSel),
}

impl From<PointRef> for PointSel {
    fn from(r: PointRef) -> Self {
        match r {
            PointRef::Origin => Self::Origin,
            PointRef::Feature(id) => Self::Feature(FeatureSel::Id(id)),
            PointRef::Vertex(v) => Self::Vertex(VertexSel::Ref(v)),
        }
    }
}

impl PointSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        if v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("origin")) {
            return Ok(Self::Origin);
        }
        if v.is_object() {
            return VertexSel::parse(v).map(Self::Vertex);
        }
        FeatureSel::parse(v).map(Self::Feature).map_err(|_| {
            format!("expected \"origin\", a reference point or a vertex selector, not {v}")
        })
    }
}

/// How far an extrusion goes.
#[derive(Clone, Debug, PartialEq)]
pub enum End {
    Blind,
    MidPlane,
    ThroughAll,
    UpTo(PlaneSel),
}

impl End {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        if let Some(m) = v.as_object().filter(|m| m.len() == 1)
            && let Some(up_to) = m.get("up_to")
        {
            return Ok(Self::UpTo(PlaneSel::parse(up_to)?));
        }
        match v.as_str() {
            Some("blind") => Ok(Self::Blind),
            Some("mid_plane") => Ok(Self::MidPlane),
            Some("through_all") => Ok(Self::ThroughAll),
            _ => Err(format!(
                "expected blind, mid_plane, through_all or {{\"up_to\": plane}}, not {v}"
            )),
        }
    }
}

/// Which of a sketch's closed regions a feature uses.
#[derive(Clone, Debug, PartialEq)]
pub enum Regions {
    /// The outer regions and the islands inside holes.
    Auto,
    /// The regions containing these sketch points (document units).
    Points(Vec<[f64; 2]>),
    /// The same, in mm, exactly as a feature stores them.
    PointsMm(Vec<peet_math::DVec2>),
}

impl Regions {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        if v.as_str() == Some("auto") {
            return Ok(Self::Auto);
        }
        list(v)?
            .iter()
            .map(coordinates::<2>)
            .collect::<Result<_, _>>()
            .map(Self::Points)
    }
}

/// How a sheet metal body works out the flat length of its bends.
#[derive(Clone, Debug, PartialEq)]
pub enum Bend {
    KFactor(Input),
    Allowance(Input),
    Deduction(Input),
}

impl Bend {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        let entry = v
            .as_object()
            .filter(|m| m.len() == 1)
            .and_then(|m| m.iter().next());
        let Some((key, value)) = entry else {
            return Err(format!(
                "expected {{\"k_factor\": …}}, {{\"allowance\": …}} or {{\"deduction\": …}}, not {v}"
            ));
        };
        let input = Input::parse(value)?;
        match key.as_str() {
            "k_factor" => Ok(Self::KFactor(input)),
            "allowance" => Ok(Self::Allowance(input)),
            "deduction" => Ok(Self::Deduction(input)),
            other => Err(format!("'{other}' is not k_factor, allowance or deduction")),
        }
    }
}

/// What a revolve turns about.
#[derive(Clone, Debug, PartialEq)]
pub enum RevolveAxis {
    /// The sketch's horizontal axis.
    SketchX,
    /// The sketch's vertical axis.
    SketchY,
    /// A line of the revolve's own sketch, by its entity id: usually a construction line.
    SketchLine(peet_sketch::EntityId),
    /// A standard axis, a reference axis or a straight edge lying in the sketch plane.
    Axis(AxisSel),
}

impl RevolveAxis {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        match v.as_str() {
            Some("sketch_x") => return Ok(Self::SketchX),
            Some("sketch_y") => return Ok(Self::SketchY),
            _ => {}
        }
        if let Some(m) = v.as_object().filter(|m| m.len() == 1)
            && let Some(line) = m.get("line")
        {
            return Ok(Self::SketchLine(peet_sketch::EntityId(integer(line)?)));
        }
        AxisSel::parse(v).map(Self::Axis)
    }
}

/// A standard screw size for a hole (`"M6"`) and how the hole fits it.
#[derive(Clone, Debug, PartialEq)]
pub struct HoleStandard {
    pub size: String,
    pub fit: HoleFit,
}

impl HoleStandard {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        let map = fields(v, "A standard size", &["size", "fit"])?;
        let size = field(map, "size", |s| text(s).map(str::to_owned))?
            .ok_or_else(|| "it needs a 'size', such as \"M6\"".to_owned())?;
        let fit = field(map, "fit", |f| match text(f)? {
            "close" => Ok(HoleFit::Close),
            "normal" => Ok(HoleFit::Normal),
            "loose" => Ok(HoleFit::Loose),
            "tapped" => Ok(HoleFit::Tapped),
            other => Err(format!("'{other}' is not close, normal, loose or tapped")),
        })?
        .unwrap_or(HoleFit::Normal);
        Ok(Self { size, fit })
    }
}

/// A face, an edge or a vertex, for measuring.
#[derive(Clone, Debug, PartialEq)]
pub enum GeomSel {
    Face(FaceSel),
    Edge(EdgeSel),
    Vertex(VertexSel),
}

impl GeomSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        let entry = v
            .as_object()
            .filter(|m| m.len() == 1)
            .and_then(|m| m.iter().next());
        match entry {
            Some((key, sel)) if key == "face" => FaceSel::parse(sel).map(Self::Face),
            Some((key, sel)) if key == "edge" => EdgeSel::parse(sel).map(Self::Edge),
            Some((key, sel)) if key == "vertex" => VertexSel::parse(sel).map(Self::Vertex),
            _ => Err(format!(
                "expected {{\"face\": selector}}, {{\"edge\": selector}} or {{\"vertex\": selector}}, not {v}"
            )),
        }
    }
}

/// The configurations a change applies to: in JSON `"this"` (the active one, and what is
/// meant if the field is left out), `"all"`, a configuration's name, or a list of names.
/// A configuration that happens to be called "this" or "all" is named in a list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Configs {
    /// The active configuration.
    #[default]
    This,
    /// Every configuration: the value no longer differs between them.
    All,
    /// The configurations with these names.
    Named(Vec<String>),
}

impl Configs {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        match v {
            Value::String(s) if s == "this" => Ok(Self::This),
            Value::String(s) if s == "all" => Ok(Self::All),
            Value::String(s) => Ok(Self::Named(vec![s.clone()])),
            Value::Array(names) => names
                .iter()
                .map(|n| {
                    n.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| format!("expected a configuration's name, not {n}"))
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Self::Named),
            other => Err(format!(
                "expected \"this\", \"all\", a configuration's name or a list of names, not {other}"
            )),
        }
    }

    /// The scope this means in `doc`, or which name is not a configuration.
    pub fn resolve(&self, doc: &Document) -> Result<peet_model::Scope, String> {
        Ok(match self {
            Self::This => peet_model::Scope::This,
            Self::All => peet_model::Scope::All,
            Self::Named(names) => peet_model::Scope::Only(
                names
                    .iter()
                    .map(|n| configuration(doc, n))
                    .collect::<Result<_, _>>()?,
            ),
        })
    }
}

/// The configuration called `name`, or which ones there are.
pub(crate) fn configuration(doc: &Document, name: &str) -> Result<peet_model::ConfigId, String> {
    doc.model
        .configuration_named(name)
        .map(|c| c.id)
        .ok_or_else(|| {
            let names: Vec<&str> = doc
                .model
                .configurations()
                .iter()
                .map(|c| c.name.as_str())
                .collect();
            format!(
                "There is no configuration called '{name}'. The configurations are: {}.",
                names.join(", ")
            )
        })
}
