//! Operations on the mates of an assembly.
//!
//! A mate's end is a component path and geometry of its part: a face, an edge, a vertex
//! or a reference plane,
//! described by the same selectors a part's operations take, in the part's own
//! coordinates (not where the component is in the assembly). A description is resolved
//! against the part as it is now and stored as a persistent reference, so the mate
//! follows its face through edits to the part.

use peet_document::Document;
use peet_math::Frame;
use peet_model::{
    Assembly, CompId, Mate, MateEnd, MateGeom, MateId, MateKind, Model, ScalarKind, Status,
    WidthMode,
};
use serde_json::{Map, Value, json};

use crate::args::{Args, boolean, integer, text};
use crate::assembly::CompSel;
use crate::fields::scalar_out;
use crate::op::Op;
use crate::select;
use crate::value::{GeomSel, Input, PlaneSel};

/// A mate, by name (`"Coincident1"`) or id.
#[derive(Clone, Debug, PartialEq)]
pub enum MateSel {
    Id(MateId),
    Name(String),
}

impl From<MateId> for MateSel {
    fn from(id: MateId) -> Self {
        Self::Id(id)
    }
}

impl From<&str> for MateSel {
    fn from(name: &str) -> Self {
        Self::Name(name.to_owned())
    }
}

fn the_assembly(doc: &Document) -> Result<&Assembly, String> {
    doc.model
        .assembly()
        .ok_or_else(|| format!("{} is a part: it has no mates.", doc.title()))
}

impl MateSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        match v {
            Value::Number(_) => Ok(Self::Id(MateId(integer(v)?))),
            Value::String(name) => Ok(Self::Name(name.clone())),
            other => Err(format!("expected a mate's name or id, not {other}")),
        }
    }

    /// The mate this means in `doc`, or why there is none.
    pub fn resolve(&self, doc: &Document) -> Result<MateId, String> {
        let assembly = the_assembly(doc)?;
        let names = || {
            let names: Vec<&str> = assembly.mates().map(|m| m.name.as_str()).collect();
            if names.is_empty() {
                "The assembly has no mates yet: add one with mate.".to_owned()
            } else {
                format!("The mates are: {}.", names.join(", "))
            }
        };
        match self {
            Self::Id(id) => assembly
                .mate(*id)
                .map(|m| m.id)
                .ok_or_else(|| format!("There is no mate with the id {}. {}", id.0, names())),
            Self::Name(name) => assembly
                .mates()
                .find(|m| m.name == *name)
                .map(|m| m.id)
                .ok_or_else(|| format!("There is no mate called '{name}'. {}", names())),
        }
    }
}

/// One end of a mate.
#[derive(Clone, Debug, PartialEq)]
pub enum MateEndSel {
    /// As a script writes it: a component path, and geometry or a plane of its part
    /// (none for a fasten mate, which holds the component as a whole).
    Find {
        component: Vec<CompSel>,
        geom: Option<GeomSel>,
        plane: Option<PlaneSel>,
    },
    /// Already resolved: what the application passes after a click.
    Ref(MateEnd),
}

impl MateEndSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        let wrong = || {
            format!(
                "expected {{\"component\": name or [names or ids along a path], \"face\": selector}} (or \"edge\", \"vertex\", \"plane\"; or the component alone, for fasten), not {v}"
            )
        };
        let mut map = match v {
            Value::Object(map) => map.clone(),
            // The component alone.
            Value::String(_) | Value::Number(_) => {
                return Ok(Self::Find {
                    component: vec![CompSel::parse(v)?],
                    geom: None,
                    plane: None,
                });
            }
            _ => return Err(wrong()),
        };
        let value = map.remove("component").ok_or_else(wrong)?;
        let component = match &value {
            Value::Array(path) if !path.is_empty() => path
                .iter()
                .map(CompSel::parse)
                .collect::<Result<Vec<_>, _>>()?,
            Value::Array(_) => return Err("A component path can't be empty.".to_owned()),
            _ => vec![CompSel::parse(&value)?],
        };
        let plane = map
            .remove("plane")
            .map(|v| PlaneSel::parse(&v))
            .transpose()?;
        if plane.is_some() && !map.is_empty() {
            return Err(wrong());
        }
        let geom = match map.len() {
            0 => None,
            1 => Some(GeomSel::parse(&Value::Object(map))?),
            _ => return Err(wrong()),
        };
        Ok(Self::Find {
            component,
            geom,
            plane,
        })
    }

    /// The end as a mate stores it, with the component it is on.
    pub(crate) fn resolve(&self, doc: &Document) -> Result<MateEnd, String> {
        let (component, geom, plane) = match self {
            Self::Ref(end) => return Ok(end.clone()),
            Self::Find {
                component,
                geom,
                plane,
            } => (component, geom, plane),
        };
        let mut assembly = the_assembly(doc)?;
        let mut path = Vec::new();
        let mut names = Vec::new();
        let mut definition = None;
        for (index, sel) in component.iter().enumerate() {
            let id = sel
                .resolve_in(assembly)
                .map_err(|e| format!("In component path {}: {e}", names.join(" / ")))?;
            names.push(assembly.name_of(id));
            path.push(id);
            let def = assembly
                .definition_of(id)
                .ok_or_else(|| "The component's part is missing.".to_owned())?;
            definition = Some(def);
            if index + 1 < component.len() {
                assembly = def.model.assembly().ok_or_else(|| format!("{} is a part, not a sub-assembly: the component path cannot continue inside it.", names.join(" / ")))?;
            }
        }
        let definition = definition.ok_or_else(|| "A component path can't be empty.".to_owned())?;
        if geom.is_some() && plane.is_some() {
            return Err("A mate end takes one geometry selector or one plane selector.".to_owned());
        }
        if geom.is_none() && plane.is_none() {
            return Ok(MateEnd { path, geom: None });
        }
        let name = names.join(" / ");
        let part = Document::from_model((*definition.model).clone(), None);
        let on = |e: String| format!("In {name} (its part, {}): {e}", definition.name());
        let geom = match geom {
            None => plane
                .as_ref()
                .map(|sel| {
                    select::plane(&part, sel)
                        .map(|(r, _)| MateGeom::Plane(r))
                        .map_err(on)
                })
                .transpose()?,
            Some(sel) => {
                if definition.model.is_assembly() {
                    return Err(format!(
                        "{name} is a sub-assembly: extend the component path to a part inside it to select its geometry, or select a standard plane."
                    ));
                }
                // The description is of the part, in the part's own coordinates.
                Some(match sel {
                    GeomSel::Face(f) => MateGeom::Face(select::face_ref(&part, f).map_err(on)?),
                    GeomSel::Edge(e) => MateGeom::Edge(select::edge_ref(&part, e).map_err(on)?),
                    GeomSel::Vertex(v) => {
                        MateGeom::Vertex(select::vertex_ref(&part, v).map_err(on)?)
                    }
                })
            }
        };
        Ok(MateEnd { path, geom })
    }
}

/// What a new mate asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum MateType {
    Coincident,
    Concentric,
    Parallel,
    Distance(Input),
    /// In degrees.
    Angle(Input),
    /// Hold the second component where it is relative to the first, or (what the
    /// application passes) at this placement in the first's coordinates.
    Fasten(Option<Frame>),
    Width {
        a2: Box<MateEndSel>,
        b2: Option<Box<MateEndSel>>,
        mode: WidthModeSel,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum WidthModeSel {
    Centered,
    Free,
    Dimension(Input),
    Percent(Input),
}

impl WidthModeSel {
    fn parse(a: &mut Args) -> Result<Self, String> {
        let mode = a.string("mode")?.unwrap_or_else(|| {
            if a.has("percent") {
                "percent"
            } else if a.has("distance") {
                "dimension"
            } else {
                "centered"
            }
            .to_owned()
        });
        Ok(match mode.as_str() {
            "centered" => Self::Centered,
            "free" => Self::Free,
            "dimension" => Self::Dimension(a.required("distance", Input::parse)?),
            "percent" => Self::Percent(a.required("percent", Input::parse)?),
            _ => return Err("Width mode must be centered, free, dimension or percent.".to_owned()),
        })
    }

    fn resolve(&self, doc: &Document) -> Result<WidthMode, String> {
        Ok(match self {
            Self::Centered => WidthMode::Centered,
            Self::Free => WidthMode::Free,
            Self::Dimension(v) => WidthMode::Dimension(v.scalar(ScalarKind::Length, doc)?),
            Self::Percent(v) => WidthMode::Percent(v.scalar(ScalarKind::Number, doc)?),
        })
    }
}

const TYPES: &str = "coincident, concentric, parallel, distance, angle, fasten, width";

impl MateType {
    fn parse(a: &mut Args) -> Result<(Self, MateEndSel, MateEndSel), String> {
        let word = a.required("type", |v| text(v).map(str::to_owned))?;
        if word == "width" {
            let pair = |v: &Value| -> Result<(MateEndSel, MateEndSel), String> {
                let ends = v.as_array().filter(|v| v.len() == 2).ok_or_else(|| {
                    "A width mate pair needs an array of exactly two mate ends.".to_owned()
                })?;
                Ok((MateEndSel::parse(&ends[0])?, MateEndSel::parse(&ends[1])?))
            };
            let (first, a2) = a.required("a", pair)?;
            let (second, b2) = a.required("b", |v| {
                if v.is_array() {
                    pair(v).map(|(b, b2)| (b, Some(Box::new(b2))))
                } else {
                    MateEndSel::parse(v).map(|b| (b, None))
                }
            })?;
            let mode = WidthModeSel::parse(a)?;
            return Ok((
                Self::Width {
                    a2: Box::new(a2),
                    b2,
                    mode,
                },
                first,
                second,
            ));
        }
        let value = |a: &mut Args, key: &str| {
            a.parsed(key, Input::parse)?
                .ok_or_else(|| format!("A {word} mate needs a '{key}' field."))
        };
        let kind = match word.as_str() {
            "coincident" => Self::Coincident,
            "concentric" => Self::Concentric,
            "parallel" => Self::Parallel,
            "distance" => Self::Distance(value(a, "distance")?),
            "angle" => Self::Angle(value(a, "angle")?),
            "fasten" => Self::Fasten(None),
            other => return Err(format!("type: '{other}' is not a mate: use {TYPES}.")),
        };
        Ok((
            kind,
            a.required("a", MateEndSel::parse)?,
            a.required("b", MateEndSel::parse)?,
        ))
    }
}

/// What to do to a mate.
#[derive(Clone, Debug, PartialEq)]
pub enum MateChange {
    /// Its distance or angle, and which way round two flat faces are: the ones given.
    Edit {
        value: Option<Input>,
        flip: Option<bool>,
    },
    Rename(String),
    /// Leave it out of the solve without deleting it, or put it back.
    Suppress(bool),
    Delete,
    Width(WidthModeSel),
}

impl MateChange {
    pub(crate) fn word(&self) -> &'static str {
        match self {
            Self::Edit { .. } | Self::Width(_) => "edit_mate",
            Self::Rename(_) => "rename",
            Self::Suppress(_) => "suppress",
            Self::Delete => "delete",
        }
    }
}

/// The operations of mates, with their fields, for `help`.
pub(crate) const MATE_OPS: &[(&str, &str, &str)] = &[
    (
        "mate",
        "type (coincident, concentric, parallel, distance, angle, fasten, width), a, b (each {component: name, id or nonempty path array, and face, edge, vertex or plane: a selector in the part's own coordinates}; plane: top, front, right or a reference plane's name or id; for fasten the component alone). Width: a is a pair of planar ends, b is a pair of planar ends or one axis end; each pair belongs to one top-level component; mode (centered, free, dimension, percent; default centered), distance for dimension, percent for percent (number or expression). Other mates: distance, angle, flip. All: name",
        "Hold two components together by geometry of their parts. The components move to where the mates hold, as little as they can.",
    ),
    (
        "edit_mate",
        "mate, distance or angle, flip; width mates: mode (centered, free, dimension, percent), distance for dimension, percent for percent",
        "Change a mate's value or orientation, or a width mate's mode and position.",
    ),
    ("rename", "mate, name", "Rename a mate."),
    (
        "suppress",
        "mate, on (default true)",
        "Leave a mate out of the solve without deleting it, or put it back.",
    ),
    ("delete", "mate", "Delete a mate."),
    (
        "mates",
        "",
        "The mates of the assembly (type, ends, value, status) and how many ways its components can still move.",
    ),
];

/// Reads the mate operation called `op`, if `op` is one for these fields.
pub(crate) fn parse(op: &str, a: &mut Args) -> Option<Result<Op, String>> {
    let change = |a: &mut Args, change: Result<MateChange, String>| {
        Ok(Op::EditMate {
            mate: a.required("mate", MateSel::parse)?,
            change: change?,
        })
    };
    Some(match op {
        "mate" => (|| {
            let (kind, first, second) = MateType::parse(a)?;
            Ok(Op::Mate {
                kind,
                a: first,
                b: second,
                flip: a.parsed("flip", boolean)?,
                name: a.string("name")?,
            })
        })(),
        "mates" => Ok(Op::Query(crate::op::Query::Mates)),
        "edit_mate" => {
            if a.has("mode") || a.has("percent") {
                let mode = WidthModeSel::parse(a).map(MateChange::Width);
                return Some(change(a, mode));
            }
            let edit = (|| {
                let distance = a.parsed("distance", Input::parse)?;
                let angle = a.parsed("angle", Input::parse)?;
                let flip = a.parsed("flip", boolean)?;
                if distance.is_some() && angle.is_some() {
                    return Err("Give 'distance' or 'angle', not both.".to_owned());
                }
                let value = distance.or(angle);
                if value.is_none() && flip.is_none() {
                    return Err(
                        "'edit_mate' needs a 'distance', an 'angle' or a 'flip' field.".to_owned(),
                    );
                }
                Ok(MateChange::Edit { value, flip })
            })();
            change(a, edit)
        }
        // These words are a feature's and a component's too: a mate's when given one.
        "rename" if a.has("mate") => {
            let to = a
                .required("name", |v| text(v).map(str::to_owned))
                .map(MateChange::Rename);
            change(a, to)
        }
        "suppress" if a.has("mate") => {
            let on = a.flag("on", true).map(MateChange::Suppress);
            change(a, on)
        }
        "delete" if a.has("mate") => change(a, Ok(MateChange::Delete)),
        _ => return None,
    })
}

fn status_word(status: Option<&Status>) -> &'static str {
    match status {
        Some(Status::Ok) => "ok",
        Some(Status::Warning(_)) => "warning",
        Some(Status::Failed(_)) => "failed",
        Some(Status::Suppressed | Status::SuppressedBy(_)) => "suppressed",
        Some(Status::RolledBack) | None => "not_built",
    }
}

pub(crate) fn end_out(assembly: &Assembly, end: &MateEnd) -> Value {
    let mut m = Map::new();
    let mut current = Some(assembly);
    let mut names = Vec::new();
    let mut model = None;
    for id in &end.path {
        names.push(current.map_or("?", |a| a.name_of(*id)));
        model = current
            .and_then(|a| a.definition_of(*id))
            .map(|d| &*d.model);
        current = model.and_then(Model::assembly);
    }
    m.insert("component".to_owned(), json!(names.first().copied()));
    if end.path.len() > 1 {
        m.insert("inside".to_owned(), json!(end.path.len() - 1));
        m.insert("path".to_owned(), json!(names));
    }
    if let Some(MateGeom::Plane(
        plane @ (peet_model::PlaneRef::Standard(_) | peet_model::PlaneRef::Feature(_)),
    )) = &end.geom
    {
        let value = match plane {
            peet_model::PlaneRef::Standard(p) => json!(p.label()),
            peet_model::PlaneRef::Feature(id) => {
                model.map_or_else(|| json!(id.0), |m| json!(m.name_of(*id)))
            }
            peet_model::PlaneRef::Face(_) => Value::Null,
        };
        m.insert("plane".to_owned(), value);
    }
    m.insert(
        "on".to_owned(),
        json!(match &end.geom {
            Some(MateGeom::Face(_)) => "face",
            Some(MateGeom::Edge(_)) => "edge",
            Some(MateGeom::Vertex(_)) => "vertex",
            Some(MateGeom::Plane(_)) => "plane",
            None => "component",
        }),
    );
    Value::Object(m)
}

/// A mate in a few fields: what replies and `mates` give.
pub(crate) fn mate_out(doc: &Document, id: MateId) -> Value {
    let Some(assembly) = doc.model.assembly() else {
        return Value::Null;
    };
    let Some(mate) = assembly.mate(id) else {
        return json!({ "id": id.0, "status": "deleted" });
    };
    let status = doc.evaluation().mate_status(id);
    let mut m = Map::new();
    m.insert("id".to_owned(), json!(id.0));
    m.insert("name".to_owned(), json!(mate.name));
    m.insert("type".to_owned(), json!(mate.kind.word()));
    let (a, b) = match &mate.kind {
        MateKind::Width { a2, b2, .. } => (
            json!([end_out(assembly, &mate.a), end_out(assembly, a2)]),
            b2.as_ref().map_or_else(
                || end_out(assembly, &mate.b),
                |b2| json!([end_out(assembly, &mate.b), end_out(assembly, b2)]),
            ),
        ),
        _ => (end_out(assembly, &mate.a), end_out(assembly, &mate.b)),
    };
    m.insert("a".to_owned(), a);
    m.insert("b".to_owned(), b);
    match &mate.kind {
        MateKind::Width { mode, .. } => {
            m.insert("mode".to_owned(), json!(mode.word()));
            match mode {
                WidthMode::Dimension(s) => {
                    m.insert(
                        "distance".to_owned(),
                        scalar_out(s, ScalarKind::Length, doc),
                    );
                }
                WidthMode::Percent(s) => {
                    m.insert("percent".to_owned(), scalar_out(s, ScalarKind::Number, doc));
                }
                _ => {}
            }
        }
        MateKind::Distance(s) => {
            m.insert(
                "distance".to_owned(),
                scalar_out(s, ScalarKind::Length, doc),
            );
        }
        MateKind::Angle(s) => {
            m.insert("angle".to_owned(), scalar_out(s, ScalarKind::Angle, doc));
        }
        _ => {}
    }
    if mate.flip {
        m.insert("flip".to_owned(), json!(true));
    }
    m.insert("status".to_owned(), json!(status_word(status)));
    if let Some(message) = status.and_then(Status::message) {
        m.insert("message".to_owned(), json!(message));
    }
    Value::Object(m)
}

/// The mates that don't hold: part of what `failures` is for an assembly.
pub(crate) fn failures(doc: &Document) -> Vec<Value> {
    let Some(assembly) = doc.model.assembly() else {
        return Vec::new();
    };
    assembly
        .mates()
        .filter(|m| {
            doc.evaluation()
                .mate_status(m.id)
                .is_some_and(Status::is_failed)
        })
        .map(|m| mate_out(doc, m.id))
        .collect()
}

/// The mates of the assembly, and the freedom they leave.
pub(crate) fn mates(doc: &Document) -> Result<Map<String, Value>, String> {
    let assembly = the_assembly(doc)?;
    let list: Vec<Value> = assembly.mates().map(|m| mate_out(doc, m.id)).collect();
    let mut out = Map::new();
    out.insert("mates".to_owned(), json!(list));
    out.insert("freedom".to_owned(), json!(doc.evaluation().freedom));
    Ok(out)
}

fn rename(assembly: &mut Assembly, id: MateId, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A name can't be empty.".to_owned());
    }
    if assembly.mates().any(|m| m.id != id && m.name == name) {
        return Err(format!("Another mate is already called '{name}'."));
    }
    if let Some(m) = assembly.mate_mut(id) {
        name.clone_into(&mut m.name);
    }
    Ok(())
}

/// Adds a mate. Returns it, with the undo label.
pub(crate) fn add(
    doc: &Document,
    model: &mut Model,
    kind: &MateType,
    a: &MateEndSel,
    b: &MateEndSel,
    flip: Option<bool>,
    name: Option<&str>,
) -> Result<(MateId, String), String> {
    let (mut a, mut b) = (a.resolve(doc)?, b.resolve(doc)?);
    let assembly = the_assembly(doc)?;
    let component = |end: &MateEnd| -> Result<CompId, String> {
        end.component()
            .filter(|c| assembly.component(*c).is_some())
            .ok_or_else(|| "A mate's component is not in the assembly.".to_owned())
    };
    let (ca, cb) = (component(&a)?, component(&b)?);
    crate::pattern::placed_by_pattern(assembly, ca, "mated")?;
    crate::pattern::placed_by_pattern(assembly, cb, "mated")?;
    if ca == cb {
        return Err(format!(
            "Both ends are on {}: a mate joins two components.",
            assembly.name_of(ca)
        ));
    }
    let needs_geometry = |end: &MateEnd, which: &str| {
        if end.geom.is_none() {
            Err(format!(
                "'{which}' names {} alone: this mate needs a face, an edge, a vertex or a plane of it (only fasten takes a component as a whole).",
                assembly.name_of(end.component().unwrap_or(ca))
            ))
        } else {
            Ok(())
        }
    };
    let kind = match kind {
        MateType::Width { a2, b2, mode } => {
            if flip == Some(true) {
                return Err(
                    "A width mate keeps the nearest orientation and does not take flip.".to_owned(),
                );
            }
            let a2 = a2.resolve(doc)?;
            let b2 = b2.as_ref().map(|b| b.resolve(doc)).transpose()?;
            for (first, second) in [(&a, Some(&a2)), (&b, b2.as_ref())] {
                needs_geometry(first, "width/tab")?;
                if let Some(second) = second {
                    needs_geometry(second, "width/tab")?;
                    if first.component() != second.component() {
                        return Err("Each width mate pair must belong to one top-level component. The width pair and tab must belong to different components.".to_owned());
                    }
                }
            }
            MateKind::Width {
                a2,
                b2,
                mode: mode.resolve(doc)?,
            }
        }
        MateType::Fasten(relative) => {
            // The components as wholes, where they are now.
            a.geom = None;
            b.geom = None;
            a.path.truncate(1);
            b.path.truncate(1);
            let frame = |c: CompId| assembly.component(c).map(|c| c.placement);
            let relative = match (relative, frame(ca), frame(cb)) {
                (Some(r), ..) => *r,
                (None, Some(fa), Some(fb)) => fa.inverse().compose(&fb),
                _ => return Err("A mate's component is not in the assembly.".to_owned()),
            };
            MateKind::Fasten(relative)
        }
        other => {
            needs_geometry(&a, "a")?;
            needs_geometry(&b, "b")?;
            match other {
                MateType::Coincident => MateKind::Coincident,
                MateType::Concentric => MateKind::Concentric,
                MateType::Parallel => MateKind::Parallel,
                MateType::Distance(v) => MateKind::Distance(
                    v.scalar(ScalarKind::Length, doc)
                        .map_err(|e| format!("distance: {e}"))?,
                ),
                MateType::Angle(v) => MateKind::Angle(
                    v.scalar(ScalarKind::Angle, doc)
                        .map_err(|e| format!("angle: {e}"))?,
                ),
                MateType::Fasten(_) | MateType::Width { .. } => unreachable!("handled above"),
            }
        }
    };
    let assembly = model
        .assembly_mut()
        .ok_or_else(|| "The document is not an assembly.".to_owned())?;
    let id = assembly.add_mate(kind, a, b);
    if let Some(name) = name {
        rename(assembly, id, name)?;
    }
    if let (Some(flip), Some(m)) = (flip, assembly.mate_mut(id)) {
        m.flip = flip;
    }
    let label = format!(
        "Add {}",
        assembly.mate(id).map_or("Mate", |m: &Mate| m.name.as_str())
    );
    Ok((id, label))
}

/// Changes a mate. Returns the undo label.
/// The mate whose distance or angle is at `address` in an assembly, as SolidWorks names a
/// mate's value: `D1@Distance6` (the case of `D1` and of the name doesn't matter, where only
/// one mate has the name in any case). For `set` on an assembly.
pub(crate) fn value_address(
    doc: &Document,
    address: &str,
    configurations: bool,
) -> Result<MateId, String> {
    let assembly = the_assembly(doc)?;
    let Some(name) = address
        .split_once('@')
        .filter(|(d, rest)| d.eq_ignore_ascii_case("D1") && !rest.contains('@'))
        .map(|(_, name)| name.trim())
    else {
        return Err(format!(
            "{} is an assembly: 'set' takes a mate's distance or angle there, by SolidWorks' name D1@MATE (\"D1@Distance2\"). A dimension of a part ('{address}') is set in the part: open it with 'open_component'.",
            doc.title()
        ));
    };
    if configurations {
        return Err(
            "An assembly has no configurations of its own: leave 'configurations' out.".to_owned(),
        );
    }
    let exact = assembly.mates().find(|m| m.name == name);
    let mut alike = assembly
        .mates()
        .filter(|m| m.name.eq_ignore_ascii_case(name));
    match (exact, alike.next(), alike.next()) {
        (Some(m), ..) | (None, Some(m), None) => Ok(m.id),
        _ => MateSel::Name(name.to_owned()).resolve(doc),
    }
}

pub(crate) fn change(
    doc: &Document,
    model: &mut Model,
    id: MateId,
    change: &MateChange,
) -> Result<String, String> {
    if matches!(
        the_assembly(doc)?.mate(id).map(|m| &m.kind),
        Some(MateKind::Width { .. })
    ) && matches!(change, MateChange::Edit { flip: Some(_), .. })
    {
        return Err(
            "A width mate keeps the nearest orientation and does not take flip.".to_owned(),
        );
    }
    let was = the_assembly(doc)?
        .mate(id)
        .map(|m| m.name.clone())
        .ok_or_else(|| "The mate no longer exists.".to_owned())?;
    // Read the value against the document before anything changes.
    let value = match change {
        MateChange::Edit {
            value: Some(input), ..
        } => {
            let kind = match the_assembly(doc)?.mate(id).map(|m| &m.kind) {
                Some(MateKind::Distance(_)) => ScalarKind::Length,
                Some(MateKind::Angle(_)) => ScalarKind::Angle,
                Some(MateKind::Width {
                    mode: WidthMode::Dimension(_),
                    ..
                }) => ScalarKind::Length,
                Some(other) => {
                    return Err(format!(
                        "{was} is a {} mate: it has no distance or angle to set.",
                        other.word()
                    ));
                }
                None => return Err("The mate no longer exists.".to_owned()),
            };
            Some((kind, input.scalar(kind, doc)?))
        }
        _ => None,
    };
    let width_mode = match change {
        MateChange::Width(mode) => Some(mode.resolve(doc)?),
        _ => None,
    };
    let assembly = model
        .assembly_mut()
        .ok_or_else(|| "The document is not an assembly.".to_owned())?;
    let gone = || "The mate no longer exists.".to_owned();
    Ok(match change {
        MateChange::Edit { flip, .. } => {
            let mate = assembly.mate_mut(id).ok_or_else(gone)?;
            match value {
                Some((ScalarKind::Length, s)) => {
                    if let MateKind::Width { mode, .. } = &mut mate.kind {
                        *mode = WidthMode::Dimension(s);
                    } else {
                        mate.kind = MateKind::Distance(s);
                    }
                }
                Some((_, s)) => mate.kind = MateKind::Angle(s),
                None => {}
            }
            if let Some(flip) = flip {
                mate.flip = *flip;
            }
            format!("Edit {was}")
        }
        MateChange::Rename(name) => {
            rename(assembly, id, name)?;
            format!("Rename {was}")
        }
        MateChange::Suppress(on) => {
            assembly.mate_mut(id).ok_or_else(gone)?.suppressed = *on;
            format!("{} {was}", if *on { "Suppress" } else { "Unsuppress" })
        }
        MateChange::Delete => {
            assembly.remove_mate(id).ok_or_else(gone)?;
            format!("Delete {was}")
        }
        MateChange::Width(_) => {
            let mate = assembly.mate_mut(id).ok_or_else(gone)?;
            let MateKind::Width { mode, .. } = &mut mate.kind else {
                return Err("Only a width mate takes mode or percent.".to_owned());
            };
            *mode = width_mode.expect("resolved above");
            format!("Edit {was}")
        }
    })
}
