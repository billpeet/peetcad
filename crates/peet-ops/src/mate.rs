//! Operations on the mates of an assembly.
//!
//! A mate's end is a component and geometry of its part: a face, an edge or a vertex,
//! described by the same selectors a part's operations take, in the part's own
//! coordinates (not where the component is in the assembly). A description is resolved
//! against the part as it is now and stored as a persistent reference, so the mate
//! follows its face through edits to the part.

use peet_document::Document;
use peet_math::Frame;
use peet_model::{
    Assembly, CompId, Mate, MateEnd, MateGeom, MateId, MateKind, Model, ScalarKind, Status,
};
use serde_json::{Map, Value, json};

use crate::args::{Args, boolean, integer, text};
use crate::assembly::CompSel;
use crate::fields::scalar_out;
use crate::op::Op;
use crate::select;
use crate::value::{GeomSel, Input};

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
    /// As a script writes it: a component, and a face, an edge or a vertex of its part
    /// (none for a fasten mate, which holds the component as a whole).
    Find {
        component: CompSel,
        geom: Option<GeomSel>,
    },
    /// Already resolved: what the application passes after a click.
    Ref(MateEnd),
}

impl MateEndSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        let wrong = || {
            format!(
                "expected {{\"component\": name, \"face\": selector}} (or \"edge\", or \"vertex\"; or the component alone, for fasten), not {v}"
            )
        };
        let mut map = match v {
            Value::Object(map) => map.clone(),
            // The component alone.
            Value::String(_) | Value::Number(_) => {
                return Ok(Self::Find {
                    component: CompSel::parse(v)?,
                    geom: None,
                });
            }
            _ => return Err(wrong()),
        };
        let component = CompSel::parse(&map.remove("component").ok_or_else(wrong)?)?;
        let geom = match map.len() {
            0 => None,
            1 => Some(GeomSel::parse(&Value::Object(map))?),
            _ => return Err(wrong()),
        };
        Ok(Self::Find { component, geom })
    }

    /// The end as a mate stores it, with the component it is on.
    pub(crate) fn resolve(&self, doc: &Document) -> Result<MateEnd, String> {
        let (component, geom) = match self {
            Self::Ref(end) => return Ok(end.clone()),
            Self::Find { component, geom } => (component, geom),
        };
        let assembly = the_assembly(doc)?;
        let id = component.resolve(doc)?;
        let geom = match geom {
            None => None,
            Some(sel) => {
                let definition = assembly
                    .definition_of(id)
                    .ok_or_else(|| "The component's part is missing.".to_owned())?;
                let name = assembly.name_of(id);
                if definition.model.is_assembly() {
                    return Err(format!(
                        "{name} is a sub-assembly: a script can't describe a face of a part inside it yet. Mate the sub-assembly's parts in the sub-assembly, or fasten it as a whole."
                    ));
                }
                // The description is of the part, in the part's own coordinates.
                let part = Document::from_model((*definition.model).clone(), None);
                let on = |e: String| format!("In {name} (its part, {}): {e}", definition.name());
                Some(match sel {
                    GeomSel::Face(f) => MateGeom::Face(select::face_ref(&part, f).map_err(on)?),
                    GeomSel::Edge(e) => MateGeom::Edge(select::edge_ref(&part, e).map_err(on)?),
                    GeomSel::Vertex(v) => {
                        MateGeom::Vertex(select::vertex_ref(&part, v).map_err(on)?)
                    }
                })
            }
        };
        Ok(MateEnd {
            path: vec![id],
            geom,
        })
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
}

const TYPES: &str = "coincident, concentric, parallel, distance, angle, fasten";

impl MateType {
    fn parse(a: &mut Args) -> Result<Self, String> {
        let word = a.required("type", |v| text(v).map(str::to_owned))?;
        let value = |a: &mut Args, key: &str| {
            a.parsed(key, Input::parse)?
                .ok_or_else(|| format!("A {word} mate needs a '{key}' field."))
        };
        Ok(match word.as_str() {
            "coincident" => Self::Coincident,
            "concentric" => Self::Concentric,
            "parallel" => Self::Parallel,
            "distance" => Self::Distance(value(a, "distance")?),
            "angle" => Self::Angle(value(a, "angle")?),
            "fasten" => Self::Fasten(None),
            other => return Err(format!("type: '{other}' is not a mate: use {TYPES}.")),
        })
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
}

impl MateChange {
    pub(crate) fn word(&self) -> &'static str {
        match self {
            Self::Edit { .. } => "edit_mate",
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
        "type (coincident, concentric, parallel, distance, angle, fasten), a, b (each {component, and face, edge or vertex: a selector in the part's own coordinates}; for fasten the component alone), distance, angle, flip, name",
        "Hold two components together by geometry of their parts. The components move to where the mates hold, as little as they can.",
    ),
    (
        "edit_mate",
        "mate, distance or angle, flip",
        "Change a mate's distance or angle, or which way round two flat faces are.",
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
            Ok(Op::Mate {
                kind: MateType::parse(a)?,
                a: a.required("a", MateEndSel::parse)?,
                b: a.required("b", MateEndSel::parse)?,
                flip: a.parsed("flip", boolean)?,
                name: a.string("name")?,
            })
        })(),
        "mates" => Ok(Op::Query(crate::op::Query::Mates)),
        "edit_mate" => {
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
        Some(Status::Suppressed) => "suppressed",
        Some(Status::RolledBack) | None => "not_built",
    }
}

fn end_out(assembly: &Assembly, end: &MateEnd) -> Value {
    let mut m = Map::new();
    let names: Vec<&str> = end
        .component()
        .map(|c| assembly.name_of(c))
        .into_iter()
        .collect();
    m.insert("component".to_owned(), json!(names.first().copied()));
    if end.path.len() > 1 {
        m.insert("inside".to_owned(), json!(end.path.len() - 1));
    }
    m.insert(
        "on".to_owned(),
        json!(match &end.geom {
            Some(MateGeom::Face(_)) => "face",
            Some(MateGeom::Edge(_)) => "edge",
            Some(MateGeom::Vertex(_)) => "vertex",
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
    m.insert("a".to_owned(), end_out(assembly, &mate.a));
    m.insert("b".to_owned(), end_out(assembly, &mate.b));
    match &mate.kind {
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
                "'{which}' names {} alone: this mate needs a face, an edge or a vertex of it (only fasten takes a component as a whole).",
                assembly.name_of(end.component().unwrap_or(ca))
            ))
        } else {
            Ok(())
        }
    };
    let kind = match kind {
        MateType::Fasten(relative) => {
            // The components as wholes, where they are now.
            a.geom = None;
            b.geom = None;
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
                MateType::Fasten(_) => unreachable!("handled above"),
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
pub(crate) fn change(
    doc: &Document,
    model: &mut Model,
    id: MateId,
    change: &MateChange,
) -> Result<String, String> {
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
    let assembly = model
        .assembly_mut()
        .ok_or_else(|| "The document is not an assembly.".to_owned())?;
    let gone = || "The mate no longer exists.".to_owned();
    Ok(match change {
        MateChange::Edit { flip, .. } => {
            let mate = assembly.mate_mut(id).ok_or_else(gone)?;
            match value {
                Some((ScalarKind::Length, s)) => mate.kind = MateKind::Distance(s),
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
    })
}
