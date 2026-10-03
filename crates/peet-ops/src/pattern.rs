//! Operations on the component patterns of an assembly.
//!
//! A pattern copies components in rows (along one direction or two) or round an axis.
//! The copies are components of their own (listed, counted, weighed, exported), but
//! where they are is the pattern's doing: each follows its original, so mating the
//! original places them all. A direction or an axis is one of the assembly's own, or
//! geometry of a component's part (a straight edge, a round face, a flat face's
//! normal), which the pattern then follows.

use peet_document::Document;
use peet_math::DVec3;
use peet_model::{
    Assembly, ComponentPattern, MAX_PATTERN, MateEnd, MateGeom, Model, PatternId, PatternKind,
    PatternLine, PatternStep, ScalarKind, Status,
};
use serde_json::{Map, Value, json};

use crate::args::{Args, boolean, coordinates, direction_out, integer, point3_out, text};
use crate::assembly::{CompSel, Point3};
use crate::fields::scalar_out;
use crate::mate::MateEndSel;
use crate::op::Op;
use crate::value::Input;

/// A component pattern, by name (`"LPattern1"`) or id.
#[derive(Clone, Debug, PartialEq)]
pub enum PatternSel {
    Id(PatternId),
    Name(String),
}

impl From<PatternId> for PatternSel {
    fn from(id: PatternId) -> Self {
        Self::Id(id)
    }
}

impl From<&str> for PatternSel {
    fn from(name: &str) -> Self {
        Self::Name(name.to_owned())
    }
}

fn the_assembly(doc: &Document) -> Result<&Assembly, String> {
    doc.model
        .assembly()
        .ok_or_else(|| format!("{} is a part: it has no component patterns.", doc.title()))
}

impl PatternSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        match v {
            Value::Number(_) => Ok(Self::Id(PatternId(integer(v)?))),
            Value::String(name) => Ok(Self::Name(name.clone())),
            other => Err(format!("expected a pattern's name or id, not {other}")),
        }
    }

    /// The pattern this means in `doc`, or why there is none.
    pub fn resolve(&self, doc: &Document) -> Result<PatternId, String> {
        let assembly = the_assembly(doc)?;
        let names = || {
            let names: Vec<&str> = assembly.patterns().map(|p| p.name.as_str()).collect();
            if names.is_empty() {
                "The assembly has no component patterns yet: add one with component_pattern."
                    .to_owned()
            } else {
                format!("The patterns are: {}.", names.join(", "))
            }
        };
        match self {
            Self::Id(id) => assembly
                .pattern(*id)
                .map(|p| p.id)
                .ok_or_else(|| format!("There is no pattern with the id {}. {}", id.0, names())),
            Self::Name(name) => assembly
                .patterns()
                .find(|p| p.name == *name)
                .map(|p| p.id)
                .ok_or_else(|| format!("There is no pattern called '{name}'. {}", names())),
        }
    }
}

/// A direction or an axis, as an operation gives it.
#[derive(Clone, Debug, PartialEq)]
pub enum PatternLineSel {
    /// Of the assembly itself: a direction, and for an axis a point on it.
    Fixed { origin: Point3, direction: DVec3 },
    /// Geometry of a component's part: a straight edge, a round face or edge (its
    /// axis), or a flat face (its normal).
    Geom(MateEndSel),
}

impl PatternLineSel {
    fn parse(v: &Value) -> Result<Self, String> {
        let wrong = || {
            format!(
                "expected \"x\", \"y\" or \"z\", a direction [x, y, z], {{\"origin\": [x, y, z], \"direction\": ...}}, or {{\"component\": name, \"edge\" or \"face\": selector}}, not {v}"
            )
        };
        let direction = |v: &Value| -> Result<DVec3, String> {
            match v {
                Value::String(s) => match s.as_str() {
                    "x" => Ok(DVec3::X),
                    "y" => Ok(DVec3::Y),
                    "z" => Ok(DVec3::Z),
                    _ => Err(wrong()),
                },
                Value::Array(_) => coordinates::<3>(v).map(DVec3::from_array),
                _ => Err(wrong()),
            }
        };
        match v {
            Value::Object(map) if map.contains_key("component") => {
                MateEndSel::parse(v).map(Self::Geom)
            }
            Value::Object(map) => {
                let known = map.keys().all(|k| k == "origin" || k == "direction");
                let given = map.get("direction").filter(|_| known).ok_or_else(wrong)?;
                let origin = match map.get("origin") {
                    Some(o) => coordinates::<3>(o)?,
                    None => [0.0; 3],
                };
                Ok(Self::Fixed {
                    origin: Point3::Units(origin),
                    direction: direction(given)?,
                })
            }
            other => Ok(Self::Fixed {
                origin: Point3::Units([0.0; 3]),
                direction: direction(other)?,
            }),
        }
    }

    fn resolve(&self, doc: &Document, what: &str) -> Result<PatternLine, String> {
        match self {
            Self::Fixed { origin, direction } => {
                if !direction.is_finite() || direction.length() < 1e-9 {
                    return Err(format!(
                        "'{what}' has no length: give a direction such as \"x\" or [1, 0, 0]."
                    ));
                }
                Ok(PatternLine::Fixed {
                    origin: origin.mm(&doc.model.parameters.units),
                    direction: direction.normalize(),
                })
            }
            Self::Geom(end) => {
                let end = end.resolve(doc)?;
                match &end.geom {
                    Some(MateGeom::Face(_) | MateGeom::Edge(_)) => Ok(PatternLine::Geom(end)),
                    Some(MateGeom::Vertex(_)) => Err(format!(
                        "'{what}' is a corner, which has no direction: give a straight edge, a round face or edge (its axis), or a flat face (its normal)."
                    )),
                    None => Err(format!(
                        "'{what}' names a component alone: give an edge or a face of it too, as {{\"component\": ..., \"edge\": ...}}."
                    )),
                }
            }
        }
    }
}

/// One direction of a linear pattern, as an operation gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternStepSpec {
    pub direction: PatternLineSel,
    pub spacing: Input,
    pub count: u32,
    pub flip: bool,
}

impl PatternStepSpec {
    fn parse(a: &mut Args) -> Result<Self, String> {
        Ok(Self {
            direction: a.required("direction", PatternLineSel::parse)?,
            spacing: a.required("spacing", Input::parse)?,
            count: a.required("count", integer)?,
            flip: a.flag("flip", false)?,
        })
    }

    fn parse_value(v: &Value) -> Result<Self, String> {
        let Value::Object(map) = v else {
            return Err(format!(
                "expected {{\"direction\": ..., \"spacing\": ..., \"count\": ...}}, not {v}"
            ));
        };
        let mut a = Args::new("second", map.clone());
        let step = Self::parse(&mut a)?;
        a.check()?;
        Ok(step)
    }

    fn resolve(&self, doc: &Document) -> Result<PatternStep, String> {
        Ok(PatternStep {
            direction: self.direction.resolve(doc, "direction")?,
            spacing: self
                .spacing
                .scalar(ScalarKind::Length, doc)
                .map_err(|e| format!("spacing: {e}"))?,
            count: count(self.count)?,
            flip: self.flip,
        })
    }
}

fn count(n: u32) -> Result<u32, String> {
    if (1..=MAX_PATTERN).contains(&n) {
        Ok(n)
    } else {
        Err(format!(
            "'count' is how many there are, the original included: 1 to {MAX_PATTERN}, not {n}."
        ))
    }
}

/// What a new pattern is.
#[derive(Clone, Debug, PartialEq)]
pub enum PatternSpec {
    Linear {
        first: PatternStepSpec,
        second: Option<PatternStepSpec>,
    },
    Circular {
        axis: PatternLineSel,
        /// 360 if absent: evenly spaced all the way round.
        angle: Option<Input>,
        count: u32,
        flip: bool,
    },
    /// As the model has it: what the application passes.
    Exact(PatternKind),
}

impl PatternSpec {
    fn resolve(&self, doc: &Document) -> Result<PatternKind, String> {
        Ok(match self {
            Self::Exact(kind) => kind.clone(),
            Self::Linear { first, second } => PatternKind::Linear {
                first: first.resolve(doc)?,
                second: second
                    .as_ref()
                    .map(|s| s.resolve(doc).map_err(|e| format!("second: {e}")))
                    .transpose()?,
            },
            Self::Circular {
                axis,
                angle,
                count: n,
                flip,
            } => PatternKind::Circular {
                axis: axis.resolve(doc, "axis")?,
                angle: match angle {
                    Some(a) => a
                        .scalar(ScalarKind::Angle, doc)
                        .map_err(|e| format!("angle: {e}"))?,
                    None => peet_model::Scalar::new(360.0),
                },
                count: count(*n)?,
                flip: *flip,
            },
        })
    }
}

/// What of a pattern to change: the fields given.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PatternEdit {
    /// A linear pattern's direction, or a circular one's axis.
    pub direction: Option<PatternLineSel>,
    pub spacing: Option<Input>,
    pub count: Option<u32>,
    pub angle: Option<Input>,
    pub flip: Option<bool>,
    /// A linear pattern's second direction: `Some(None)` takes it away.
    pub second: Option<Option<PatternStepSpec>>,
}

/// What to do to a component pattern.
#[derive(Clone, Debug, PartialEq)]
pub enum PatternChange {
    Edit(PatternEdit),
    /// Make it exactly this: what the application passes.
    Set(PatternKind),
    Rename(String),
    /// Delete it, with the components it made. Its originals stay.
    Delete,
}

impl PatternChange {
    pub(crate) fn word(&self) -> &'static str {
        match self {
            Self::Edit(_) | Self::Set(_) => "edit_component_pattern",
            Self::Rename(_) => "rename",
            Self::Delete => "delete",
        }
    }
}

/// The operations of component patterns, with their fields, for `help`.
pub(crate) const PATTERN_OPS: &[(&str, &str, &str)] = &[
    (
        "component_pattern",
        "components (names or ids: the originals), type (linear, circular), name. Linear: direction (\"x\", \"y\", \"z\", [x, y, z], or {component, edge or face: a selector in the part's own coordinates}), spacing, count (the original included), flip, second ({direction, spacing, count, flip}). Circular: axis (as a direction; or {origin, direction} for an axis of the assembly not through its origin; or {component, face or edge} for a round face's axis), angle (default 360: evenly all the way round), count, flip",
        "Copy components in rows or round an axis. The copies are components of the same part, placed by the pattern: they follow the original, so mate the original and the copies are where they should be.",
    ),
    (
        "edit_component_pattern",
        "pattern, and any of count, spacing, angle, flip, direction, axis, second (null takes a second direction away)",
        "Change a component pattern: copies are added and removed to match.",
    ),
    ("rename", "pattern, name", "Rename a component pattern."),
    (
        "delete",
        "pattern",
        "Delete a component pattern and the copies it made. Its originals stay.",
    ),
];

fn components(v: &Value) -> Result<Vec<CompSel>, String> {
    match v {
        Value::Array(items) => items.iter().map(CompSel::parse).collect(),
        other => Ok(vec![CompSel::parse(other)?]),
    }
}

/// Reads the pattern operation called `op`, if `op` is one for these fields.
pub(crate) fn parse(op: &str, a: &mut Args) -> Option<Result<Op, String>> {
    let change = |a: &mut Args, change: Result<PatternChange, String>| {
        Ok(Op::EditComponentPattern {
            pattern: a.required("pattern", PatternSel::parse)?,
            change: change?,
        })
    };
    Some(match op {
        "component_pattern" => (|| {
            let components = a.required("components", components)?;
            let name = a.string("name")?;
            let kind = match a.required("type", |v| text(v).map(str::to_owned))?.as_str() {
                "linear" => PatternSpec::Linear {
                    first: PatternStepSpec::parse(a)?,
                    second: a
                        .parsed("second", PatternStepSpec::parse_value)
                        .map_err(|e| format!("second: {e}"))?,
                },
                "circular" => PatternSpec::Circular {
                    axis: a.required("axis", PatternLineSel::parse)?,
                    angle: a.parsed("angle", Input::parse)?,
                    count: a.required("count", integer)?,
                    flip: a.flag("flip", false)?,
                },
                other => {
                    return Err(format!(
                        "'{other}' is not a type of component pattern: use \"linear\" or \"circular\"."
                    ));
                }
            };
            Ok(Op::ComponentPattern {
                components,
                kind,
                name,
            })
        })(),
        "edit_component_pattern" => {
            let edit = (|| {
                let direction = a.parsed("direction", PatternLineSel::parse)?;
                let axis = a.parsed("axis", PatternLineSel::parse)?;
                if direction.is_some() && axis.is_some() {
                    return Err(
                        "Give 'direction' (linear) or 'axis' (circular), not both.".to_owned()
                    );
                }
                let second = match a.take_nullable("second") {
                    None => None,
                    Some(Value::Null) => Some(None),
                    Some(v) => Some(Some(
                        PatternStepSpec::parse_value(&v).map_err(|e| format!("second: {e}"))?,
                    )),
                };
                let edit = PatternEdit {
                    direction: direction.or(axis),
                    spacing: a.parsed("spacing", Input::parse)?,
                    count: a.parsed("count", integer)?,
                    angle: a.parsed("angle", Input::parse)?,
                    flip: a.parsed("flip", boolean)?,
                    second,
                };
                if edit == PatternEdit::default() {
                    return Err(
                        "'edit_component_pattern' needs something to change: count, spacing, angle, flip, direction, axis or second."
                            .to_owned(),
                    );
                }
                Ok(PatternChange::Edit(edit))
            })();
            change(a, edit)
        }
        "rename" if a.has("pattern") => {
            let to = a
                .required("name", |v| text(v).map(str::to_owned))
                .map(PatternChange::Rename);
            change(a, to)
        }
        "delete" if a.has("pattern") => change(a, Ok(PatternChange::Delete)),
        _ => return None,
    })
}

fn line_out(doc: &Document, assembly: &Assembly, line: &PatternLine, axis: bool) -> Value {
    match line {
        PatternLine::Fixed { origin, direction } => {
            if axis {
                json!({
                    "origin": point3_out(*origin, &doc.model.parameters.units),
                    "direction": direction_out(*direction),
                })
            } else {
                direction_out(*direction)
            }
        }
        PatternLine::Geom(end) => end_out(assembly, end),
    }
}

fn end_out(assembly: &Assembly, end: &MateEnd) -> Value {
    json!({
        "component": end.component().map(|c| assembly.name_of(c)),
        "on": match &end.geom {
            Some(MateGeom::Face(_)) => "face",
            Some(MateGeom::Edge(_)) => "edge",
            Some(MateGeom::Vertex(_)) => "vertex",
            None => "component",
        },
    })
}

/// A pattern in a few fields: what replies and `components` give.
pub(crate) fn pattern_out(doc: &Document, id: PatternId) -> Value {
    let Some((assembly, pattern)) = doc.model.assembly().and_then(|a| Some((a, a.pattern(id)?)))
    else {
        return json!({ "id": id.0, "status": "deleted" });
    };
    let names = |ids: &mut dyn Iterator<Item = peet_model::CompId>| -> Vec<String> {
        ids.map(|c| assembly.name_of(c).to_owned()).collect()
    };
    let mut m = Map::new();
    m.insert("id".to_owned(), json!(pattern.id.0));
    m.insert("name".to_owned(), json!(pattern.name));
    m.insert("type".to_owned(), json!(pattern.kind.word()));
    m.insert(
        "components".to_owned(),
        json!(names(&mut pattern.seeds.iter().copied())),
    );
    m.insert(
        "copies".to_owned(),
        json!(names(&mut pattern.instances.iter().map(|i| i.component))),
    );
    let step_out = |m: &mut Map<String, Value>, s: &PatternStep| {
        m.insert(
            "direction".to_owned(),
            line_out(doc, assembly, &s.direction, false),
        );
        m.insert(
            "spacing".to_owned(),
            scalar_out(&s.spacing, ScalarKind::Length, doc),
        );
        m.insert("count".to_owned(), json!(s.count));
        if s.flip {
            m.insert("flip".to_owned(), json!(true));
        }
    };
    match &pattern.kind {
        PatternKind::Linear { first, second } => {
            step_out(&mut m, first);
            if let Some(second) = second {
                let mut inner = Map::new();
                step_out(&mut inner, second);
                m.insert("second".to_owned(), Value::Object(inner));
            }
        }
        PatternKind::Circular {
            axis,
            angle,
            count,
            flip,
        } => {
            m.insert("axis".to_owned(), line_out(doc, assembly, axis, true));
            m.insert(
                "angle".to_owned(),
                scalar_out(angle, ScalarKind::Angle, doc),
            );
            m.insert("count".to_owned(), json!(count));
            if *flip {
                m.insert("flip".to_owned(), json!(true));
            }
        }
    }
    let status = doc.evaluation().pattern_status(id);
    m.insert(
        "status".to_owned(),
        json!(match status {
            Some(Status::Failed(_)) => "failed",
            Some(_) => "ok",
            None => "not_built",
        }),
    );
    if let Some(message) = status.and_then(Status::message) {
        m.insert("message".to_owned(), json!(message));
    }
    Value::Object(m)
}

/// The patterns that can't be worked out, for `failures`.
pub(crate) fn failures(doc: &Document) -> Vec<Value> {
    let Some(assembly) = doc.model.assembly() else {
        return Vec::new();
    };
    assembly
        .patterns()
        .filter(|p| {
            matches!(
                doc.evaluation().pattern_status(p.id),
                Some(Status::Failed(_))
            )
        })
        .map(|p| pattern_out(doc, p.id))
        .collect()
}

/// Why a component can't be moved, mated or deleted by itself, if a pattern made it.
pub(crate) fn placed_by_pattern(
    assembly: &Assembly,
    id: peet_model::CompId,
    what: &str,
) -> Result<(), String> {
    match assembly.pattern_of(id) {
        Some(pattern) => Err(format!(
            "{} is a copy made by {}, which also places it, so it can't be {what} by itself: do it to the original ({}), change the pattern with edit_component_pattern, or delete the pattern.",
            assembly.name_of(id),
            pattern.name,
            pattern
                .instances
                .iter()
                .find(|i| i.component == id)
                .map_or("its original", |i| assembly.name_of(i.seed)),
        )),
        None => Ok(()),
    }
}

fn rename(assembly: &mut Assembly, id: PatternId, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A name can't be empty.".to_owned());
    }
    if assembly.patterns().any(|p| p.id != id && p.name == name) {
        return Err(format!("Another pattern is already called '{name}'."));
    }
    if let Some(p) = assembly.pattern_mut(id) {
        name.clone_into(&mut p.name);
    }
    Ok(())
}

/// A pattern must make something.
fn makes_copies(kind: &PatternKind) -> Result<(), String> {
    if kind.places().is_empty() {
        return Err(
            "The pattern would make no copies: 'count' is how many there are with the original, so give at least 2."
                .to_owned(),
        );
    }
    Ok(())
}

/// Adds a pattern. Returns it, with the undo label.
pub(crate) fn add(
    doc: &Document,
    model: &mut Model,
    components: &[CompSel],
    kind: &PatternSpec,
    name: Option<&str>,
) -> Result<(PatternId, String), String> {
    let assembly = the_assembly(doc)?;
    let mut seeds = Vec::new();
    for c in components {
        let id = c.resolve(doc)?;
        placed_by_pattern(assembly, id, "patterned")?;
        if !seeds.contains(&id) {
            seeds.push(id);
        }
    }
    if seeds.is_empty() {
        return Err(
            "'components' is empty: name the components to copy (as 'components' lists them)."
                .to_owned(),
        );
    }
    let kind = kind.resolve(doc)?;
    makes_copies(&kind)?;
    let assembly = model
        .assembly_mut()
        .ok_or_else(|| "The document is not an assembly.".to_owned())?;
    let id = assembly
        .add_pattern(&seeds, kind)
        .ok_or_else(|| "The pattern has no components to copy.".to_owned())?;
    if let Some(name) = name {
        rename(assembly, id, name)?;
    }
    let label = format!(
        "Add {}",
        assembly
            .pattern(id)
            .map_or("Pattern", |p: &ComponentPattern| p.name.as_str())
    );
    Ok((id, label))
}

/// Changes a pattern. Returns the undo label.
pub(crate) fn change(
    doc: &Document,
    model: &mut Model,
    id: PatternId,
    change: &PatternChange,
) -> Result<String, String> {
    let gone = || "The pattern no longer exists.".to_owned();
    let pattern = the_assembly(doc)?.pattern(id).ok_or_else(gone)?;
    let was = pattern.name.clone();
    // Read what is given against the document before anything changes.
    let kind = match change {
        PatternChange::Set(kind) => Some(kind.clone()),
        PatternChange::Edit(edit) => {
            let mut kind = pattern.kind.clone();
            let line = |what: &str| {
                edit.direction
                    .as_ref()
                    .map(|d| d.resolve(doc, what))
                    .transpose()
            };
            match &mut kind {
                PatternKind::Linear { first, second } => {
                    if edit.angle.is_some() {
                        return Err(format!(
                            "{was} is a linear pattern: it has a spacing, not an angle."
                        ));
                    }
                    if let Some(direction) = line("direction")? {
                        first.direction = direction;
                    }
                    if let Some(spacing) = &edit.spacing {
                        first.spacing = spacing
                            .scalar(ScalarKind::Length, doc)
                            .map_err(|e| format!("spacing: {e}"))?;
                    }
                    if let Some(n) = edit.count {
                        first.count = count(n)?;
                    }
                    if let Some(flip) = edit.flip {
                        first.flip = flip;
                    }
                    if let Some(new) = &edit.second {
                        *second = new
                            .as_ref()
                            .map(|s| s.resolve(doc).map_err(|e| format!("second: {e}")))
                            .transpose()?;
                    }
                }
                PatternKind::Circular {
                    axis,
                    angle,
                    count: n,
                    flip,
                } => {
                    if edit.spacing.is_some() || edit.second.is_some() {
                        return Err(format!(
                            "{was} is a circular pattern: it has an angle, not a spacing or a second direction."
                        ));
                    }
                    if let Some(new) = line("axis")? {
                        *axis = new;
                    }
                    if let Some(new) = &edit.angle {
                        *angle = new
                            .scalar(ScalarKind::Angle, doc)
                            .map_err(|e| format!("angle: {e}"))?;
                    }
                    if let Some(new) = edit.count {
                        *n = count(new)?;
                    }
                    if let Some(new) = edit.flip {
                        *flip = new;
                    }
                }
            }
            Some(kind)
        }
        PatternChange::Rename(_) | PatternChange::Delete => None,
    };
    if let Some(kind) = &kind {
        makes_copies(kind)?;
    }
    let assembly = model
        .assembly_mut()
        .ok_or_else(|| "The document is not an assembly.".to_owned())?;
    Ok(match change {
        PatternChange::Edit(_) | PatternChange::Set(_) => {
            if !assembly.set_pattern_kind(id, kind.ok_or_else(gone)?) {
                return Err(gone());
            }
            format!("Edit {was}")
        }
        PatternChange::Rename(name) => {
            rename(assembly, id, name)?;
            format!("Rename {was}")
        }
        PatternChange::Delete => {
            assembly.remove_pattern(id).ok_or_else(gone)?;
            format!("Delete {was}")
        }
    })
}
