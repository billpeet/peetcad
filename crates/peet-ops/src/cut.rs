//! Assembly cuts: an extruded cut made in an assembly, with its own sketch, as SolidWorks'
//! assembly feature cut. It removes material from the components in the assembly only:
//! their parts are not changed. Its scope is every component it reaches, or the ones
//! listed. Its sketch lies on one of the assembly's planes or on a flat face or plane of
//! a component, and entities of it can be converted from components' edges, which they
//! then follow.

use peet_document::Document;
use peet_math::{DVec3, Plane};
use peet_model::{
    Assembly, AssemblyCut, CompId, CutConversion, CutId, CutPlane, EndCondition, MateGeom, Model,
    PlaneRef, ScalarKind, SketchFeature, Status, StdPlane,
};
use serde_json::{Map, Value, json};

use crate::args::{Args, boolean, coordinates, integer, list, point3_out, text};
use crate::assembly::CompSel;
use crate::fields::scalar_out;
use crate::mate::{MateEndSel, end_out};
use crate::op::Op;
use crate::sketch::{self, DrawItem};
use crate::value::Input;

/// An assembly cut, by name (`"Cut-Extrude1"`) or id.
#[derive(Clone, Debug, PartialEq)]
pub enum CutSel {
    Id(CutId),
    Name(String),
}

fn the_assembly(doc: &Document) -> Result<&Assembly, String> {
    doc.model
        .assembly()
        .ok_or_else(|| format!("{} is a part: an assembly cut is made in an assembly (in a part, sketch and extrude a cut).", doc.title()))
}

impl CutSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        match v {
            Value::Number(_) => Ok(Self::Id(CutId(integer(v)?))),
            Value::String(name) => Ok(Self::Name(name.clone())),
            other => Err(format!(
                "expected an assembly cut's name or id, not {other}"
            )),
        }
    }

    pub fn resolve(&self, doc: &Document) -> Result<CutId, String> {
        let assembly = the_assembly(doc)?;
        let names = || {
            let names: Vec<&str> = assembly.cuts().map(|c| c.name.as_str()).collect();
            if names.is_empty() {
                "The assembly has no cuts yet: add one with assembly_cut.".to_owned()
            } else {
                format!("The cuts are: {}.", names.join(", "))
            }
        };
        match self {
            Self::Id(id) => assembly.cut(*id).map(|c| c.id).ok_or_else(|| {
                format!("There is no assembly cut with the id {}. {}", id.0, names())
            }),
            Self::Name(name) => assembly
                .cuts()
                .find(|c| c.name == *name || c.sketch_name == *name)
                .map(|c| c.id)
                .ok_or_else(|| format!("There is no assembly cut called '{name}'. {}", names())),
        }
    }
}

/// What the sketch lies on, as an operation gives it.
#[derive(Clone, Debug, PartialEq)]
pub enum CutPlaneSel {
    /// A plane of the assembly at a place: its origin, normal and x direction.
    Fixed {
        origin: [f64; 3],
        normal: DVec3,
        x: Option<DVec3>,
    },
    /// The assembly's own standard plane, or a flat face or plane of a component.
    End(Box<MateEndSel>),
}

impl CutPlaneSel {
    fn parse(v: &Value) -> Result<Self, String> {
        match v {
            Value::String(s) => {
                MateEndSel::parse(&json!({ "plane": s })).map(|e| Self::End(Box::new(e)))
            }
            Value::Object(map) if map.contains_key("normal") => {
                let normal = DVec3::from_array(coordinates::<3>(&map["normal"])?);
                let origin = match map.get("origin") {
                    Some(o) => coordinates::<3>(o)?,
                    None => [0.0; 3],
                };
                let x = map
                    .get("x")
                    .map(|x| coordinates::<3>(x).map(DVec3::from_array))
                    .transpose()?;
                if map
                    .keys()
                    .any(|k| !["normal", "origin", "x"].contains(&k.as_str()))
                {
                    return Err(format!(
                        "A plane at a place is {{\"origin\": [x, y, z], \"normal\": [x, y, z], \"x\": [x, y, z]}}, not {v}"
                    ));
                }
                Ok(Self::Fixed { origin, normal, x })
            }
            other => MateEndSel::parse(other).map(|e| Self::End(Box::new(e))),
        }
    }

    fn resolve(&self, doc: &Document) -> Result<(CutPlane, Plane), String> {
        match self {
            Self::Fixed { origin, normal, x } => {
                let origin = crate::args::mm3(*origin, &doc.model.parameters.units);
                let x = x.unwrap_or_else(|| {
                    if normal.normalize_or_zero().cross(DVec3::X).length() > 1e-6 {
                        DVec3::X
                    } else {
                        DVec3::Y
                    }
                });
                let plane = Plane::from_origin_normal_x(origin, *normal, x).ok_or_else(|| {
                    "'on': the normal has no length, or 'x' is along it.".to_owned()
                })?;
                Ok((CutPlane::Fixed(plane), plane))
            }
            Self::End(end) => {
                let end = end.resolve(doc)?;
                let flat = matches!(&end.geom, Some(MateGeom::Face(_) | MateGeom::Plane(_)));
                if !flat {
                    return Err(
                        "'on' is a plane of the assembly (\"top\", \"front\", \"right\"), a flat face or a plane of a component ({\"component\": ..., \"face\": ...} or \"plane\"), or {\"origin\", \"normal\", \"x\"}."
                            .to_owned(),
                    );
                }
                let placement = match &end.geom {
                    Some(MateGeom::Plane(PlaneRef::Standard(p))) if end.path.is_empty() => {
                        p.plane()
                    }
                    _ => StdPlane::Top.plane(),
                };
                Ok((CutPlane::Geom(end), placement))
            }
        }
    }
}

/// How far a cut goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CutEnd {
    Blind,
    MidPlane,
    ThroughAll,
}

impl CutEnd {
    fn parse(v: &Value) -> Result<Self, String> {
        match text(v)? {
            "blind" => Ok(Self::Blind),
            "mid_plane" => Ok(Self::MidPlane),
            "through_all" => Ok(Self::ThroughAll),
            other => Err(format!(
                "'{other}' is not how an assembly cut ends: use \"blind\", \"mid_plane\" or \"through_all\"."
            )),
        }
    }

    fn condition(self) -> EndCondition {
        match self {
            Self::Blind => EndCondition::Blind,
            Self::MidPlane => EndCondition::Symmetric,
            Self::ThroughAll => EndCondition::ThroughAll,
        }
    }
}

/// What an assembly cut is or becomes: the fields given.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CutSpec {
    pub on: Option<CutPlaneSel>,
    pub draw: Vec<DrawItem>,
    /// Components' edges to convert into the sketch.
    pub convert: Vec<MateEndSel>,
    pub end: Option<CutEnd>,
    pub depth: Option<Input>,
    pub flip: Option<bool>,
    pub flip_side: Option<bool>,
    /// The components it may cut; `Some(None)`: every one it reaches.
    pub components: Option<Option<Vec<CompSel>>>,
    pub suppressed: Option<bool>,
    pub name: Option<String>,
    pub sketch_name: Option<String>,
}

/// What to do to an assembly cut.
#[derive(Clone, Debug, PartialEq)]
pub enum CutChange {
    Edit(Box<CutSpec>),
    Rename(String),
    Delete,
}

impl CutChange {
    pub(crate) fn word(&self) -> &'static str {
        match self {
            Self::Edit(_) => "edit_assembly_cut",
            Self::Rename(_) => "rename",
            Self::Delete => "delete",
        }
    }
}

pub(crate) const CUT_OPS: &[(&str, &str, &str)] = &[
    (
        "assembly_cut",
        "on (\"top\", \"front\" or \"right\": the assembly's plane; {component, face or plane}: a flat face or plane of a component, which the sketch follows; or {origin, normal, x}), draw (as for sketch, in the plane's coordinates), convert (list of {component, edge}: components' edges converted into the sketch, which follow them), end (blind, mid_plane, through_all; default blind), depth (default 10), flip (go along the plane's normal instead of against it), flip_side, components (the components it may cut; left out: every one it reaches), name, sketch_name",
        "Cut components in the assembly with an extruded sketch, as SolidWorks' assembly cut: their parts are not changed.",
    ),
    (
        "edit_assembly_cut",
        "cut, and any of on, draw (added), convert (added), end, depth, flip, flip_side, components (null: every one it reaches), suppressed",
        "Change an assembly cut.",
    ),
    ("rename", "cut, name", "Rename an assembly cut."),
    (
        "delete",
        "cut",
        "Delete an assembly cut: the components are whole again.",
    ),
];

fn components(v: &Value) -> Result<Vec<CompSel>, String> {
    match v {
        Value::Array(items) => items.iter().map(CompSel::parse).collect(),
        other => Ok(vec![CompSel::parse(other)?]),
    }
}

fn spec(a: &mut Args, new: bool) -> Result<CutSpec, String> {
    let on = if new {
        Some(a.required("on", CutPlaneSel::parse)?)
    } else {
        a.parsed("on", CutPlaneSel::parse)?
    };
    let convert = a
        .parsed("convert", |v| {
            list(v)?.iter().map(MateEndSel::parse).collect()
        })?
        .unwrap_or_default();
    let scope = match a.take_nullable("components") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => Some(Some(components(&v)?)),
    };
    Ok(CutSpec {
        on,
        draw: a.parsed("draw", sketch::parse)?.unwrap_or_default(),
        convert,
        end: a.parsed("end", CutEnd::parse)?,
        depth: a.parsed("depth", Input::parse)?,
        flip: a.parsed("flip", boolean)?,
        flip_side: a.parsed("flip_side", boolean)?,
        components: scope,
        suppressed: if new {
            None
        } else {
            a.parsed("suppressed", boolean)?
        },
        name: if new { a.string("name")? } else { None },
        sketch_name: if new { a.string("sketch_name")? } else { None },
    })
}

pub(crate) fn parse(op: &str, a: &mut Args) -> Option<Result<Op, String>> {
    let change = |a: &mut Args, change: Result<CutChange, String>| {
        Ok(Op::EditAssemblyCut {
            cut: a.required("cut", CutSel::parse)?,
            change: change?,
        })
    };
    Some(match op {
        "assembly_cut" => spec(a, true).map(|spec| Op::AssemblyCut { spec }),
        "edit_assembly_cut" => {
            let edit = spec(a, false).and_then(|s| {
                if s == CutSpec::default() {
                    Err("'edit_assembly_cut' needs something to change.".to_owned())
                } else {
                    Ok(CutChange::Edit(Box::new(s)))
                }
            });
            change(a, edit)
        }
        "rename" if a.has("cut") => {
            let to = a
                .required("name", |v| text(v).map(str::to_owned))
                .map(CutChange::Rename);
            change(a, to)
        }
        "delete" if a.has("cut") => change(a, Ok(CutChange::Delete)),
        _ => return None,
    })
}

fn resolve_scope(doc: &Document, list: &[CompSel]) -> Result<Vec<CompId>, String> {
    let mut out = Vec::new();
    for c in list {
        let id = c.resolve(doc)?;
        if !out.contains(&id) {
            out.push(id);
        }
    }
    if out.is_empty() {
        return Err(
            "'components' is empty: name the components it may cut, or leave it out to cut every one it reaches."
                .to_owned(),
        );
    }
    Ok(out)
}

/// Applies what `spec` gives to `cut`, read against the document.
fn fill(doc: &Document, cut: &mut AssemblyCut, spec: &CutSpec) -> Result<(), String> {
    if let Some(on) = &spec.on {
        let (plane, placement) = on.resolve(doc)?;
        cut.plane = plane;
        cut.placement = placement;
    }
    for e in &spec.convert {
        let edge = e.resolve(doc)?;
        if !matches!(edge.geom, Some(MateGeom::Edge(_))) || edge.path.is_empty() {
            return Err(
                "'convert' takes edges of components: {\"component\": ..., \"edge\": ...}."
                    .to_owned(),
            );
        }
        cut.conversions.push(CutConversion { entity: None, edge });
    }
    if !spec.draw.is_empty() {
        let mut feature = SketchFeature {
            plane: PlaneRef::Standard(StdPlane::Top),
            placement: cut.placement,
            sketch: std::mem::take(&mut cut.sketch),
            projections: Vec::new(),
        };
        let drawn = sketch::draw(&mut feature, &spec.draw, doc);
        cut.sketch = feature.sketch;
        drawn?;
    }
    if let Some(end) = spec.end {
        cut.params.end = end.condition();
    }
    if let Some(depth) = &spec.depth {
        cut.params.depth = depth
            .scalar(ScalarKind::Length, doc)
            .map_err(|e| format!("depth: {e}"))?;
    }
    if let Some(flip) = spec.flip {
        cut.params.reverse = flip;
    }
    if let Some(flip) = spec.flip_side {
        cut.params.flip_side = flip;
    }
    if let Some(scope) = &spec.components {
        cut.scope = scope.as_ref().map(|s| resolve_scope(doc, s)).transpose()?;
    }
    if let Some(s) = spec.suppressed {
        cut.suppressed = s;
    }
    Ok(())
}

fn unique(assembly: &Assembly, id: Option<CutId>, name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A name can't be empty.".to_owned());
    }
    if assembly
        .cuts()
        .any(|c| Some(c.id) != id && (c.name == name || c.sketch_name == name))
    {
        return Err(format!(
            "Another assembly cut or its sketch is already called '{name}'."
        ));
    }
    Ok(name.to_owned())
}

pub(crate) fn add(
    doc: &Document,
    model: &mut Model,
    spec: &CutSpec,
) -> Result<(CutId, String), String> {
    let assembly = the_assembly(doc)?;
    let mut cut = AssemblyCut::new(
        assembly.next_cut_id(),
        CutPlane::Fixed(Plane::TOP),
        Plane::TOP,
    );
    if let Some(n) = &spec.name {
        cut.name = unique(assembly, None, n)?;
    }
    if let Some(n) = &spec.sketch_name {
        cut.sketch_name = unique(assembly, None, n)?;
    }
    fill(doc, &mut cut, spec)?;
    if cut.sketch.entities().next().is_none() {
        return Err("The cut's sketch is empty: give 'draw' or 'convert'.".to_owned());
    }
    let assembly = model
        .assembly_mut()
        .ok_or_else(|| "The document is not an assembly.".to_owned())?;
    let id = assembly.add_cut(cut);
    let label = format!("Add {}", assembly.cut(id).map_or("", |c| c.name.as_str()));
    Ok((id, label))
}

pub(crate) fn change(
    doc: &Document,
    model: &mut Model,
    id: CutId,
    change: &CutChange,
) -> Result<String, String> {
    let assembly = the_assembly(doc)?;
    let gone = || "The assembly cut no longer exists.".to_owned();
    let was = assembly.cut(id).ok_or_else(gone)?.clone();
    let edited = match change {
        CutChange::Edit(spec) => {
            let mut cut = was.clone();
            fill(doc, &mut cut, spec)?;
            Some(cut)
        }
        CutChange::Rename(name) => {
            let mut cut = was.clone();
            cut.name = unique(assembly, Some(id), name)?;
            Some(cut)
        }
        CutChange::Delete => None,
    };
    let assembly = model
        .assembly_mut()
        .ok_or_else(|| "The document is not an assembly.".to_owned())?;
    Ok(match (change, edited) {
        (_, Some(cut)) => {
            *assembly.cut_mut(id).ok_or_else(gone)? = cut;
            match change {
                CutChange::Rename(_) => format!("Rename {}", was.name),
                _ => format!("Edit {}", was.name),
            }
        }
        (_, None) => {
            assembly.remove_cut(id).ok_or_else(gone)?;
            format!("Delete {}", was.name)
        }
    })
}

/// A cut in a few fields: what replies and `components` give.
pub(crate) fn cut_out(doc: &Document, id: CutId) -> Value {
    let Some((assembly, cut)) = doc.model.assembly().and_then(|a| Some((a, a.cut(id)?))) else {
        return json!({ "id": id.0, "status": "deleted" });
    };
    let units = &doc.model.parameters.units;
    let mut m = Map::new();
    m.insert("id".to_owned(), json!(cut.id.0));
    m.insert("name".to_owned(), json!(cut.name));
    m.insert("sketch".to_owned(), json!(cut.sketch_name));
    m.insert(
        "on".to_owned(),
        match &cut.plane {
            CutPlane::Fixed(p) => json!({
                "origin": point3_out(p.origin(), units),
                "normal": crate::args::direction_out(p.normal()),
            }),
            CutPlane::Geom(end) => end_out(assembly, end),
        },
    );
    m.insert(
        "end".to_owned(),
        json!(match cut.params.end {
            EndCondition::ThroughAll => "through_all",
            EndCondition::Symmetric => "mid_plane",
            _ => "blind",
        }),
    );
    if cut.params.end.uses_depth() {
        m.insert(
            "depth".to_owned(),
            scalar_out(&cut.params.depth, ScalarKind::Length, doc),
        );
    }
    if cut.params.reverse {
        m.insert("flip".to_owned(), json!(true));
    }
    if cut.params.flip_side {
        m.insert("flip_side".to_owned(), json!(true));
    }
    m.insert(
        "components".to_owned(),
        match &cut.scope {
            None => json!("all"),
            Some(s) => json!(s.iter().map(|c| assembly.name_of(*c)).collect::<Vec<_>>()),
        },
    );
    m.insert("entities".to_owned(), json!(cut.sketch.entities().count()));
    let converted: Vec<Value> = cut
        .conversions
        .iter()
        .map(|c| {
            let mut v = end_out(assembly, &c.edge);
            if let Some(e) = c.entity {
                v["entity"] = json!(e.0);
            }
            v
        })
        .collect();
    if !converted.is_empty() {
        m.insert("converted".to_owned(), json!(converted));
    }
    // The plane where it was last worked out.
    m.insert(
        "plane".to_owned(),
        json!({
            "origin": point3_out(cut.placement.origin(), units),
            "normal": crate::args::direction_out(cut.placement.normal()),
        }),
    );
    if cut.suppressed {
        m.insert("suppressed".to_owned(), json!(true));
    }
    let status = doc.evaluation().cut_status(id);
    m.insert(
        "status".to_owned(),
        json!(match status {
            Some(Status::Failed(_)) => "failed",
            Some(Status::Warning(_)) => "warning",
            Some(Status::Suppressed) => "suppressed",
            Some(_) => "ok",
            None => "not_built",
        }),
    );
    if let Some(message) = status.and_then(Status::message) {
        m.insert("message".to_owned(), json!(message));
    }
    Value::Object(m)
}

/// The cuts that can't be worked out, or cut nothing, for `failures`.
pub(crate) fn failures(doc: &Document) -> Vec<Value> {
    let Some(assembly) = doc.model.assembly() else {
        return Vec::new();
    };
    assembly
        .cuts()
        .filter(|c| {
            doc.evaluation()
                .cut_status(c.id)
                .is_some_and(|s| s.message().is_some())
        })
        .map(|c| cut_out(doc, c.id))
        .collect()
}
