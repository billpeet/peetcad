//! In-context references, as SolidWorks' external references: a part's sketch, sketch
//! plane, reference plane or "up to" end following another component's geometry in an
//! assembly (`in_context`), listing them with how each stands (`external_references`), and
//! locking and breaking them (`lock_references`, `break_references`). The part can be a
//! component of the assembly or a part inside one of its sub-assemblies. See
//! [`peet_model::in_context`].

use std::path::PathBuf;
use std::sync::Arc;

use peet_document::Document;
use peet_math::{DVec3, Frame, Plane};
use peet_model::in_context::{
    External, Geom, Holder, RefStatus, Reference, change_at, context_key, geom_in_part, model_at,
    plane_like,
};
use peet_model::projection::{Projection, Source};
use peet_model::{
    Assembly, CompId, EndCondition, FeatureId, FeatureKind, MateEnd, MateGeom, Model, Output,
    PlaneRef,
};
use peet_sketch::{EntityId, Geometry};
use serde_json::{Map, Value, json};

use crate::args::{Args, integer, point3_out};
use crate::assembly::CompSel;
use crate::mate::MateEndSel;
use crate::op::Op;
use crate::value::FeatureSel;

/// The operations of this module: where they work, name, fields and what they do, for `help`.
pub(crate) const IN_CONTEXT_OPS: &[(&str, &str, &str, &str)] = &[
    (
        "an assembly",
        "in_context",
        "component (the part that follows: a component's name or id, or a path [sub-assembly, ..., part] to a part inside a sub-assembly), sketch (a sketch of its part, by name or id: an entity of it follows) or feature (a feature of its part: its plane follows; a sketch's plane, a reference plane's base plane, or an extrude's or cut's end, which becomes up to it), from (what it follows: an end as a mate's on another component, {component: name or path, face|edge|vertex|plane: selector in that part's coordinates} or {component, sketch, entity} or {component, origin: true}; or the assembly's own {plane: top|front|right} or {origin: true}), entity (with sketch: an entity of the sketch to make follow it instead of adding one: its id), construction (with sketch; default true; false: a converted entity, part of the sketch's profiles)",
        "Make a part follow other geometry in this assembly (SolidWorks' in-context reference, made by editing the part in the assembly): with `sketch`, what `from` is seen as on the sketch's plane is a fixed entity of the sketch (Convert Entities, or a relation to another part); with `feature`, the feature's plane is `from`'s flat face or plane (a sketch on another part's face, a plane on it, an extrude up to it). This assembly moves it wherever that geometry goes. Opened alone, or in another assembly, the part keeps it where it last was.",
    ),
    (
        "a part or an assembly",
        "external_references",
        "component (in an assembly: one component's part, or a path to a part or sub-assembly in a sub-assembly; default all)",
        "List the in-context references: each feature, what holds it (a sketch entity, or the feature's plane), what it follows in which assembly, and its status: in_context (worked out by this assembly), out_of_context (kept where it last was: the part alone, another assembly, or another instance of the part), locked, broken (a plane whose reference is broken), or dangling (what it follows is gone; with why). Parts inside sub-assemblies are listed too, with their path.",
    ),
    (
        "a part or an assembly",
        "lock_references",
        "component (in an assembly: whose part, by name, id or path; default every part with references), feature (a feature; default all of the part's), on (default true; false unlocks)",
        "Lock in-context references (SolidWorks' Lock): they keep where they are in their assembly too, until unlocked.",
    ),
    (
        "a part or an assembly",
        "break_references",
        "component (in an assembly: whose part, by name, id or path; default every part with references), feature (a feature; default all of the part's)",
        "Break in-context references (SolidWorks' Break): each followed sketch entity stays where it is as plain fixed sketch geometry, and each followed plane stays where it is, for good.",
    ),
];

/// An in-context operation.
#[derive(Clone, Debug, PartialEq)]
pub enum InContextOp {
    Make {
        component: Vec<CompSel>,
        /// The sketch an entity of follows, or the feature whose plane follows.
        target: MadeIn,
        from: Box<MateEndSel>,
        entity: Option<u32>,
        construction: bool,
    },
    List {
        component: Option<Vec<CompSel>>,
    },
    Lock {
        component: Option<Vec<CompSel>>,
        feature: Option<FeatureSel>,
        on: bool,
    },
    Break {
        component: Option<Vec<CompSel>>,
        feature: Option<FeatureSel>,
    },
}

/// What of the part follows: an entity of a sketch, or a feature's plane.
#[derive(Clone, Debug, PartialEq)]
pub enum MadeIn {
    Sketch(FeatureSel),
    Plane(FeatureSel),
}

impl InContextOp {
    pub(crate) fn word(&self) -> &'static str {
        match self {
            Self::Make { .. } => "in_context",
            Self::List { .. } => "external_references",
            Self::Lock { .. } => "lock_references",
            Self::Break { .. } => "break_references",
        }
    }
}

/// A component, or a path of them down into sub-assemblies.
pub(crate) fn parse_path(v: &Value) -> Result<Vec<CompSel>, String> {
    match v {
        Value::Array(path) if !path.is_empty() => path.iter().map(CompSel::parse).collect(),
        Value::Array(_) => Err("A component path can't be empty.".to_owned()),
        _ => Ok(vec![CompSel::parse(v)?]),
    }
}

/// Reads the operation called `op`, if it is one of this module's.
pub(crate) fn parse(op: &str, a: &mut Args) -> Option<Result<Op, String>> {
    let component = |a: &mut Args| a.parsed("component", parse_path);
    let feature = |a: &mut Args| a.parsed("feature", FeatureSel::parse);
    Some(
        match op {
            "in_context" => (|| {
                let component = a.required("component", parse_path)?;
                let target = match (
                    a.parsed("sketch", FeatureSel::parse)?,
                    a.parsed("feature", FeatureSel::parse)?,
                ) {
                    (Some(s), None) => MadeIn::Sketch(s),
                    (None, Some(f)) => MadeIn::Plane(f),
                    _ => {
                        return Err(
                            "'in_context' needs 'sketch' (an entity of that sketch follows) or 'feature' (that feature's plane follows), not both."
                                .to_owned(),
                        );
                    }
                };
                let entity = a.parsed("entity", integer)?;
                let construction = a.flag("construction", true)?;
                if matches!(target, MadeIn::Plane(_)) && entity.is_some() {
                    return Err("'entity' is for 'sketch': with 'feature', the feature's plane follows.".to_owned());
                }
                Ok(InContextOp::Make {
                    component,
                    target,
                    from: Box::new(a.required("from", MateEndSel::parse)?),
                    entity,
                    construction,
                })
            })(),
            "external_references" => component(a).map(|component| InContextOp::List { component }),
            "lock_references" => (|| {
                Ok(InContextOp::Lock {
                    component: component(a)?,
                    feature: feature(a)?,
                    on: a.flag("on", true)?,
                })
            })(),
            "break_references" => (|| {
                Ok(InContextOp::Break {
                    component: component(a)?,
                    feature: feature(a)?,
                })
            })(),
            _ => return None,
        }
        .map(Op::InContext),
    )
}

fn the_assembly(doc: &Document) -> Result<&Assembly, String> {
    doc.model
        .assembly()
        .ok_or_else(|| format!("{} is a part: it has no components.", doc.title()))
}

/// A path of components from the assembly's own, with their names.
pub(crate) fn resolve_path(
    doc: &Document,
    sels: &[CompSel],
) -> Result<(Vec<CompId>, Vec<String>), String> {
    let mut path = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut at: Option<Arc<Model>> = None;
    for sel in sels {
        let assembly = match &at {
            None => the_assembly(doc)?,
            Some(m) => m.assembly().ok_or_else(|| {
                format!(
                    "{} is a part, not a sub-assembly: the component path cannot continue inside it.",
                    names.join(" / ")
                )
            })?,
        };
        let id = sel.resolve_in(assembly).map_err(|e| {
            if names.is_empty() {
                e
            } else {
                format!("In component path {}: {e}", names.join(" / "))
            }
        })?;
        names.push(assembly.name_of(id).to_owned());
        path.push(id);
        at = Some(
            assembly
                .component_model(id)
                .ok_or("The component's part is missing.")?,
        );
    }
    Ok((path, names))
}

/// A feature of a part by name or id.
fn feature_in(part: &Model, sel: &FeatureSel) -> Result<FeatureId, String> {
    match sel {
        FeatureSel::Id(id) => part
            .feature(*id)
            .map(|f| f.id)
            .ok_or_else(|| format!("{} has no feature with the id {}.", part.name, id.0)),
        FeatureSel::Name(name) => {
            let mut found = part.features().filter(|f| f.name == *name);
            match (found.next(), found.next()) {
                (Some(f), None) => Ok(f.id),
                (Some(_), Some(_)) => Err(format!(
                    "Several features of {} are called '{name}': give its id.",
                    part.name
                )),
                _ => Err(format!(
                    "{} has no feature called '{name}'. Its sketches are: {}.",
                    part.name,
                    part.features()
                        .filter(|f| f.sketch().is_some())
                        .map(|f| f.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }
    }
}

/// The names along a path of components, from the assembly's own.
fn names_of(assembly: &Assembly, path: &[CompId]) -> Vec<String> {
    let mut names = Vec::new();
    let mut at: Option<Arc<Model>> = None;
    for id in path {
        let a = match &at {
            None => assembly,
            Some(m) => match m.assembly() {
                Some(a) => a,
                None => break,
            },
        };
        names.push(a.name_of(*id).to_owned());
        at = a.component_model(*id);
    }
    names
}

/// Where `target` is, in the coordinates of the part at `own`, as the assembly was last
/// rebuilt.
fn geometry(doc: &Document, own: &[CompId], target: &MateEnd) -> Result<Geom, String> {
    let geom = target.geom.as_ref().ok_or(
        "'from' names a whole component: give the face, edge, vertex, plane or sketch entity to follow.",
    )?;
    let eval = doc.evaluation();
    let at = |p: &[CompId]| -> Result<Frame, String> {
        eval.component_frame(p)
            .ok_or_else(|| "A component it involves is suppressed or not placed.".to_owned())
    };
    let own = at(own)?;
    // The assembly's own planes and origin, where the assembly's origin is.
    if target.is_assembly() {
        let there = match geom {
            MateGeom::Plane(PlaneRef::Standard(p)) => Geom::Plane(p.plane()),
            MateGeom::Origin => Geom::Point(DVec3::ZERO),
            _ => {
                return Err(
                    "The assembly itself has only its standard planes and its origin.".to_owned(),
                );
            }
        };
        return Ok(there.moved(&own.inverse()));
    }
    let assembly = the_assembly(doc)?;
    let part = names_of(assembly, &target.path);
    let part_model = model_at(&doc.model, &target.path)
        .ok_or_else(|| format!("{} has no part.", part.join(" / ")))?;
    if part_model.is_assembly() {
        return Err("It follows a sub-assembly: give the path down to a part in it.".to_owned());
    }
    let built = Document::from_model((*part_model).clone(), None);
    let evaluation = built.evaluation();
    let plane_of = |r: &PlaneRef| -> Option<Plane> {
        match r {
            PlaneRef::Standard(p) => Some(p.plane()),
            PlaneRef::Feature(id) => match evaluation.output(*id) {
                Output::Plane(p) => Some(p),
                Output::Frame(frame) => Some(Plane { frame }),
                _ => None,
            },
            PlaneRef::Face(_) | PlaneRef::External(_) => None,
        }
    };
    let local = geom_in_part(&built.model, &evaluation.bodies, &plane_of, geom)?;
    let target_frame = at(&target.path)?;
    Ok(local.moved(&own.inverse().compose(&target_frame)))
}

/// Whether an entity of a sketch is of the kind a projection of `shape` makes.
fn same_kind(geometry: &Geometry, geom: &Geom, plane: Plane) -> bool {
    use peet_model::projection::Shape;
    matches!(
        (geom.project(plane), geometry),
        (Ok(Shape::Point(_)), Geometry::Point { .. })
            | (Ok(Shape::Line(..)), Geometry::Line { .. })
            | (Ok(Shape::Circle(..)), Geometry::Circle { .. })
            | (Ok(Shape::Arc(..)), Geometry::Arc { .. })
    )
}

/// Makes an entity of `sketch` follow `last` (`reference` built around it). Returns the entity.
fn follow_entity(
    part: &mut Model,
    fid: FeatureId,
    entity: Option<u32>,
    construction: bool,
    last: Geom,
    reference: impl FnOnce(Geom) -> External,
    name: &str,
) -> Result<EntityId, String> {
    let s = part
        .feature_mut(fid)
        .and_then(peet_model::Feature::sketch_mut)
        .ok_or_else(|| format!("That feature of {name}'s part is not a sketch: give 'feature' to make its plane follow."))?;
    let shape = last.project(s.placement)?;
    let entity = match entity {
        None => shape.add(&mut s.sketch, construction),
        Some(id) => {
            let id = EntityId(id);
            let existing = s.sketch.entity(id).ok_or_else(|| {
                format!(
                    "The sketch has no entity {}: 'feature' with the sketch lists them.",
                    id.0
                )
            })?;
            if !same_kind(&existing.geometry, &last, s.placement) {
                return Err(format!(
                    "Entity {} is not what the {} it follows is seen as on the sketch: give a {} of the sketch, or leave 'entity' out to add one.",
                    id.0,
                    last.word(),
                    match shape {
                        peet_model::projection::Shape::Point(_) => "point",
                        peet_model::projection::Shape::Line(..) => "line",
                        peet_model::projection::Shape::Circle(..) => "circle",
                        peet_model::projection::Shape::Arc(..) => "arc",
                    }
                ));
            }
            shape.update(&mut s.sketch, id)?;
            let mut ids = s
                .sketch
                .entity(id)
                .map(|e| e.geometry.points())
                .unwrap_or_default();
            ids.push(id);
            // What held it where it was gives way to what it follows now.
            let fixes: Vec<_> = s
                .sketch
                .constraints()
                .filter(|(_, k)| {
                    matches!(k.kind, peet_sketch::ConstraintKind::Fix { point, .. } if ids.contains(&point))
                })
                .map(|(k, _)| k)
                .collect();
            for k in fixes {
                s.sketch.remove_constraint(k);
            }
            for e in ids {
                if let Some(e) = s.sketch.entity_mut(e) {
                    e.locked = true;
                }
            }
            id
        }
    };
    s.projections.retain(|p| p.entity != entity);
    s.projections.push(Projection {
        entity,
        source: Source::External(Box::new(reference(last))),
    });
    Ok(entity)
}

/// Makes the plane of feature `fid` follow `last`: a sketch's plane, a reference plane's
/// base, an extrusion's end (which becomes "up to" it).
fn follow_plane(
    part: &mut Model,
    fid: FeatureId,
    last: Geom,
    reference: impl FnOnce(Geom) -> External,
) -> Result<(), String> {
    if !matches!(last, Geom::Plane(_)) {
        return Err(format!(
            "A feature's plane follows a flat face or a plane: 'from' is seen as a {}.",
            last.word()
        ));
    }
    let name = part.name_of(fid).to_owned();
    let f = part.feature_mut(fid).ok_or("The feature is gone.")?;
    // A sketch keeps its place and side on the plane: only the plane changes.
    let last = match (&f.kind, last) {
        (FeatureKind::Sketch(s), Geom::Plane(p)) => Geom::Plane(plane_like(p, s.placement)),
        _ => last,
    };
    let plane = PlaneRef::External(Box::new(reference(last)));
    match &mut f.kind {
        FeatureKind::Extrude(e) => {
            e.params.end = EndCondition::UpTo(plane);
            Ok(())
        }
        kind => {
            let mut refs = kind.plane_refs_mut();
            match refs.len() {
                1 => {
                    *refs[0].1 = plane;
                    Ok(())
                }
                0 => Err(format!(
                    "{name} has no plane to follow another component: give a sketch, a reference plane, or an extrude or cut (its end becomes up to it)."
                )),
                _ => Err(format!(
                    "{name} is made from two planes: make a plane offset from the face instead, and use it."
                )),
            }
        }
    }
}

/// `in_context`: the part's path, the feature, what holds the reference and the undo label.
#[allow(clippy::too_many_arguments)]
pub(crate) fn make(
    doc: &Document,
    model: &mut Model,
    data: &mut Map<String, Value>,
    component: &[CompSel],
    target_in: &MadeIn,
    from: &MateEndSel,
    entity: Option<u32>,
    construction: bool,
) -> Result<(Vec<CompId>, FeatureId, Holder, String), String> {
    let assembly = the_assembly(doc)?;
    let (own, own_names) = resolve_path(doc, component)?;
    let name = own_names.join(" / ");
    let shown = model_at(&doc.model, &own).ok_or("The component's part is gone.")?;
    if shown.is_assembly() {
        return Err(format!(
            "{name} is a sub-assembly: an in-context reference belongs to a part. Give the path down to a part in it, as [\"{name}\", \"Part-1\"]."
        ));
    }
    // The definition's model, which is what changes.
    let part = change_at(&doc.model, &own, &mut |m| m.clone())
        .map(|(_, m)| m)
        .ok_or("The component's part is gone.")?;
    let mut part = part;
    let fid = match target_in {
        MadeIn::Sketch(s) | MadeIn::Plane(s) => feature_in(&part, s)?,
    };
    let target = from.resolve(doc)?;
    if target.path == own {
        return Err(format!(
            "'from' is on {name} itself: a sketch follows its own part's geometry with a sketch 'project', not in context."
        ));
    }
    let last = geometry(doc, &own, &target)?;
    let target_names = names_of(assembly, &target.path);
    let reference = |last: Geom| External {
        context: doc.title(),
        component: own.clone(),
        target: target.clone(),
        component_names: own_names.clone(),
        target_names: target_names.clone(),
        last,
        locked: false,
        broken: false,
    };
    let holder = match target_in {
        MadeIn::Sketch(_) => Holder::Entity(follow_entity(
            &mut part,
            fid,
            entity,
            construction,
            last,
            reference,
            &name,
        )?),
        MadeIn::Plane(_) => {
            follow_plane(&mut part, fid, last, reference)?;
            Holder::Plane(0)
        }
    };
    let feature_name = part.name_of(fid).to_owned();
    let (changed, _) = change_at(model, &own, &mut |m| *m = part.clone())
        .ok_or("The component's part is gone.")?;
    *model = changed;
    if own.len() == 1
        && let Some(def) = model
            .assembly()
            .and_then(|a| a.definition_of(own[0]))
            .map(|d| d.id)
        && let Some(file) = crate::equations::write_link(doc, model, def)?
    {
        data.insert("written".to_owned(), json!(file.display().to_string()));
    }
    match holder {
        Holder::Entity(e) => {
            data.insert("sketch".to_owned(), json!(feature_name));
            data.insert("entity".to_owned(), json!(e.0));
        }
        Holder::Plane(_) => {
            data.insert("plane_of".to_owned(), json!(feature_name));
        }
    }
    let follows = if target.is_assembly() {
        "the assembly".to_owned()
    } else {
        target_names.join(" / ")
    };
    let label = match holder {
        Holder::Entity(_) => {
            format!("In-context reference of {name}'s {feature_name} to {follows}")
        }
        Holder::Plane(_) => format!("In-context plane of {name}'s {feature_name} on {follows}"),
    };
    Ok((own, fid, holder, label))
}

fn status_of(doc: &Document, path: Option<&[CompId]>, r: &Reference) -> (String, Option<String>) {
    if let Some(path) = path
        && let Some(s) = doc
            .evaluation()
            .external
            .iter()
            .find(|s| s.path == path && s.feature == r.feature && s.holder == r.holder)
    {
        let why = match &s.status {
            RefStatus::Dangling(why) => Some(why.clone()),
            _ => None,
        };
        return (s.status.word().to_owned(), why);
    }
    (RefStatus::kept(r.external).word().to_owned(), None)
}

fn reference_out(doc: &Document, part: &Model, path: Option<&[CompId]>, r: &Reference) -> Value {
    let x = r.external;
    let (status, why) = status_of(doc, path, r);
    let units = &doc.model.parameters.units;
    let mut m = Map::new();
    if let Some(path) = path
        && let Some(a) = doc.model.assembly()
    {
        let names = names_of(a, path);
        m.insert("component".to_owned(), json!(names.join(" / ")));
        if path.len() > 1 {
            m.insert("path".to_owned(), json!(names));
        }
    }
    m.insert("feature".to_owned(), json!(part.name_of(r.feature)));
    m.insert("use".to_owned(), json!(r.role));
    if let Holder::Entity(e) = r.holder {
        m.insert("sketch".to_owned(), json!(part.name_of(r.feature)));
        m.insert("entity".to_owned(), json!(e.0));
    }
    m.insert("context".to_owned(), json!(x.context));
    m.insert("made_in".to_owned(), json!(x.component_names.join(" / ")));
    m.insert("follows".to_owned(), json!(x.target_names));
    m.insert(
        "what".to_owned(),
        json!(match &x.target.geom {
            Some(MateGeom::Face(_)) => "face",
            Some(MateGeom::Edge(_)) => "edge",
            Some(MateGeom::Vertex(_)) => "vertex",
            Some(MateGeom::Plane(_)) => "plane",
            Some(MateGeom::Origin) => "origin",
            Some(MateGeom::Sketch(_)) => "sketch_entity",
            None => "component",
        }),
    );
    if x.target.is_assembly() {
        m.insert("on_assembly".to_owned(), json!(true));
    }
    m.insert("geometry".to_owned(), json!(x.last.word()));
    let at = match x.last {
        Geom::Point(p) | Geom::Circle { center: p, .. } | Geom::Arc { center: p, .. } => p,
        Geom::Line(a, b) => (a + b) * 0.5,
        Geom::Plane(p) => p.origin(),
    };
    m.insert("at".to_owned(), point3_out(at, units));
    m.insert("status".to_owned(), json!(status));
    if let Some(why) = why {
        m.insert("why".to_owned(), json!(why));
    }
    Value::Object(m)
}

/// The parts below an assembly at any depth, by path, each with its model.
/// A part below an assembly, by its path of components, with its model.
type PartAt = (Vec<CompId>, Arc<Model>);

pub(crate) fn parts_below(model: &Model) -> Vec<PartAt> {
    let mut out = Vec::new();
    let Some(assembly) = model.assembly() else {
        return out;
    };
    for c in assembly.components() {
        let Some(d) = assembly.definition_of(c.id) else {
            continue;
        };
        if d.model.is_assembly() {
            for (mut path, m) in parts_below(&d.model) {
                path.insert(0, c.id);
                out.push((path, m));
            }
        } else {
            out.push((vec![c.id], d.model.clone()));
        }
    }
    out
}

/// The parts (by path) the component selection names: those at or below it, or all.
fn parts_of(doc: &Document, component: Option<&[CompSel]>) -> Result<Vec<PartAt>, String> {
    let only = component.map(|c| resolve_path(doc, c)).transpose()?;
    Ok(parts_below(&doc.model)
        .into_iter()
        .filter(|(path, _)| only.as_ref().is_none_or(|(o, _)| path.starts_with(o)))
        .collect())
}

/// `external_references`.
pub(crate) fn list(
    doc: &Document,
    component: Option<&[CompSel]>,
) -> Result<Map<String, Value>, String> {
    let mut out = Vec::new();
    match doc.model.assembly() {
        None => {
            if component.is_some() {
                return Err(format!(
                    "{} is a part: 'component' is for an assembly.",
                    doc.title()
                ));
            }
            for r in doc.model.external_references() {
                out.push(reference_out(doc, &doc.model, None, &r));
            }
        }
        Some(_) => {
            for (path, part) in parts_of(doc, component)? {
                for r in part.external_references() {
                    out.push(reference_out(doc, &part, Some(&path), &r));
                }
            }
        }
    }
    let mut m = Map::new();
    m.insert("references".to_owned(), json!(out));
    Ok(m)
}

/// `lock_references` and `break_references`: the undo label, or `None` if nothing changed.
pub(crate) fn change(
    doc: &Document,
    model: &mut Model,
    data: &mut Map<String, Value>,
    component: Option<&[CompSel]>,
    feature: Option<&FeatureSel>,
    lock: Option<bool>,
) -> Result<Option<String>, String> {
    let mut count = 0;
    let mut written: Vec<String> = Vec::new();
    let apply = |part: &mut Model| -> Result<usize, String> {
        let f = feature.map(|s| feature_in(part, s)).transpose()?;
        Ok(match lock {
            Some(on) => part.lock_external(f, on),
            None => part.break_external(f),
        })
    };
    if !doc.is_assembly() {
        if component.is_some() {
            return Err(format!(
                "{} is a part: 'component' is for an assembly.",
                doc.title()
            ));
        }
        count = apply(model)?;
    } else {
        // Each part once, however many instances it has.
        let mut done: Vec<*const Model> = Vec::new();
        let given = component.is_some();
        for (path, part) in parts_of(doc, component)? {
            if done.contains(&Arc::as_ptr(&part))
                || (!given && part.external_references().is_empty())
            {
                continue;
            }
            done.push(Arc::as_ptr(&part));
            let mut failed = None;
            let Some((changed, n)) = change_at(model, &path, &mut |m| match apply(m) {
                Ok(n) => n,
                Err(e) => {
                    failed = Some(e);
                    0
                }
            }) else {
                continue;
            };
            if let Some(e) = failed {
                return Err(e);
            }
            if n == 0 {
                continue;
            }
            count += n;
            *model = changed;
            if path.len() == 1
                && let Some(def) = model
                    .assembly()
                    .and_then(|a| a.definition_of(path[0]))
                    .map(|d| d.id)
                && let Some(file) = crate::equations::write_link(doc, model, def)?
            {
                written.push(file.display().to_string());
            }
        }
    }
    data.insert("changed".to_owned(), json!(count));
    if !written.is_empty() {
        data.insert("written".to_owned(), json!(written));
    }
    Ok((count > 0).then(|| {
        match lock {
            Some(true) => "Lock references",
            Some(false) => "Unlock references",
            None => "Break references",
        }
        .to_owned()
    }))
}

/// Writes each linked part of the assembly whose in-context references this assembly
/// moved, as SolidWorks saves the parts an assembly changed with it. Returns the files
/// written.
pub(crate) fn write_moved_links(doc: &Document) -> Result<Vec<PathBuf>, String> {
    let Some(assembly) = doc.model.assembly() else {
        return Ok(Vec::new());
    };
    let here = context_key(&doc.title());
    let mut out = Vec::new();
    for d in assembly.definitions() {
        let Some(link) = &d.link else {
            continue;
        };
        if !d
            .model
            .external_references()
            .iter()
            .any(|r| context_key(&r.external.context) == here)
        {
            continue;
        }
        crate::links::unavailable()?;
        let path = crate::links::find(link, crate::links::folder(doc).as_deref())
            .unwrap_or_else(|| PathBuf::from(&link.path));
        let bytes = peet_io::document::save(
            &d.model.for_file(path.parent()),
            &peet_io::document::Metadata::default(),
            None,
        )
        .map_err(|e| e.message)?;
        // Only when it differs from what the file holds.
        if std::fs::read(&path).ok().as_deref() == Some(bytes.as_slice()) {
            continue;
        }
        peet_platform::write_file(&path, &bytes)?;
        out.push(path);
    }
    Ok(out)
}

/// The reference an `in_context` made, after the rebuild: for its reply.
pub(crate) fn made_out(doc: &Document, path: &[CompId], f: FeatureId, holder: Holder) -> Value {
    let Some(part) = model_at(&doc.model, path) else {
        return Value::Null;
    };
    part.external_references()
        .into_iter()
        .find(|r| r.feature == f && r.holder == holder)
        .map_or(Value::Null, |r| reference_out(doc, &part, Some(path), &r))
}
