//! Drawing in a sketch: the items of a `draw` list.
//!
//! In JSON each item is an object with a `type`: geometry (`point`, `line`, `polyline`,
//! `circle`, `arc`, `rectangle`, `center_rectangle`, `slot`, `polygon`), a relation
//! (`coincident`, `horizontal`, `vertical`, `parallel`, `perpendicular`, `tangent`,
//! `equal`, `concentric`, `midpoint`, `symmetric`, `fix`), a dimension (`distance`,
//! `length`, `horizontal_distance`, `vertical_distance`, `radius`, `diameter`, `angle`) or
//! an edit (`fillet`, `trim`, `extend`, `offset`, `mirror`, `construction`, `delete`).
//!
//! Coordinates are sketch coordinates `[x, y]` in document units. Geometry can be given a
//! label with `as`, and later items refer to entities by label, by id (as replies and the
//! `feature` query give them) or by a part of one: `"a.start"`, `"a.end"`, `"c.center"`,
//! `"r.bottom"`, `"r.top.end"`, `"origin"`.

use std::collections::HashMap;

use peet_document::Document;
use peet_sketch::expr;
use peet_sketch::{ConstraintKind, EntityId, EntityKind, Geometry, Sketch, ops, shapes};
use serde_json::{Map, Value, json};

use crate::args::{Args, boolean, coordinates, integer, list, mm2, number};
use crate::value::Input;

/// A sketch entity: its id, or a path from a label or an id to a part of it
/// (`"a.end"`, `"r.bottom"`, `"12.start"`, `"origin"`).
#[derive(Clone, Debug, PartialEq)]
pub enum Ent {
    Id(EntityId),
    Path(String),
}

impl From<EntityId> for Ent {
    fn from(id: EntityId) -> Self {
        Self::Id(id)
    }
}

impl From<&str> for Ent {
    fn from(path: &str) -> Self {
        Self::Path(path.to_owned())
    }
}

impl Ent {
    fn parse(v: &Value) -> Result<Self, String> {
        match v {
            Value::Number(_) => Ok(Self::Id(EntityId(integer(v)?))),
            Value::String(s) => Ok(Self::Path(s.clone())),
            other => Err(format!("expected an entity's id or label, not {other}")),
        }
    }

    fn parse_list(v: &Value) -> Result<Vec<Self>, String> {
        list(v)?.iter().map(Self::parse).collect()
    }
}

/// A geometric relation between sketch entities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    Coincident,
    Horizontal,
    Vertical,
    Parallel,
    Perpendicular,
    Tangent,
    Equal,
    Concentric,
    Midpoint,
    Symmetric,
    Fix,
}

/// What a dimension measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Measure {
    Distance,
    Length,
    HorizontalDistance,
    VerticalDistance,
    Radius,
    Diameter,
    Angle,
}

const RELATIONS: [(Relation, &str, &str); 11] = [
    (
        Relation::Coincident,
        "coincident",
        "two points, or a point and a curve it lies on",
    ),
    (Relation::Horizontal, "horizontal", "a line, or two points"),
    (Relation::Vertical, "vertical", "a line, or two points"),
    (Relation::Parallel, "parallel", "two lines"),
    (Relation::Perpendicular, "perpendicular", "two lines"),
    (
        Relation::Tangent,
        "tangent",
        "a line and a circle or arc, or two circles or arcs",
    ),
    (
        Relation::Equal,
        "equal",
        "two lines (length) or two circles or arcs (radius)",
    ),
    (Relation::Concentric, "concentric", "two circles or arcs"),
    (Relation::Midpoint, "midpoint", "a point and a line"),
    (
        Relation::Symmetric,
        "symmetric",
        "two points and the line they mirror across",
    ),
    (Relation::Fix, "fix", "an entity, held where it is"),
];

const MEASURES: [(Measure, &str, &str); 7] = [
    (
        Measure::Distance,
        "distance",
        "two points, or a point and a line",
    ),
    (Measure::Length, "length", "a line"),
    (
        Measure::HorizontalDistance,
        "horizontal_distance",
        "two points",
    ),
    (Measure::VerticalDistance, "vertical_distance", "two points"),
    (Measure::Radius, "radius", "a circle or arc"),
    (Measure::Diameter, "diameter", "a circle or arc"),
    (Measure::Angle, "angle", "two lines"),
];

const GEOMETRY: [(&str, &str); 9] = [
    ("point", "at"),
    ("line", "from, to"),
    ("polyline", "points, closed: connected lines"),
    ("circle", "center, radius"),
    (
        "arc",
        "center, start, end: counter-clockwise from start to end",
    ),
    (
        "rectangle",
        "from, to: opposite corners; parts bottom, right, top, left",
    ),
    (
        "center_rectangle",
        "center, corner; parts bottom, right, top, left, center",
    ),
    (
        "slot",
        "from, to, radius: the two arc centres and half the width",
    ),
    ("polygon", "center, vertex, sides"),
];

const EDITS: [(&str, &str); 7] = [
    (
        "fillet",
        "corner, radius: round the corner at a point where two lines meet",
    ),
    (
        "trim",
        "curve, near: remove the piece of the curve nearest the point",
    ),
    (
        "extend",
        "curve, near: extend the end nearest the point to the next curve",
    ),
    (
        "offset",
        "of, distance: parallel copies of connected curves",
    ),
    ("mirror", "of, axis: mirror images across a line"),
    (
        "construction",
        "of, on: make entities construction geometry (or not)",
    ),
    ("delete", "of: remove entities"),
];

/// One thing to draw or do in a sketch. Points are `[x, y]` in the sketch plane and
/// lengths are numbers, both in document units.
#[derive(Clone, Debug, PartialEq)]
pub enum Draw {
    Point {
        at: [f64; 2],
    },
    Line {
        from: [f64; 2],
        to: [f64; 2],
    },
    /// Connected lines through the points.
    Polyline {
        points: Vec<[f64; 2]>,
        closed: bool,
    },
    Circle {
        center: [f64; 2],
        radius: f64,
    },
    /// Counter-clockwise from `start` to `end`.
    Arc {
        center: [f64; 2],
        start: [f64; 2],
        end: [f64; 2],
    },
    /// Parts: `bottom`, `right`, `top`, `left`.
    Rectangle {
        from: [f64; 2],
        to: [f64; 2],
    },
    CenterRectangle {
        center: [f64; 2],
        corner: [f64; 2],
    },
    /// `from` and `to` are the arc centres; `radius` is half the width.
    Slot {
        from: [f64; 2],
        to: [f64; 2],
        radius: f64,
    },
    Polygon {
        center: [f64; 2],
        vertex: [f64; 2],
        sides: u32,
    },
    Relation {
        kind: Relation,
        of: Vec<Ent>,
    },
    Dimension {
        kind: Measure,
        of: Vec<Ent>,
        /// The measured value if absent.
        value: Option<Input>,
        /// An automatic name (`d1`) if absent.
        name: Option<String>,
        /// A reference dimension: it reports, and doesn't drive.
        driven: bool,
    },
    Fillet {
        corner: Ent,
        radius: f64,
    },
    Trim {
        curve: Ent,
        near: [f64; 2],
    },
    Extend {
        curve: Ent,
        near: [f64; 2],
    },
    Offset {
        of: Vec<Ent>,
        distance: f64,
    },
    Mirror {
        of: Vec<Ent>,
        axis: Ent,
    },
    Construction {
        of: Vec<Ent>,
        on: bool,
    },
    Delete {
        of: Vec<Ent>,
    },
}

/// A [`Draw`] with what geometry can also have: a label for later items of the same list,
/// and whether it is construction geometry.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawItem {
    pub draw: Draw,
    pub label: Option<String>,
    pub construction: bool,
}

impl From<Draw> for DrawItem {
    fn from(draw: Draw) -> Self {
        Self {
            draw,
            label: None,
            construction: false,
        }
    }
}

impl Draw {
    /// The item with a label, for later items of the same list to refer to.
    pub fn labelled(self, label: &str) -> DrawItem {
        DrawItem {
            label: Some(label.to_owned()),
            ..self.into()
        }
    }

    fn word(&self) -> &'static str {
        match self {
            Self::Point { .. } => "point",
            Self::Line { .. } => "line",
            Self::Polyline { .. } => "polyline",
            Self::Circle { .. } => "circle",
            Self::Arc { .. } => "arc",
            Self::Rectangle { .. } => "rectangle",
            Self::CenterRectangle { .. } => "center_rectangle",
            Self::Slot { .. } => "slot",
            Self::Polygon { .. } => "polygon",
            Self::Relation { kind, .. } => RELATIONS
                .iter()
                .find(|(k, ..)| k == kind)
                .map_or("", |(_, w, _)| *w),
            Self::Dimension { kind, .. } => MEASURES
                .iter()
                .find(|(k, ..)| k == kind)
                .map_or("", |(_, w, _)| *w),
            Self::Fillet { .. } => "fillet",
            Self::Trim { .. } => "trim",
            Self::Extend { .. } => "extend",
            Self::Offset { .. } => "offset",
            Self::Mirror { .. } => "mirror",
            Self::Construction { .. } => "construction",
            Self::Delete { .. } => "delete",
        }
    }
}

impl DrawItem {
    /// The item as construction geometry.
    pub fn construction(mut self) -> Self {
        self.construction = true;
        self
    }

    fn parse(kind: &str, a: &mut Args) -> Result<Self, String> {
        let p2 = coordinates::<2>;
        let label = a.string("as")?;
        let construction = a.flag("construction", false)?;
        let draw = if let Some((relation, ..)) = RELATIONS.iter().find(|(_, w, _)| *w == kind) {
            Draw::Relation {
                kind: *relation,
                of: a.required("of", Ent::parse_list)?,
            }
        } else if let Some((measure, ..)) = MEASURES.iter().find(|(_, w, _)| *w == kind) {
            Draw::Dimension {
                kind: *measure,
                of: a.required("of", Ent::parse_list)?,
                value: a.parsed("value", Input::parse)?,
                name: a.string("name")?,
                driven: a.flag("driven", false)?,
            }
        } else {
            match kind {
                "point" => Draw::Point {
                    at: a.required("at", p2)?,
                },
                "line" => Draw::Line {
                    from: a.required("from", p2)?,
                    to: a.required("to", p2)?,
                },
                "polyline" => Draw::Polyline {
                    points: a.required("points", |v| list(v)?.iter().map(p2).collect())?,
                    closed: a.flag("closed", false)?,
                },
                "circle" => Draw::Circle {
                    center: a.required("center", p2)?,
                    radius: a.required("radius", number)?,
                },
                "arc" => Draw::Arc {
                    center: a.required("center", p2)?,
                    start: a.required("start", p2)?,
                    end: a.required("end", p2)?,
                },
                "rectangle" => Draw::Rectangle {
                    from: a.required("from", p2)?,
                    to: a.required("to", p2)?,
                },
                "center_rectangle" => Draw::CenterRectangle {
                    center: a.required("center", p2)?,
                    corner: a.required("corner", p2)?,
                },
                "slot" => Draw::Slot {
                    from: a.required("from", p2)?,
                    to: a.required("to", p2)?,
                    radius: a.required("radius", number)?,
                },
                "polygon" => Draw::Polygon {
                    center: a.required("center", p2)?,
                    vertex: a.required("vertex", p2)?,
                    sides: a.required("sides", integer)?,
                },
                "fillet" => Draw::Fillet {
                    corner: a.required("corner", Ent::parse)?,
                    radius: a.required("radius", number)?,
                },
                "trim" => Draw::Trim {
                    curve: a.required("curve", Ent::parse)?,
                    near: a.required("near", p2)?,
                },
                "extend" => Draw::Extend {
                    curve: a.required("curve", Ent::parse)?,
                    near: a.required("near", p2)?,
                },
                "offset" => Draw::Offset {
                    of: a.required("of", Ent::parse_list)?,
                    distance: a.required("distance", number)?,
                },
                "mirror" => Draw::Mirror {
                    of: a.required("of", Ent::parse_list)?,
                    axis: a.required("axis", Ent::parse)?,
                },
                "construction" => Draw::Construction {
                    of: a.required("of", Ent::parse_list)?,
                    on: a.parsed("on", boolean)?.unwrap_or(true),
                },
                "delete" => Draw::Delete {
                    of: a.required("of", Ent::parse_list)?,
                },
                other => {
                    let all: Vec<&str> = GEOMETRY
                        .iter()
                        .map(|(k, _)| *k)
                        .chain(RELATIONS.iter().map(|(_, w, _)| *w))
                        .chain(MEASURES.iter().map(|(_, w, _)| *w))
                        .chain(EDITS.iter().map(|(k, _)| *k))
                        .collect();
                    return Err(format!(
                        "'{other}' is not something to draw. Types are: {}.",
                        all.join(", ")
                    ));
                }
            }
        };
        Ok(Self {
            draw,
            label,
            construction,
        })
    }
}

/// Reads a JSON `draw` list.
pub(crate) fn parse(items: &Value) -> Result<Vec<DrawItem>, String> {
    let mut out = Vec::new();
    for (i, item) in list(items)?.iter().enumerate() {
        let at = |e: String| format!("draw item {} ({item}): {e}", i + 1);
        let Value::Object(map) = item else {
            return Err(at("expected an object with a 'type'".to_owned()));
        };
        let mut map = map.clone();
        let kind = match map.remove("type") {
            Some(Value::String(k)) => k,
            _ => return Err(at("it needs a 'type'".to_owned())),
        };
        let mut a = Args::new(&kind, map);
        out.push(DrawItem::parse(&kind, &mut a).map_err(at)?);
        a.check().map_err(at)?;
    }
    Ok(out)
}

const RECTANGLE: &[&str] = &["bottom", "right", "top", "left"];

/// Something a label stands for.
enum Named {
    Entity(EntityId),
    Shape {
        curves: Vec<EntityId>,
        points: Vec<EntityId>,
        /// Names of the first curves ("bottom", …), where the shape has them.
        names: &'static [&'static str],
    },
}

fn shape(s: shapes::Shape, names: &'static [&'static str]) -> Named {
    Named::Shape {
        curves: s.curves,
        points: s.points,
        names,
    }
}

fn resolve(
    sketch: &Sketch,
    labels: &HashMap<String, Named>,
    ent: &Ent,
) -> Result<EntityId, String> {
    let exists = |id: EntityId| match sketch.entity(id) {
        Some(_) => Ok(id),
        None => Err(format!("the sketch has no entity {}", id.0)),
    };
    let path = match ent {
        Ent::Id(id) => return exists(*id),
        Ent::Path(p) => p.as_str(),
    };
    let mut parts = path.split('.');
    let first = parts.next().unwrap_or_default();
    let mut current = if first == "origin" {
        Sketch::ORIGIN
    } else if let Ok(n) = first.parse::<u32>() {
        exists(EntityId(n))?
    } else {
        match labels.get(first) {
            None => {
                return Err(format!(
                    "nothing is labelled '{first}' in this draw list (labels last for one operation: afterwards, use ids)"
                ));
            }
            Some(Named::Entity(id)) => *id,
            Some(Named::Shape {
                curves,
                points,
                names,
            }) => {
                let Some(part) = parts.next() else {
                    return Err(format!(
                        "'{first}' is a shape: say which part, such as '{first}.{}'",
                        names.first().copied().unwrap_or("0")
                    ));
                };
                let found = match part {
                    "center" => points.first().copied(),
                    _ => names
                        .iter()
                        .position(|n| *n == part)
                        .or_else(|| part.parse::<usize>().ok())
                        .and_then(|i| curves.get(i).copied()),
                };
                found.ok_or_else(|| format!("'{first}' has no part '{part}'"))?
            }
        }
    };
    for part in parts {
        let next = match part {
            "start" => sketch.endpoints(current).map(|e| e.0),
            "end" => sketch.endpoints(current).map(|e| e.1),
            "center" => sketch.center(current),
            _ => None,
        };
        current = next.ok_or_else(|| {
            format!(
                "'{path}': entity {} has no '{part}' (parts are start, end and center)",
                current.0
            )
        })?;
    }
    Ok(current)
}

fn resolve_all(
    sketch: &Sketch,
    labels: &HashMap<String, Named>,
    of: &[Ent],
) -> Result<Vec<EntityId>, String> {
    of.iter().map(|e| resolve(sketch, labels, e)).collect()
}

fn entity_out(sketch: &Sketch, id: EntityId) -> Value {
    let Some(e) = sketch.entity(id) else {
        return json!({ "id": id.0 });
    };
    match e.geometry {
        Geometry::Point { .. } => json!({ "id": id.0, "type": "point" }),
        Geometry::Line { start, end } => {
            json!({ "id": id.0, "type": "line", "start": start.0, "end": end.0 })
        }
        Geometry::Circle { center, .. } => {
            json!({ "id": id.0, "type": "circle", "center": center.0 })
        }
        Geometry::Arc { center, start, end } => {
            json!({ "id": id.0, "type": "arc", "center": center.0, "start": start.0, "end": end.0 })
        }
    }
}

fn ids(list: &[EntityId]) -> Value {
    list.iter().map(|e| e.0).collect()
}

fn relation(sketch: &Sketch, kind: Relation, of: &[EntityId]) -> Result<ConstraintKind, String> {
    use ConstraintKind as K;
    let is_point = |id: EntityId| sketch.kind(id) == Some(EntityKind::Point);
    let wrong = |takes: &str| Err(format!("it takes {takes} in 'of'"));
    Ok(match (kind, of) {
        (Relation::Coincident, &[a, b]) => match (is_point(a), is_point(b)) {
            (true, true) => K::Coincident(a, b),
            (true, false) => K::PointOnCurve { point: a, curve: b },
            (false, true) => K::PointOnCurve { point: b, curve: a },
            (false, false) => return wrong("two points, or a point and a curve"),
        },
        (Relation::Coincident, _) => return wrong("two points, or a point and a curve"),
        (Relation::Horizontal, &[a]) => K::Horizontal(a),
        (Relation::Horizontal, &[a, b]) => K::HorizontalPoints(a, b),
        (Relation::Vertical, &[a]) => K::Vertical(a),
        (Relation::Vertical, &[a, b]) => K::VerticalPoints(a, b),
        (Relation::Horizontal | Relation::Vertical, _) => return wrong("a line, or two points"),
        (Relation::Parallel, &[a, b]) => K::Parallel(a, b),
        (Relation::Perpendicular, &[a, b]) => K::Perpendicular(a, b),
        (Relation::Tangent, &[a, b]) => K::Tangent(a, b),
        (Relation::Equal, &[a, b]) => K::Equal(a, b),
        (Relation::Concentric, &[a, b]) => K::Concentric(a, b),
        (Relation::Midpoint, &[a, b]) if is_point(a) => K::Midpoint { point: a, line: b },
        (Relation::Midpoint, &[a, b]) => K::Midpoint { point: b, line: a },
        (Relation::Symmetric, &[a, b, axis]) => K::Symmetric { a, b, axis },
        (Relation::Symmetric, _) => return wrong("two points and a line"),
        (Relation::Fix, _) => return wrong("one entity"),
        _ => return wrong("two entities"),
    })
}

fn measure(kind: Measure, of: &[EntityId]) -> Result<ConstraintKind, String> {
    use ConstraintKind as K;
    Ok(match (kind, of) {
        (Measure::Distance, &[a, b]) => K::Distance(a, b),
        (Measure::Length, &[a]) => K::Length(a),
        (Measure::HorizontalDistance, &[a, b]) => K::HorizontalDistance(a, b),
        (Measure::VerticalDistance, &[a, b]) => K::VerticalDistance(a, b),
        (Measure::Radius, &[a]) => K::Radius(a),
        (Measure::Diameter, &[a]) => K::Diameter(a),
        (Measure::Angle, &[a, b]) => K::Angle(a, b),
        (Measure::Length | Measure::Radius | Measure::Diameter, _) => {
            return Err("it takes one entity in 'of'".to_owned());
        }
        _ => return Err("it takes two entities in 'of'".to_owned()),
    })
}

/// Sets a dimension's value from a number (document units, or degrees) or an expression.
pub(crate) fn set_dimension(
    sketch: &mut Sketch,
    doc: &Document,
    name: &str,
    value: &Input,
) -> Result<(), String> {
    let id = sketch.dimension_by_name(name).ok_or_else(|| {
        let names: Vec<String> = sketch
            .constraints()
            .filter_map(|(_, c)| c.dimension.as_ref().map(|d| d.name.clone()))
            .collect();
        format!(
            "The sketch has no dimension '{name}'. Its dimensions are: {}.",
            if names.is_empty() {
                "(none)".to_owned()
            } else {
                names.join(", ")
            }
        )
    })?;
    expr::set_dimension_input(sketch, &doc.model.parameters, id, &value.text())
        .map(|_| ())
        .map_err(|e| format!("{name}: {}", e.message))
}

fn one(
    sketch: &mut Sketch,
    labels: &mut HashMap<String, Named>,
    doc: &Document,
    item: &DrawItem,
) -> Result<Value, String> {
    let units = &doc.model.parameters.units;
    let p = |p: &[f64; 2]| mm2(*p, units);
    let len = |v: &f64| units.to_mm(*v);
    let word = item.draw.word();
    let mut reply = Map::new();
    reply.insert("type".to_owned(), json!(word));
    let failed = |e: ops::OpError| e.to_string();

    let made: Option<Named> = match &item.draw {
        Draw::Point { at } => Some(Named::Entity(sketch.add_point(p(at)))),
        Draw::Line { from, to } => Some(Named::Entity(sketch.add_line(p(from), p(to)))),
        Draw::Circle { center, radius } => {
            Some(Named::Entity(sketch.add_circle(p(center), len(radius))))
        }
        Draw::Arc { center, start, end } => {
            Some(Named::Entity(sketch.add_arc(p(center), p(start), p(end))))
        }
        Draw::Polyline { points, closed } => {
            if points.len() < 2 || (*closed && points.len() < 3) {
                return Err("a polyline needs at least two points (three if closed)".to_owned());
            }
            let n = if *closed {
                points.len()
            } else {
                points.len() - 1
            };
            let curves: Vec<EntityId> = (0..n)
                .map(|i| sketch.add_line(p(&points[i]), p(&points[(i + 1) % points.len()])))
                .collect();
            for i in 0..curves.len() {
                let next = (i + 1) % curves.len();
                if next == 0 && !closed {
                    break;
                }
                let end = sketch.endpoints(curves[i]).map(|e| e.1);
                let start = sketch.endpoints(curves[next]).map(|e| e.0);
                if let (Some(end), Some(start)) = (end, start) {
                    sketch
                        .add_constraint(ConstraintKind::Coincident(end, start))
                        .map_err(|e| e.to_string())?;
                }
            }
            Some(Named::Shape {
                curves,
                points: Vec::new(),
                names: &[],
            })
        }
        Draw::Rectangle { from, to } => {
            Some(shape(shapes::rectangle(sketch, p(from), p(to)), RECTANGLE))
        }
        Draw::CenterRectangle { center, corner } => Some(shape(
            shapes::center_rectangle(sketch, p(center), p(corner)),
            RECTANGLE,
        )),
        Draw::Slot { from, to, radius } => Some(shape(
            shapes::slot(sketch, p(from), p(to), len(radius)),
            &[],
        )),
        Draw::Polygon {
            center,
            vertex,
            sides,
        } => Some(shape(
            shapes::polygon(sketch, p(center), p(vertex), *sides as usize),
            &[],
        )),
        Draw::Relation { kind, of } => {
            let of = resolve_all(sketch, labels, of)?;
            let made: Vec<u32> = if *kind == Relation::Fix {
                let [target] = of[..] else {
                    return Err("it takes one entity in 'of'".to_owned());
                };
                sketch
                    .fix(target)
                    .map_err(|e| e.to_string())?
                    .iter()
                    .map(|c| c.0)
                    .collect()
            } else {
                let k = relation(sketch, *kind, &of)?;
                vec![sketch.add_constraint(k).map_err(|e| e.to_string())?.0]
            };
            reply.insert("relations".to_owned(), json!(made));
            None
        }
        Draw::Dimension {
            kind,
            of,
            value,
            name,
            driven,
        } => {
            let of = resolve_all(sketch, labels, of)?;
            let k = measure(*kind, &of)?;
            let measured = sketch.measure(&k).unwrap_or(0.0);
            let id = sketch
                .add_dimension(k, measured)
                .map_err(|e| e.to_string())?;
            if let Some(name) = name {
                expr::check_name(name).map_err(|e| e.message)?;
                if sketch.dimension_by_name(name).is_some() {
                    return Err(format!("the sketch already has a dimension '{name}'"));
                }
            }
            let mut given = String::new();
            if let Some(d) = sketch.constraint_mut(id).and_then(|c| c.dimension.as_mut()) {
                if let Some(name) = name {
                    d.name.clone_from(name);
                }
                d.driving = !driven;
                given.clone_from(&d.name);
            }
            if let Some(value) = value {
                set_dimension(sketch, doc, &given, value)?;
            }
            reply.insert("dimension".to_owned(), json!(given));
            reply.insert("relation".to_owned(), json!(id.0));
            None
        }
        Draw::Fillet { corner, radius } => {
            let corner = resolve(sketch, labels, corner)?;
            let arc = ops::fillet(sketch, corner, len(radius)).map_err(failed)?;
            reply.insert("arc".to_owned(), entity_out(sketch, arc));
            return finish(item, reply, labels, Some(Named::Entity(arc)), false);
        }
        Draw::Trim { curve, near } => {
            let curve = resolve(sketch, labels, curve)?;
            let left = ops::trim(sketch, curve, p(near)).map_err(failed)?;
            reply.insert("curves".to_owned(), ids(&left));
            None
        }
        Draw::Extend { curve, near } => {
            let curve = resolve(sketch, labels, curve)?;
            ops::extend(sketch, curve, p(near)).map_err(failed)?;
            None
        }
        Draw::Offset { of, distance } => {
            let of = resolve_all(sketch, labels, of)?;
            let curves = ops::offset(sketch, &of, len(distance)).map_err(failed)?;
            reply.insert("curves".to_owned(), ids(&curves));
            let named = Named::Shape {
                curves,
                points: Vec::new(),
                names: &[],
            };
            return finish(item, reply, labels, Some(named), false);
        }
        Draw::Mirror { of, axis } => {
            let of = resolve_all(sketch, labels, of)?;
            let axis = resolve(sketch, labels, axis)?;
            let copies = ops::mirror(sketch, &of, axis).map_err(failed)?;
            reply.insert("copies".to_owned(), ids(&copies));
            let named = Named::Shape {
                curves: copies,
                points: Vec::new(),
                names: &[],
            };
            return finish(item, reply, labels, Some(named), false);
        }
        Draw::Construction { of, on } => {
            for id in resolve_all(sketch, labels, of)? {
                sketch.set_construction(id, *on);
            }
            None
        }
        Draw::Delete { of } => {
            let mut removed = Vec::new();
            for id in resolve_all(sketch, labels, of)? {
                if sketch.entity(id).is_some() {
                    removed.extend(sketch.remove_entity(id).map_err(|e| e.to_string())?);
                }
            }
            reply.insert("removed".to_owned(), ids(&removed));
            None
        }
    };
    // New geometry: say what it is made of, and make it construction if asked.
    match &made {
        Some(Named::Entity(id)) => {
            if item.construction {
                sketch.set_construction(*id, true);
            }
            if let Value::Object(m) = entity_out(sketch, *id) {
                reply.extend(m);
            }
        }
        Some(Named::Shape { curves, points, .. }) => {
            if item.construction {
                for c in curves {
                    sketch.set_construction(*c, true);
                }
            }
            reply.insert(
                "curves".to_owned(),
                curves.iter().map(|c| entity_out(sketch, *c)).collect(),
            );
            if !points.is_empty() {
                reply.insert("points".to_owned(), ids(points));
            }
        }
        None => {}
    }
    let geometry = made.is_some();
    finish(item, reply, labels, made, !geometry)
}

/// Records the item's label, which only things that made geometry can have.
fn finish(
    item: &DrawItem,
    mut reply: Map<String, Value>,
    labels: &mut HashMap<String, Named>,
    made: Option<Named>,
    no_construction: bool,
) -> Result<Value, String> {
    if item.construction && no_construction {
        return Err(format!(
            "'construction' is for geometry: a {} can't have it",
            item.draw.word()
        ));
    }
    if let Some(label) = &item.label {
        match made {
            Some(named) => {
                reply.insert("as".to_owned(), json!(label));
                labels.insert(label.clone(), named);
            }
            None => {
                return Err(format!(
                    "'as' labels geometry: a {} can't have one",
                    item.draw.word()
                ));
            }
        }
    }
    Ok(Value::Object(reply))
}

/// Applies a draw list to a sketch. Returns what each item made, in order.
pub(crate) fn draw(
    sketch: &mut Sketch,
    items: &[DrawItem],
    doc: &Document,
) -> Result<Vec<Value>, String> {
    let mut labels = HashMap::new();
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let reply = one(sketch, &mut labels, doc, item)
            .map_err(|e| format!("draw item {} ({}): {e}", i + 1, item.draw.word()))?;
        out.push(reply);
    }
    Ok(out)
}

/// The kinds of draw item, for `help`.
pub(crate) fn help() -> Value {
    let table = |rows: &mut dyn Iterator<Item = (&str, &str)>| -> Value {
        rows.map(|(k, v)| (k.to_owned(), json!(v)))
            .collect::<Map<String, Value>>()
            .into()
    };
    json!({
        "geometry": table(&mut GEOMETRY.iter().copied()),
        "relations (of: [entities])": table(&mut RELATIONS.iter().map(|(_, k, v)| (*k, *v))),
        "dimensions (of: [entities], value, name, driven)":
            table(&mut MEASURES.iter().map(|(_, k, v)| (*k, *v))),
        "edits": table(&mut EDITS.iter().copied()),
        "entities": "an id, a label given with 'as', or a part: a.start, a.end, c.center, r.bottom, origin",
        "coordinates": "[x, y] in the sketch plane, in document units",
    })
}
