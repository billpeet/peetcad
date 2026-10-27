//! Operations on the mates of an assembly.
//!
//! A mate's end is a component path and geometry of its part: a face, an edge, a vertex
//! or a reference plane,
//! described by the same selectors a part's operations take, in the part's own
//! coordinates (not where the component is in the assembly). A description is resolved
//! against the part as it is now and stored as a persistent reference, so the mate
//! follows its face through edits to the part.

use std::sync::Arc;

use peet_document::Document;
use peet_math::Frame;
use peet_model::{
    Align, Assembly, CompId, Mate, MateEnd, MateGeom, MateId, MateKind, Model, ScalarKind, Status,
    WidthMode,
};
use serde_json::{Map, Value, json};

use crate::args::{Args, boolean, integer, text};
use crate::assembly::CompSel;
use crate::fields::scalar_out;
use crate::op::Op;
use crate::select;
use crate::value::{FeatureSel, GeomSel, Input, PlaneSel};

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
    /// As a script writes it: a component path, and geometry, a plane, the origin or a
    /// sketch entity of its part (none for a fasten mate, which holds the component as
    /// a whole). With no component, the assembly's own standard plane or origin.
    Find {
        component: Vec<CompSel>,
        geom: Option<GeomSel>,
        plane: Option<PlaneSel>,
        origin: bool,
        /// A sketch of the part, and the id of a point, line, circle or arc in it.
        sketch: Option<(FeatureSel, u32)>,
    },
    /// Already resolved: what the application passes after a click.
    Ref(MateEnd),
}

impl MateEndSel {
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        let wrong = || {
            format!(
                "expected {{\"component\": name or [names or ids along a path], \"face\": selector}} (or \"edge\", \"vertex\", \"plane\", \"origin\": true, or \"sketch\" with \"entity\"; or the component alone, for fasten; or, with no component, the assembly's own \"plane\" or \"origin\"), not {v}"
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
                    origin: false,
                    sketch: None,
                });
            }
            _ => return Err(wrong()),
        };
        let component = match map.remove("component") {
            // The assembly itself.
            None => Vec::new(),
            Some(Value::Array(path)) if !path.is_empty() => path
                .iter()
                .map(CompSel::parse)
                .collect::<Result<Vec<_>, _>>()?,
            Some(Value::Array(_)) => return Err("A component path can't be empty.".to_owned()),
            Some(value) => vec![CompSel::parse(&value)?],
        };
        let plane = map
            .remove("plane")
            .map(|v| PlaneSel::parse(&v))
            .transpose()?;
        let origin = map
            .remove("origin")
            .map(|v| boolean(&v))
            .transpose()?
            .unwrap_or(false);
        let sketch = match (map.remove("sketch"), map.remove("entity")) {
            (Some(s), Some(e)) => Some((FeatureSel::parse(&s)?, integer(&e)?)),
            (None, None) => None,
            _ => {
                return Err(
                    "A sketch end needs both 'sketch' (the sketch's name or id) and 'entity' (the id of a point, line, circle or arc in it, as 'feature' lists them)."
                        .to_owned(),
                );
            }
        };
        let chosen =
            usize::from(plane.is_some()) + usize::from(origin) + usize::from(sketch.is_some());
        if chosen > 1 || (chosen == 1 && !map.is_empty()) {
            return Err(wrong());
        }
        let geom = match map.len() {
            0 => None,
            1 => Some(GeomSel::parse(&Value::Object(map))?),
            _ => return Err(wrong()),
        };
        if component.is_empty() && (geom.is_some() || sketch.is_some() || chosen == 0) {
            return Err(format!(
                "An end with no 'component' is on the assembly itself, which has only its standard planes and its origin: give \"plane\" (top, front or right) or \"origin\": true, or name a component. Not {v}"
            ));
        }
        Ok(Self::Find {
            component,
            geom,
            plane,
            origin,
            sketch,
        })
    }

    /// The end as a mate stores it, with the component it is on.
    pub(crate) fn resolve(&self, doc: &Document) -> Result<MateEnd, String> {
        let (component, geom, plane, origin, sketch) = match self {
            Self::Ref(end) => return Ok(end.clone()),
            Self::Find {
                component,
                geom,
                plane,
                origin,
                sketch,
            } => (component, geom, plane, *origin, sketch),
        };
        if component.is_empty() {
            // The assembly's own: fixed, where the assembly's origin is.
            the_assembly(doc)?;
            return match plane {
                _ if origin => Ok(MateEnd::of_assembly(MateGeom::Origin)),
                Some(PlaneSel::Standard(p)) => Ok(MateEnd::of_assembly(MateGeom::Plane(
                    peet_model::PlaneRef::Standard(*p),
                ))),
                _ => Err(
                    "The assembly itself has only its standard planes (top, front, right) and its origin: name a component for any other plane."
                        .to_owned(),
                ),
            };
        }
        let mut path = Vec::new();
        let mut names: Vec<String> = Vec::new();
        // Each component's part (or sub-assembly) in the configuration it shows.
        let mut definition: Option<Arc<Model>> = None;
        for sel in component {
            let assembly = match &definition {
                None => the_assembly(doc)?,
                Some(model) => model.assembly().ok_or_else(|| format!("{} is a part, not a sub-assembly: the component path cannot continue inside it.", names.join(" / ")))?,
            };
            let id = sel
                .resolve_in(assembly)
                .map_err(|e| format!("In component path {}: {e}", names.join(" / ")))?;
            names.push(assembly.name_of(id).to_owned());
            path.push(id);
            let shown = assembly
                .component_model(id)
                .ok_or_else(|| "The component's part is missing.".to_owned())?;
            definition = Some(shown);
        }
        let definition = definition.ok_or_else(|| "A component path can't be empty.".to_owned())?;
        if geom.is_some() && plane.is_some() {
            return Err("A mate end takes one geometry selector or one plane selector.".to_owned());
        }
        if origin {
            return Ok(MateEnd {
                path,
                geom: Some(MateGeom::Origin),
            });
        }
        if geom.is_none() && plane.is_none() && sketch.is_none() {
            return Ok(MateEnd { path, geom: None });
        }
        let name = names.join(" / ");
        let part = Document::from_model((*definition).clone(), None);
        let on = |e: String| format!("In {name} (its part, {}): {e}", definition.name);
        if let Some((sel, entity)) = sketch {
            if definition.is_assembly() {
                return Err(format!(
                    "{name} is a sub-assembly: extend the component path to a part inside it to select one of its sketches."
                ));
            }
            let feature = sel.resolve(&part).map_err(on)?;
            let s = part
                .model
                .feature(feature)
                .and_then(peet_model::Feature::sketch)
                .ok_or_else(|| on("that feature is not a sketch.".to_owned()))?;
            let id = peet_sketch::EntityId(*entity);
            match s.sketch.entity(id).map(|e| &e.geometry) {
                Some(
                    peet_sketch::Geometry::Point { .. }
                    | peet_sketch::Geometry::Line { .. }
                    | peet_sketch::Geometry::Circle { .. }
                    | peet_sketch::Geometry::Arc { .. },
                ) => {}
                Some(_) => {
                    return Err(on(format!(
                        "entity {entity} is a spline or text: a mate takes a sketch point, line, circle or arc."
                    )));
                }
                None => {
                    return Err(on(format!(
                        "the sketch has no entity {entity}: 'feature' with the sketch lists its entities and their ids."
                    )));
                }
            }
            return Ok(MateEnd {
                path,
                geom: Some(MateGeom::Sketch(peet_model::SketchRef {
                    feature,
                    entity: id,
                })),
            });
        }
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
                if definition.is_assembly() {
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
    /// A pin in a slot: where along it.
    Slot(WidthModeSel),
    /// An angle (degrees) or a distance kept between two values.
    LimitAngle {
        min: Input,
        max: Input,
    },
    LimitDistance {
        min: Input,
        max: Input,
    },
    /// The two ends mirror images in this plane.
    Symmetric(Box<MateEndSel>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum WidthModeSel {
    Centered,
    Free,
    Dimension(Input),
    Percent(Input),
}

impl WidthModeSel {
    /// A width mate's mode (centred unless said), or a slot mate's (`slot`: free unless
    /// said).
    fn parse_for(a: &mut Args, slot: bool) -> Result<Self, String> {
        let mode = a.string("mode")?.unwrap_or_else(|| {
            if a.has("percent") {
                "percent"
            } else if a.has("distance") {
                "dimension"
            } else if slot {
                "free"
            } else {
                "centered"
            }
            .to_owned()
        });
        let what = if slot { "Slot" } else { "Width" };
        Ok(match mode.as_str() {
            "centered" | "center" => Self::Centered,
            "free" => Self::Free,
            "dimension" | "distance" => Self::Dimension(a.required("distance", Input::parse)?),
            "percent" => Self::Percent(a.required("percent", Input::parse)?),
            _ => {
                return Err(format!(
                    "{what} mode must be centered, free, dimension or percent."
                ));
            }
        })
    }

    fn parse(a: &mut Args) -> Result<Self, String> {
        Self::parse_for(a, false)
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

const TYPES: &str = "coincident, concentric, parallel, distance, angle, fasten, width, slot, limit_angle, limit_distance, symmetric";

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
            "slot" => Self::Slot(WidthModeSel::parse_for(a, true)?),
            "limit_angle" => Self::LimitAngle {
                min: value(a, "min")?,
                max: value(a, "max")?,
            },
            "limit_distance" => Self::LimitDistance {
                min: value(a, "min")?,
                max: value(a, "max")?,
            },
            "symmetric" => Self::Symmetric(Box::new(a.required("plane", MateEndSel::parse)?)),
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
    /// Its distance or angle, which way round its ends go, and which way its distance
    /// or angle is measured: the ones given.
    Edit {
        value: Option<Input>,
        flip: Option<bool>,
        align: Option<Align>,
        flip_dimension: Option<bool>,
    },
    Rename(String),
    /// Leave it out of the solve without deleting it, or put it back.
    Suppress(bool),
    Delete,
    /// A width or slot mate's mode and value.
    Width(WidthModeSel),
    /// A limit mate's least and greatest value: the ones given.
    Limits {
        min: Option<Input>,
        max: Option<Input>,
    },
}

impl MateChange {
    pub(crate) fn word(&self) -> &'static str {
        match self {
            Self::Edit { .. } | Self::Width(_) | Self::Limits { .. } => "edit_mate",
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
        "type (coincident, concentric, parallel, distance, angle, fasten, width, slot, limit_angle, limit_distance, symmetric), a, b (each {component: name, id or nonempty path array, and face, edge, vertex, plane, origin: true, or sketch: name or id with entity: id (a point, line, circle or arc); geometry in the part's own coordinates}; plane: top, front, right or a reference plane's name or id; with no component: the assembly's own plane (top, front, right) or origin: true; for fasten the component alone). Width: a is a pair of planar ends, b is a pair of planar ends or one axis end; mode (centered, free, dimension, percent; default centered), distance for dimension, percent for percent (number or expression). Slot: a and b are a face of a straight slot and an axis, either way round; mode (free, centered, dimension, percent; default free), distance, percent; flip_dimension measures from the slot's other end. limit_angle: min, max (degrees); limit_distance: min, max. Symmetric: plane (an end: the plane of symmetry). Other mates: distance, angle, flip (true: aligned), align (aligned, anti_aligned, closest; default: planes anti_aligned, axes closest), flip_dimension (bool: the distance or angle measured the other way). All: name, suppressed (bool: added without being solved)",
        "Hold two components together by geometry of their parts. The components move to where the mates hold, as little as they can.",
    ),
    (
        "edit_mate",
        "mate, distance or angle, flip, align (aligned, anti_aligned, closest), flip_dimension; width and slot mates: mode (centered, free, dimension, percent), distance for dimension, percent for percent; limit mates: min, max; configurations (of the assembly, for the value, mode and limits: \"this\", \"all\", a name or a list of names; left out: this one if they already differ between configurations, else all)",
        "Change a mate's value, which way round its ends go or which way its value is measured, a width or slot mate's mode and position, or a limit mate's range.",
    ),
    ("rename", "mate, name", "Rename a mate."),
    (
        "suppress",
        "mate, on (default true), configurations (of the assembly: \"this\" (default), \"all\", a name or a list of names)",
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
            configurations: a.parsed("configurations", crate::value::Configs::parse)?,
        })
    };
    Some(match op {
        "mate" => (|| {
            let (kind, first, second) = MateType::parse(a)?;
            let (flip, align) = orientation(a)?;
            Ok(Op::Mate {
                kind,
                a: first,
                b: second,
                flip,
                align,
                flip_dimension: a.parsed("flip_dimension", boolean)?,
                suppressed: a.flag("suppressed", false)?,
                name: a.string("name")?,
            })
        })(),
        "mates" => Ok(Op::Query(crate::op::Query::Mates)),
        "edit_mate" => {
            if a.has("mode") || a.has("percent") {
                let mode = WidthModeSel::parse(a).map(MateChange::Width);
                return Some(change(a, mode));
            }
            if a.has("min") || a.has("max") {
                let limits = (|| {
                    Ok(MateChange::Limits {
                        min: a.parsed("min", Input::parse)?,
                        max: a.parsed("max", Input::parse)?,
                    })
                })();
                return Some(change(a, limits));
            }
            let edit = (|| {
                let distance = a.parsed("distance", Input::parse)?;
                let angle = a.parsed("angle", Input::parse)?;
                let (flip, align) = orientation(a)?;
                let flip_dimension = a.parsed("flip_dimension", boolean)?;
                if distance.is_some() && angle.is_some() {
                    return Err("Give 'distance' or 'angle', not both.".to_owned());
                }
                let value = distance.or(angle);
                if value.is_none() && flip.is_none() && align.is_none() && flip_dimension.is_none()
                {
                    return Err(
                        "'edit_mate' needs a 'distance', an 'angle', 'flip', 'align' or 'flip_dimension' field."
                            .to_owned(),
                    );
                }
                Ok(MateChange::Edit {
                    value,
                    flip,
                    align,
                    flip_dimension,
                })
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

/// A mate's `flip` and `align`: one or the other.
fn orientation(a: &mut Args) -> Result<(Option<bool>, Option<Align>), String> {
    let flip = a.parsed("flip", boolean)?;
    let align = a.parsed("align", |v| {
        let word = text(v)?;
        Align::from_word(word).ok_or_else(|| {
            format!("align: '{word}' is not an alignment: use aligned, anti_aligned or closest.")
        })
    })?;
    if flip.is_some() && align.is_some() {
        return Err(
            "Give 'flip' or 'align', not both: 'flip': true is 'align': 'aligned'.".to_owned(),
        );
    }
    Ok((flip, align))
}

/// What a mate's `flip` and `align` fields ask for, if either is given.
pub(crate) fn align_of(flip: Option<bool>, align: Option<Align>) -> Option<Option<Align>> {
    match (flip, align) {
        (_, Some(align)) => Some(Some(align)),
        (Some(true), None) => Some(Some(Align::Aligned)),
        (Some(false), None) => Some(None),
        (None, None) => None,
    }
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
    let mut names = Vec::new();
    let mut model: Option<Arc<Model>> = None;
    for (i, id) in end.path.iter().enumerate() {
        let current = if i == 0 {
            Some(assembly)
        } else {
            model.as_deref().and_then(Model::assembly)
        };
        names.push(current.map_or("?", |a| a.name_of(*id)).to_owned());
        model = current.and_then(|a| a.component_model(*id));
    }
    if end.is_assembly() {
        m.insert("assembly".to_owned(), json!(true));
    } else {
        m.insert("component".to_owned(), json!(names.first()));
    }
    if let Some(MateGeom::Sketch(s)) = &end.geom {
        m.insert(
            "sketch".to_owned(),
            model
                .as_ref()
                .map_or_else(|| json!(s.feature.0), |m| json!(m.name_of(s.feature))),
        );
        m.insert("entity".to_owned(), json!(s.entity.0));
    }
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
            Some(MateGeom::Origin) => "origin",
            Some(MateGeom::Sketch(_)) => "sketch",
            None => "component",
        }),
    );
    Value::Object(m)
}

/// A mate's values, as `mates` gives them: its distance or angle, a limit's range, a
/// width or slot mate's mode.
pub(crate) fn kind_values(kind: &MateKind, doc: &Document, m: &mut Map<String, Value>) {
    match kind {
        MateKind::LimitAngle { min, max } => {
            m.insert("min".to_owned(), scalar_out(min, ScalarKind::Angle, doc));
            m.insert("max".to_owned(), scalar_out(max, ScalarKind::Angle, doc));
        }
        MateKind::LimitDistance { min, max } => {
            m.insert("min".to_owned(), scalar_out(min, ScalarKind::Length, doc));
            m.insert("max".to_owned(), scalar_out(max, ScalarKind::Length, doc));
        }
        MateKind::Width { mode, .. } | MateKind::Slot { mode } => {
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
    if let MateKind::Symmetric { plane } = &mate.kind {
        m.insert("plane".to_owned(), end_out(assembly, plane));
    }
    kind_values(&mate.kind, doc, &mut m);
    if mate.flip() {
        m.insert("flip".to_owned(), json!(true));
    }
    if let Some(align) = mate.align {
        m.insert("align".to_owned(), json!(align.word()));
    }
    if mate.flip_dimension {
        m.insert("flip_dimension".to_owned(), json!(true));
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

/// How a new mate is set: which way round, which way its value is measured, whether it
/// is suppressed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct MateSettings {
    pub flip: Option<bool>,
    pub align: Option<Align>,
    pub flip_dimension: Option<bool>,
    pub suppressed: bool,
}

/// Adds a mate. Returns it, with the undo label.
pub(crate) fn add(
    doc: &Document,
    model: &mut Model,
    kind: &MateType,
    a: &MateEndSel,
    b: &MateEndSel,
    settings: MateSettings,
    name: Option<&str>,
) -> Result<(MateId, String), String> {
    let align = align_of(settings.flip, settings.align).flatten();
    let (mut a, mut b) = (a.resolve(doc)?, b.resolve(doc)?);
    let assembly = the_assembly(doc)?;
    // The component an end is on, or none for the assembly's own plane or origin.
    let component = |end: &MateEnd| -> Result<Option<CompId>, String> {
        if end.is_assembly() {
            return Ok(None);
        }
        end.component()
            .filter(|c| assembly.component(*c).is_some())
            .map(Some)
            .ok_or_else(|| "A mate's component is not in the assembly.".to_owned())
    };
    let (ca, cb) = (component(&a)?, component(&b)?);
    for c in [ca, cb].into_iter().flatten() {
        crate::pattern::placed_by_pattern(assembly, c, "mated")?;
    }
    let symmetric = matches!(kind, MateType::Symmetric(_));
    match (ca, cb) {
        (None, None) => {
            return Err(
                "Both ends are on the assembly itself: a mate joins a component to something."
                    .to_owned(),
            );
        }
        (Some(ca), Some(cb)) if ca == cb && !symmetric => {
            return Err(format!(
                "Both ends are on {}: a mate joins two components.",
                assembly.name_of(ca)
            ));
        }
        _ => {}
    }
    let named = |end: &MateEnd| {
        end.component()
            .map_or("the assembly", |c| assembly.name_of(c))
            .to_owned()
    };
    let needs_geometry = |end: &MateEnd, which: &str| {
        if end.geom.is_none() {
            Err(format!(
                "'{which}' names {} alone: this mate needs a face, an edge, a vertex or a plane of it (only fasten takes a component as a whole).",
                named(end)
            ))
        } else {
            Ok(())
        }
    };
    let no_turn = |what: &str| {
        if align.is_some() {
            Err(format!(
                "A {what} mate keeps the nearest orientation and does not take flip or align."
            ))
        } else {
            Ok(())
        }
    };
    let kind = match kind {
        MateType::Width { a2, b2, mode } => {
            no_turn("width")?;
            if settings.flip_dimension == Some(true) {
                return Err(
                    "A width mate keeps the nearest orientation and does not take flip, align or flip_dimension: reverse the width pair to measure from its other face."
                        .to_owned(),
                );
            }
            let a2 = a2.resolve(doc)?;
            let b2 = b2.as_ref().map(|b| b.resolve(doc)).transpose()?;
            for end in [&a, &a2, &b].into_iter().chain(b2.as_ref()) {
                needs_geometry(end, "width/tab")?;
            }
            let on: Vec<Option<CompId>> = [&a, &a2, &b]
                .into_iter()
                .chain(b2.as_ref())
                .map(MateEnd::component)
                .collect();
            if on.iter().all(|c| *c == on[0]) {
                return Err("The width pair and the tab are all on one component: a mate joins two components.".to_owned());
            }
            MateKind::Width {
                a2,
                b2,
                mode: mode.resolve(doc)?,
            }
        }
        MateType::Fasten(relative) => {
            let (Some(ca), Some(cb)) = (ca, cb) else {
                return Err(
                    "A fasten mate holds two components: name a component at each end.".to_owned(),
                );
            };
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
        MateType::Slot(mode) => {
            no_turn("slot")?;
            needs_geometry(&a, "a")?;
            needs_geometry(&b, "b")?;
            MateKind::Slot {
                mode: mode.resolve(doc)?,
            }
        }
        MateType::Symmetric(plane) => {
            needs_geometry(&a, "a")?;
            needs_geometry(&b, "b")?;
            let plane = plane.resolve(doc)?;
            needs_geometry(&plane, "plane")?;
            if let Some(c) = component(&plane)? {
                crate::pattern::placed_by_pattern(assembly, c, "mated")?;
            }
            if ca == cb && cb == plane.component() {
                return Err(format!(
                    "Both ends and the plane are on {}: a mate joins two components.",
                    named(&a)
                ));
            }
            MateKind::Symmetric { plane }
        }
        other => {
            needs_geometry(&a, "a")?;
            needs_geometry(&b, "b")?;
            let scalar = |v: &Input, kind, what: &str| {
                v.scalar(kind, doc).map_err(|e| format!("{what}: {e}"))
            };
            match other {
                MateType::Coincident => MateKind::Coincident,
                MateType::Concentric => MateKind::Concentric,
                MateType::Parallel => MateKind::Parallel,
                MateType::Distance(v) => {
                    MateKind::Distance(scalar(v, ScalarKind::Length, "distance")?)
                }
                MateType::Angle(v) => MateKind::Angle(scalar(v, ScalarKind::Angle, "angle")?),
                MateType::LimitAngle { min, max } => MateKind::LimitAngle {
                    min: scalar(min, ScalarKind::Angle, "min")?,
                    max: scalar(max, ScalarKind::Angle, "max")?,
                },
                MateType::LimitDistance { min, max } => MateKind::LimitDistance {
                    min: scalar(min, ScalarKind::Length, "min")?,
                    max: scalar(max, ScalarKind::Length, "max")?,
                },
                MateType::Fasten(_)
                | MateType::Width { .. }
                | MateType::Slot(_)
                | MateType::Symmetric(_) => unreachable!("handled above"),
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
    if let Some(m) = assembly.mate_mut(id) {
        m.align = align;
        m.flip_dimension = settings.flip_dimension.unwrap_or(false);
        // Added as it is, without being solved: nothing moves until it is unsuppressed.
        m.suppressed = settings.suppressed;
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
    let _ = configurations;
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
    configurations: Option<&crate::value::Configs>,
) -> Result<String, String> {
    let kind = the_assembly(doc)?.mate(id).map(|m| &m.kind);
    let turns = matches!(
        change,
        MateChange::Edit { flip: Some(_), .. } | MateChange::Edit { align: Some(_), .. }
    );
    let measures = matches!(
        change,
        MateChange::Edit {
            flip_dimension: Some(_),
            ..
        }
    );
    if matches!(kind, Some(MateKind::Width { .. })) && (turns || measures) {
        return Err(
            "A width mate keeps the nearest orientation and does not take flip, align or flip_dimension."
                .to_owned(),
        );
    }
    if matches!(kind, Some(MateKind::Slot { .. })) && turns {
        return Err(
            "A slot mate keeps the nearest orientation and does not take flip or align.".to_owned(),
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
            let kind = match kind {
                Some(MateKind::Distance(_)) => ScalarKind::Length,
                Some(MateKind::Angle(_)) => ScalarKind::Angle,
                Some(
                    MateKind::Width {
                        mode: WidthMode::Dimension(_),
                        ..
                    }
                    | MateKind::Slot {
                        mode: WidthMode::Dimension(_),
                    },
                ) => ScalarKind::Length,
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
    let limits = match (change, kind) {
        (MateChange::Limits { min, max }, Some(MateKind::LimitAngle { .. })) => {
            Some((ScalarKind::Angle, min, max))
        }
        (MateChange::Limits { min, max }, Some(MateKind::LimitDistance { .. })) => {
            Some((ScalarKind::Length, min, max))
        }
        (MateChange::Limits { .. }, _) => {
            return Err(format!(
                "{was} is not a limit mate: only limit_angle and limit_distance take min and max."
            ));
        }
        _ => None,
    }
    .map(|(kind, min, max)| -> Result<_, String> {
        let read = |v: &Option<Input>| v.as_ref().map(|v| v.scalar(kind, doc)).transpose();
        Ok((read(min)?, read(max)?))
    })
    .transpose()?;
    if matches!(change, MateChange::Width(_))
        && !matches!(kind, Some(MateKind::Width { .. } | MateKind::Slot { .. }))
    {
        return Err("Only a width or slot mate takes mode or percent.".to_owned());
    }
    if configurations.is_some() && matches!(change, MateChange::Rename(_) | MateChange::Delete) {
        return Err(format!(
            "'{}' is the same in every configuration of the assembly: leave 'configurations' out.",
            change.word()
        ));
    }
    let scope =
        |default: crate::value::Configs| configurations.cloned().unwrap_or(default).resolve(doc);
    // What can differ between the assembly's configurations: the value, the mode and the
    // limits, which are in the mate's kind.
    let edit_kind = |kind: &mut MateKind| {
        match &value {
            Some((ScalarKind::Length, s)) => match &mut *kind {
                MateKind::Width { mode, .. } | MateKind::Slot { mode } => {
                    *mode = WidthMode::Dimension(s.clone());
                }
                other => *other = MateKind::Distance(s.clone()),
            },
            Some((_, s)) => *kind = MateKind::Angle(s.clone()),
            None => {}
        }
        if let (Some(new), MateKind::Width { mode, .. } | MateKind::Slot { mode }) =
            (&width_mode, &mut *kind)
        {
            *mode = new.clone();
        }
        if let (
            Some((new_min, new_max)),
            MateKind::LimitAngle { min, max } | MateKind::LimitDistance { min, max },
        ) = (&limits, kind)
        {
            if let Some(s) = new_min {
                *min = s.clone();
            }
            if let Some(s) = new_max {
                *max = s.clone();
            }
        }
    };
    let gone = || "The mate no longer exists.".to_owned();
    Ok(match change {
        MateChange::Edit {
            flip,
            align,
            flip_dimension,
            ..
        } => {
            if value.is_some() {
                let differs = model.mate_differs(id);
                let scope = scope(if differs {
                    crate::value::Configs::This
                } else {
                    crate::value::Configs::All
                })?;
                model.set_mate(id, &scope, |s| edit_kind(&mut s.kind))?;
            }
            let assembly = model
                .assembly_mut()
                .ok_or_else(|| "The document is not an assembly.".to_owned())?;
            let mate = assembly.mate_mut(id).ok_or_else(gone)?;
            if let Some(align) = align_of(*flip, *align) {
                mate.align = align;
            }
            if let Some(flip_dimension) = flip_dimension {
                mate.flip_dimension = *flip_dimension;
            }
            format!("Edit {was}")
        }
        MateChange::Rename(name) => {
            let assembly = model
                .assembly_mut()
                .ok_or_else(|| "The document is not an assembly.".to_owned())?;
            rename(assembly, id, name)?;
            format!("Rename {was}")
        }
        MateChange::Suppress(on) => {
            let on = *on;
            model.set_mate(id, &scope(crate::value::Configs::This)?, |s| {
                s.suppressed = on;
            })?;
            format!("{} {was}", if on { "Suppress" } else { "Unsuppress" })
        }
        MateChange::Delete => {
            let assembly = model
                .assembly_mut()
                .ok_or_else(|| "The document is not an assembly.".to_owned())?;
            assembly.remove_mate(id).ok_or_else(gone)?;
            format!("Delete {was}")
        }
        MateChange::Width(_) | MateChange::Limits { .. } => {
            let differs = model.mate_differs(id);
            let scope = scope(if differs {
                crate::value::Configs::This
            } else {
                crate::value::Configs::All
            })?;
            model.set_mate(id, &scope, |s| edit_kind(&mut s.kind))?;
            format!("Edit {was}")
        }
    })
}
