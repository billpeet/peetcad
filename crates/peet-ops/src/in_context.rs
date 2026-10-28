//! In-context references, as SolidWorks' external references: a component's part's sketch
//! following another component's geometry in an assembly (`in_context`), listing them with
//! how each stands (`external_references`), and locking and breaking them
//! (`lock_references`, `break_references`). See [`peet_model::in_context`].

use std::path::PathBuf;
use std::sync::Arc;

use peet_document::Document;
use peet_math::{Frame, Plane};
use peet_model::in_context::{External, Geom, RefStatus, context_key, geom_in_part};
use peet_model::projection::{Projection, Source};
use peet_model::{Assembly, CompId, DefId, FeatureId, MateGeom, Model, Output, PlaneRef};
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
        "component (the component whose part's sketch follows: name or id), sketch (a sketch of its part: name or id), from (what it follows, on another component: an end as a mate's, {component: name or path, face|edge|vertex|plane: selector in that part's coordinates} or {component, sketch, entity} or {component, origin: true}), entity (an entity of the sketch to make follow it instead of adding one: its id), construction (default true; false: a converted entity, part of the sketch's profiles)",
        "Make a component's part's sketch follow another component's geometry in this assembly (SolidWorks' in-context reference, Convert Entities or a relation to another part): what `from` is, seen on the sketch's plane, is a fixed entity of the sketch that this assembly moves wherever that component's geometry goes. Opened alone, or in another assembly, the part keeps it where it last was.",
    ),
    (
        "a part or an assembly",
        "external_references",
        "component (in an assembly: one component's part; default all)",
        "List the in-context references: each sketch entity, what it follows in which assembly, and its status: in_context (worked out by this assembly), out_of_context (kept where it last was: the part alone, another assembly, or another instance of the part), locked, or dangling (what it follows is gone; with why).",
    ),
    (
        "a part or an assembly",
        "lock_references",
        "component (in an assembly: whose part; default every part with references), feature (a sketch; default all of the part's), on (default true; false unlocks)",
        "Lock in-context references (SolidWorks' Lock): they keep where they are in their assembly too, until unlocked.",
    ),
    (
        "a part or an assembly",
        "break_references",
        "component (in an assembly: whose part; default every part with references), feature (a sketch; default all of the part's)",
        "Break in-context references (SolidWorks' Break): each followed entity stays where it is as plain fixed sketch geometry, and follows nothing from then on.",
    ),
];

/// An in-context operation.
#[derive(Clone, Debug, PartialEq)]
pub enum InContextOp {
    Make {
        component: CompSel,
        sketch: FeatureSel,
        from: Box<MateEndSel>,
        entity: Option<u32>,
        construction: bool,
    },
    List {
        component: Option<CompSel>,
    },
    Lock {
        component: Option<CompSel>,
        feature: Option<FeatureSel>,
        on: bool,
    },
    Break {
        component: Option<CompSel>,
        feature: Option<FeatureSel>,
    },
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

/// Reads the operation called `op`, if it is one of this module's.
pub(crate) fn parse(op: &str, a: &mut Args) -> Option<Result<Op, String>> {
    let component = |a: &mut Args| a.parsed("component", CompSel::parse);
    let feature = |a: &mut Args| a.parsed("feature", FeatureSel::parse);
    Some(
        match op {
            "in_context" => (|| {
                Ok(InContextOp::Make {
                    component: a.required("component", CompSel::parse)?,
                    sketch: a.required("sketch", FeatureSel::parse)?,
                    from: Box::new(a.required("from", MateEndSel::parse)?),
                    entity: a.parsed("entity", integer)?,
                    construction: a.flag("construction", true)?,
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

/// The part (or sub-assembly) at the end of a path of components.
fn model_at(assembly: &Assembly, path: &[CompId]) -> Option<Arc<Model>> {
    let mut at: Option<Arc<Model>> = None;
    for id in path {
        let a = match &at {
            None => assembly,
            Some(m) => m.assembly()?,
        };
        at = Some(a.component_model(*id)?);
    }
    at
}

/// Where `geom` of the part at the end of `path` is, in the coordinates of component `c`'s
/// part, as the assembly was last rebuilt.
fn geometry(doc: &Document, c: CompId, path: &[CompId], geom: &MateGeom) -> Result<Geom, String> {
    let assembly = the_assembly(doc)?;
    let part = model_at(assembly, path).ok_or("The component it follows has no part.")?;
    if part.is_assembly() {
        return Err("It follows a sub-assembly: give the path down to a part in it.".to_owned());
    }
    let built = Document::from_model((*part).clone(), None);
    let evaluation = built.evaluation();
    let plane_of = |r: &PlaneRef| -> Option<Plane> {
        match r {
            PlaneRef::Standard(p) => Some(p.plane()),
            PlaneRef::Feature(id) => match evaluation.output(*id) {
                Output::Plane(p) => Some(p),
                Output::Frame(frame) => Some(Plane { frame }),
                _ => None,
            },
            PlaneRef::Face(_) => None,
        }
    };
    let local = geom_in_part(&built.model, &evaluation.bodies, &plane_of, geom)?;
    let eval = doc.evaluation();
    let at = |p: &[CompId]| -> Result<Frame, String> {
        eval.component_frame(p)
            .ok_or_else(|| "A component it involves is suppressed or not placed.".to_owned())
    };
    let target = at(path)?;
    let own = at(&[c])?;
    Ok(local.moved(&own.inverse().compose(&target)))
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

/// `in_context`: the changed assembly, the sketch, the entity, the part's definition and the
/// undo label.
#[allow(clippy::too_many_arguments)]
pub(crate) fn make(
    doc: &Document,
    model: &mut Model,
    data: &mut Map<String, Value>,
    component: &CompSel,
    sketch: &FeatureSel,
    from: &MateEndSel,
    entity: Option<u32>,
    construction: bool,
) -> Result<(CompId, FeatureId, EntityId, String), String> {
    let assembly = the_assembly(doc)?;
    let c = component.resolve(doc)?;
    let name = assembly.name_of(c).to_owned();
    let def = assembly
        .definition_of(c)
        .ok_or("The component's part is gone.")?;
    let def_id = def.id;
    if def.model.is_assembly() {
        return Err(format!(
            "{name} is a sub-assembly: an in-context reference belongs to a part's sketch. Give a part of this assembly."
        ));
    }
    let mut part = (*def.model).clone();
    let fid = feature_in(&part, sketch)?;
    let target = from.resolve(doc)?;
    if target.component() == Some(c) {
        return Err(format!(
            "'from' is on {name} itself: a sketch follows its own part's geometry with a sketch 'project', not in context."
        ));
    }
    let geom = target.geom.clone().ok_or(
        "'from' names a whole component: give the face, edge, vertex, plane or sketch entity to follow.",
    )?;
    if target.is_assembly() {
        return Err(
            "'from' is the assembly's own plane or origin: a part follows another component's geometry. Mate the part to the assembly's planes instead."
                .to_owned(),
        );
    }
    let last = geometry(doc, c, &target.path, &geom)?;
    let s = part
        .feature_mut(fid)
        .and_then(peet_model::Feature::sketch_mut)
        .ok_or_else(|| format!("That feature of {name}'s part is not a sketch."))?;
    let shape = last.project(s.placement)?;
    let entity = match entity {
        None => shape.add(&mut s.sketch, construction),
        Some(id) => {
            let id = EntityId(id);
            let existing = s.sketch.entity(id).ok_or_else(|| {
                format!("The sketch has no entity {}: 'feature' with the sketch lists them.", id.0)
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
            let mut ids = s.sketch.entity(id).map(|e| e.geometry.points()).unwrap_or_default();
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
        source: Source::External(Box::new(External {
            context: doc.title(),
            component: vec![c],
            target: target.clone(),
            component_names: vec![name.clone()],
            target_names: names_of(assembly, &target.path),
            last,
            locked: false,
        })),
    });
    let sketch_name = part.name_of(fid).to_owned();
    model
        .assembly_mut()
        .expect("an assembly")
        .set_model(def_id, Arc::new(part));
    if let Some(file) = crate::equations::write_link(doc, model, def_id)? {
        data.insert("written".to_owned(), json!(file.display().to_string()));
    }
    data.insert("sketch".to_owned(), json!(sketch_name));
    data.insert("entity".to_owned(), json!(entity.0));
    Ok((
        c,
        fid,
        entity,
        format!(
            "In-context reference of {name}'s {sketch_name} to {}",
            names_of(assembly, &target.path).join(" / ")
        ),
    ))
}

fn status_of(doc: &Document, c: Option<CompId>, f: FeatureId, e: EntityId, x: &External) -> (String, Option<String>) {
    if let Some(c) = c
        && let Some(r) = doc
            .evaluation()
            .external
            .iter()
            .find(|r| r.component == c && r.feature == f && r.entity == e)
    {
        let why = match &r.status {
            RefStatus::Dangling(why) => Some(why.clone()),
            _ => None,
        };
        return (r.status.word().to_owned(), why);
    }
    let word = if x.locked {
        RefStatus::Locked.word()
    } else {
        RefStatus::OutOfContext.word()
    };
    (word.to_owned(), None)
}

fn reference_out(
    doc: &Document,
    part: &Model,
    c: Option<CompId>,
    f: FeatureId,
    e: EntityId,
    x: &External,
) -> Value {
    let (status, why) = status_of(doc, c, f, e, x);
    let units = &doc.model.parameters.units;
    let mut m = Map::new();
    if let Some(c) = c
        && let Some(a) = doc.model.assembly()
    {
        m.insert("component".to_owned(), json!(a.name_of(c)));
    }
    m.insert("sketch".to_owned(), json!(part.name_of(f)));
    m.insert("entity".to_owned(), json!(e.0));
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

/// `external_references`.
pub(crate) fn list(doc: &Document, component: Option<&CompSel>) -> Result<Map<String, Value>, String> {
    let mut out = Vec::new();
    match doc.model.assembly() {
        None => {
            if component.is_some() {
                return Err(format!(
                    "{} is a part: 'component' is for an assembly.",
                    doc.title()
                ));
            }
            for (f, e, x) in doc.model.external_references() {
                out.push(reference_out(doc, &doc.model, None, f, e, x));
            }
        }
        Some(assembly) => {
            let only = component.map(|c| c.resolve(doc)).transpose()?;
            for c in assembly.components() {
                if only.is_some_and(|o| o != c.id) {
                    continue;
                }
                let Some(part) = assembly.definition_of(c.id).map(|d| d.model.clone()) else {
                    continue;
                };
                for (f, e, x) in part.external_references() {
                    out.push(reference_out(doc, &part, Some(c.id), f, e, x));
                }
            }
        }
    }
    let mut m = Map::new();
    m.insert("references".to_owned(), json!(out));
    Ok(m)
}

/// The parts `lock_references` and `break_references` work on: the document itself (a
/// part), or each definition of the component given (or of every component with
/// references), with its name.
fn targets(doc: &Document, component: Option<&CompSel>) -> Result<Vec<(Option<DefId>, String)>, String> {
    let Some(assembly) = doc.model.assembly() else {
        if component.is_some() {
            return Err(format!("{} is a part: 'component' is for an assembly.", doc.title()));
        }
        return Ok(vec![(None, doc.title())]);
    };
    let only = component.map(|c| c.resolve(doc)).transpose()?;
    let mut out: Vec<(Option<DefId>, String)> = Vec::new();
    for c in assembly.components() {
        if only.is_some_and(|o| o != c.id) {
            continue;
        }
        if let Some(d) = assembly.definition_of(c.id)
            && (only.is_some() || !d.model.external_references().is_empty())
            && !out.iter().any(|(id, _)| *id == Some(d.id))
        {
            out.push((Some(d.id), d.model.name.clone()));
        }
    }
    Ok(out)
}

/// `lock_references` and `break_references`: the undo label, or `None` if nothing changed.
pub(crate) fn change(
    doc: &Document,
    model: &mut Model,
    data: &mut Map<String, Value>,
    component: Option<&CompSel>,
    feature: Option<&FeatureSel>,
    lock: Option<bool>,
) -> Result<Option<String>, String> {
    let mut count = 0;
    let mut written: Vec<String> = Vec::new();
    for (def, _) in targets(doc, component)? {
        let mut part = match def {
            None => model.clone(),
            Some(d) => (*model
                .assembly()
                .and_then(|a| a.definition(d))
                .ok_or("The part is gone.")?
                .model)
                .clone(),
        };
        let f = feature.map(|s| feature_in(&part, s)).transpose()?;
        let n = match lock {
            Some(on) => part.lock_external(f, on),
            None => part.break_external(f),
        };
        if n == 0 {
            continue;
        }
        count += n;
        match def {
            None => *model = part,
            Some(d) => {
                model
                    .assembly_mut()
                    .expect("an assembly")
                    .set_model(d, Arc::new(part));
                if let Some(file) = crate::equations::write_link(doc, model, d)? {
                    written.push(file.display().to_string());
                }
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
            .any(|(.., x)| context_key(&x.context) == here)
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
pub(crate) fn made_out(doc: &Document, c: CompId, f: FeatureId, e: EntityId) -> Value {
    let Some(part) = doc
        .model
        .assembly()
        .and_then(|a| a.definition_of(c))
        .map(|d| d.model.clone())
    else {
        return Value::Null;
    };
    part.external_references()
        .into_iter()
        .find(|(g, h, _)| *g == f && *h == e)
        .map_or(Value::Null, |(_, _, x)| reference_out(doc, &part, Some(c), f, e, x))
}
