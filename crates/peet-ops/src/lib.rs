//! PeetCAD's operations: everything a user can do to a part, as data.
//!
//! An operation is an [`Op`]. Rust callers build one, and the compiler checks its fields:
//!
//! ```
//! use peet_document::Document;
//! use peet_ops::{Draw, Extrude, FeatureArgs, Op, Undo, apply};
//! use peet_model::StdPlane;
//!
//! let mut doc = Document::default();
//! apply(&mut doc, &Op::Sketch {
//!     on: StdPlane::Top.into(),
//!     name: None,
//!     draw: vec![Draw::Rectangle { from: [0.0, 0.0], to: [80.0, 50.0] }.into()],
//! }, Undo::Step);
//! let reply = apply(&mut doc, &Op::add(FeatureArgs::Extrude(Extrude {
//!     sketch: Some("Sketch1".into()),
//!     depth: Some(8.into()),
//!     ..Default::default()
//! })), Undo::Step);
//! assert!(reply.ok);
//! ```
//!
//! Scripts and agents write the same operations as JSON, which [`apply_json`] reads into
//! an [`Op`] (and says what is wrong if it can't):
//!
//! ```json
//! {"op": "sketch", "on": "top", "draw": [{"type": "rectangle", "from": [0, 0], "to": [80, 50]}]}
//! {"op": "extrude", "sketch": "Sketch1", "depth": 8}
//! ```
//!
//! [`apply`] returns a JSON reply: `ok`, what was created (with ids, names and rebuild
//! status), every feature that currently fails to build, and the data of a query. An
//! operation that can't be applied changes nothing and says why. An operation that
//! changes the part is one undo step, exactly like the same action in the application.
//!
//! - Numbers are in document units (or degrees); text is an expression (`"2 * t"`,
//!   `"1in"`): see [`Input`].
//! - Features are referred to by name or id: see [`FeatureSel`].
//! - Planes, faces, edges and vertices are referred to by selectors, which are either a
//!   description to resolve or an already resolved reference: see [`select`].
//! - Sketch contents are drawn with lists of [`Draw`] items: see [`sketch`].
//! - `{"op": "help"}` lists every operation and its fields.

mod analysis;
mod args;
mod assembly;
mod diff;
mod export;
mod fields;
mod host;
mod library;
mod links;
mod mate;
mod op;
mod query;
pub mod select;
mod session;
pub mod sketch;
mod value;

pub use assembly::{CompSel, ComponentChange, InsertSource, Placing, Point3};
pub use diff::{Translation, apply_model, diff};
pub use export::export_bytes;
pub use fields::{
    AngledPlane, BaseFlange, Blend, CircularPattern, ConvertToSheet, CoordinateSystem,
    CoordinatesPoint, Corner, CylinderAxis, Draft, EdgeAxis, EdgeFlange, Extrude, FeatureArgs,
    Form, Hem, Hole, Jog, LinearPattern, Loft, MidPlane, Mirror, MiterFlange, OffsetPlane,
    PlanesAxis, Revolve, SheetCut, Shell, SketchPlane, SketchedBend, Sweep, VertexPoint,
};
pub use host::{AppCommand, Headless, Host, SketchTool, Toggle, View, Window};
pub use library::{CheckRule, Gauge, GaugeBend};
pub use links::{PartSel, linked_files};
pub use mate::{MateChange, MateEndSel, MateSel, MateType};
pub use op::{
    DatumSel, DxfPlacement, DxfTarget, Format, New, Op, Place, Query, RollTo, Sample, Source,
};
pub use session::{DocSel, SessionCommand, apply_session, apply_session_json};
pub use sketch::{Draw, DrawItem, Ent, Measure, Relation};
pub use value::{
    AxisSel, Bend, EdgeQuery, EdgeSel, End, FaceQuery, FaceSel, FeatureSel, GeomSel, HoleStandard,
    Input, PlaneSel, PointSel, Regions, RevolveAxis, Side, VertexQuery, VertexSel,
};

use peet_document::Document;
use peet_model::{FeatureId, FeatureKind, Model};
use peet_sketch::expr::Units;
use serde_json::{Map, Value, json};

/// How an operation that changes the part is recorded for undo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Undo {
    /// Its own undo step.
    #[default]
    Step,
    /// One undo step together with the neighbouring operations of the same group (a
    /// script run). Call [`Document::seal_history`] when the group ends.
    Group(u64),
}

/// The answer to one operation.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    /// Whether the operation was applied.
    pub ok: bool,
    /// Whether it changed the part.
    pub changed: bool,
    /// The features it added, in the order they were made.
    pub created: Vec<FeatureId>,
    /// Whether the document was replaced by another (new, open): whatever was known
    /// about the old one (a selection, an open sketch) no longer applies.
    pub replaced: bool,
    /// The reply as JSON: `ok`, `op`, then `error`, or `created`, `failures` and data.
    pub json: Value,
}

/// What an operation did, before it is turned into a reply.
#[derive(Default)]
struct Done {
    /// The changed model and its undo label.
    commit: Option<(String, Model)>,
    /// Set by operations that change the document themselves (undo, redo).
    changed: bool,
    /// The document was replaced by another.
    replaced: bool,
    created: Vec<FeatureId>,
    /// A feature the operation was about, reported with its status.
    feature: Option<FeatureId>,
    /// A sketch whose plane and definition go in the reply.
    sketch: Option<FeatureId>,
    /// A component the operation was about, reported with where it is and its status.
    component: Option<peet_model::CompId>,
    /// A mate the operation was about, reported with its status.
    mate: Option<peet_model::MateId>,
    /// A pull on a component to solve the assembly with, and its undo label.
    drag: Option<(peet_model::Drag, String)>,
    data: Map<String, Value>,
}

fn object(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn sketch_id(doc: &Document, sel: &FeatureSel) -> Result<FeatureId, String> {
    let id = sel.resolve(doc)?;
    if doc.model.sketch(id).is_none() {
        return Err(format!("{} is not a sketch.", doc.model.name_of(id)));
    }
    Ok(id)
}

fn rename(model: &mut Model, id: FeatureId, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A name can't be empty.".to_owned());
    }
    if model.features().any(|f| f.id != id && f.name == name) {
        return Err(format!("Another feature is already called '{name}'."));
    }
    if let Some(f) = model.feature_mut(id) {
        name.clone_into(&mut f.name);
    }
    Ok(())
}

fn check_copies(doc: &Document, kind: &FeatureKind) -> Result<(), String> {
    let seeds = match kind {
        FeatureKind::Pattern(p) => &p.seeds,
        FeatureKind::Mirror(m) => &m.seeds,
        _ => return Ok(()),
    };
    for id in seeds {
        if doc.feature(*id).is_some_and(|f| !f.kind.can_be_copied()) {
            return Err(format!(
                "{} can't be copied: patterns and mirrors copy extrusions, cuts, revolves, holes, sheet metal cuts and forms.",
                doc.model.name_of(*id)
            ));
        }
    }
    Ok(())
}

fn add(doc: &Document, model: &mut Model, new: &[New]) -> Result<Vec<FeatureId>, String> {
    let mut ids = Vec::new();
    for n in new {
        let mut kind = n.feature.blank().ok_or_else(|| {
            "A sketch is added with the 'sketch' operation, which also places it.".to_owned()
        })?;
        n.feature.prepare(&mut kind, doc, true)?;
        if let Some(result) = n.feature.set(&mut kind, doc, true) {
            result?;
        }
        check_copies(doc, &kind)?;
        // The sketches a feature is made from are hidden, as in the application.
        let mut sources: Vec<FeatureId> = kind.sketch().into_iter().collect();
        if let FeatureKind::Sweep(sweep) = &kind {
            sources.extend(sweep.path);
        }
        if let FeatureKind::Loft(loft) = &kind {
            sources.extend(loft.sections.iter().skip(1));
        }
        let id = model.add(kind);
        for source in sources {
            if let Some(s) = model.feature_mut(source) {
                s.visible = false;
            }
        }
        // (A name that is the automatic one needs no renaming.)
        if let Some(name) = &n.name
            && model.name_of(id) != name
        {
            rename(model, id, name)?;
        }
        ids.push(id);
    }
    if ids.is_empty() {
        return Err("There is nothing to add.".to_owned());
    }
    Ok(ids)
}

fn help() -> Map<String, Value> {
    let mut ops = Map::new();
    for (name, fields_text, what) in op::OTHER_OPS {
        ops.insert(
            (*name).to_owned(),
            json!({ "does": what, "fields": fields_text }),
        );
    }
    for (word, what, describe) in FeatureArgs::OPS {
        if !word.split('.').next().is_some_and(FeatureArgs::adds) {
            continue;
        }
        let mut f = describe();
        f.insert("name".to_owned(), json!("text"));
        match word.split_once('.') {
            // One operation with several forms, told apart by the fields given.
            Some((name, form)) => {
                let entry = ops
                    .entry(name.to_owned())
                    .or_insert_with(|| json!({ "forms": {} }));
                entry["forms"][form] = json!({ "does": what, "fields": f });
            }
            None => {
                ops.insert((*word).to_owned(), json!({ "does": what, "fields": f }));
            }
        }
    }
    for (form, (name, fields_text, what)) in assembly::ASSEMBLY_OPS
        .iter()
        .map(|o| ("component", o))
        .chain(mate::MATE_OPS.iter().map(|o| ("mate", o)))
        .chain(links::LINK_OPS.iter().map(|o| ("part", o)))
    {
        // Words a feature has too (rename, delete): the component's and the mate's
        // forms beside it.
        match ops.get_mut(*name) {
            Some(entry) => {
                entry[form] = json!({ "does": what, "fields": fields_text });
            }
            None => {
                ops.insert(
                    (*name).to_owned(),
                    json!({ "does": what, "fields": fields_text, "in": "an assembly" }),
                );
            }
        }
    }
    for (name, fields_text, what) in session::SESSION_OPS {
        ops.insert(
            (*name).to_owned(),
            json!({ "does": what, "fields": fields_text }),
        );
    }
    for (name, fields_text, what) in host::app_ops() {
        ops.insert(
            name.to_owned(),
            json!({ "does": what, "fields": fields_text, "needs": "a running PeetCAD" }),
        );
    }
    let mut out = Map::new();
    out.insert("operations".to_owned(), Value::Object(ops));
    out.insert("draw".to_owned(), sketch::help());
    out.insert(
        "selectors".to_owned(),
        json!({
            "plane": "\"top\", \"front\", \"right\", a reference plane's name, or a face selector",
            "axis": "\"x\", \"y\", \"z\", a reference axis's name, or an edge selector",
            "point": "\"origin\", a reference point's name, or a vertex selector",
            "face": "{at: [x,y,z], normal: [x,y,z], feature, side: start|end|side|top|bottom|bend|wall, body, index}: all given fields must match exactly one face",
            "edge": "{between: [[x,y,z],[x,y,z]], at: [x,y,z], faces: [face, face], body, index}",
            "vertex": "{at: [x,y,z], body, index}",
        }),
    );
    out.insert(
        "document".to_owned(),
        json!("Any operation can take \"document\": the name or id of an open document (see 'documents'). It is then applied to that document instead of the current one."),
    );
    out.insert(
        "values".to_owned(),
        json!("A number is in document units (degrees for angles). Text is an expression: \"2 * thickness\", \"1in + 5mm\"."),
    );
    out
}

fn query(host: &mut dyn Host, doc: &Document, q: &Query) -> Result<Map<String, Value>, String> {
    Ok(match q {
        Query::Materials { material } => {
            library::materials(host.materials(), material.as_deref(), doc)?
        }
        Query::Help => help(),
        Query::Status => object(query::status(doc)),
        Query::Features => object(query::features(doc)),
        Query::Feature(f) => object(query::feature(doc, f.resolve(doc)?)),
        Query::Parameters => object(query::parameters(doc)),
        Query::Bodies => object(query::bodies(doc)),
        Query::Faces { body } => object(query::faces(doc, *body)?),
        Query::Edges { body } => object(query::edges(doc, *body)?),
        Query::BendTable { body } => object(query::bend_table(doc, *body)?),
        Query::Checks { body } => object(query::checks(doc, *body, host.check_rules())?),
        Query::Mass { body } if doc.is_assembly() => analysis::mass(doc, *body)?,
        Query::Mass { body } => object(query::mass(doc, *body)?),
        Query::Interference { component } => analysis::interference(doc, component.as_ref())?,
        Query::Bom { top_level } => analysis::bom(doc, *top_level),
        Query::Measure { a, b } => object(query::measure(doc, a, b.as_deref())?),
        Query::Components => assembly::components(doc)?,
        Query::Mates => mate::mates(doc)?,
    })
}

/// Refuses to replace a document with unsaved changes, unless told to discard them.
fn guard_unsaved(doc: &Document, discard: bool, word: &str) -> Result<(), String> {
    if doc.is_modified() && !discard {
        return Err(format!(
            "{} has unsaved changes: save it first, or give '{word}' \"discard\": true.",
            doc.title()
        ));
    }
    Ok(())
}

fn dxf_unit(unit: peet_sketch::expr::LengthUnit) -> peet_io::dxf_import::Unit {
    use peet_io::dxf_import::Unit;
    use peet_sketch::expr::LengthUnit;
    match unit {
        LengthUnit::Mm => Unit::Millimetres,
        LengthUnit::Cm => Unit::Centimetres,
        LengthUnit::M => Unit::Metres,
        LengthUnit::Inch => Unit::Inches,
        LengthUnit::Ft => Unit::Feet,
    }
}

fn run(host: &mut dyn Host, doc: &mut Document, op: &Op) -> Result<Done, String> {
    if let Some(why) = assembly::wrong_kind(doc, op) {
        return Err(why);
    }
    let mut done = Done::default();
    let mut model = doc.model.clone();
    let label = match op {
        Op::Insert {
            from,
            name,
            placing,
            fixed,
            link,
            absolute,
        } => {
            if *absolute && !*link {
                return Err("'absolute' is about a link: give it with \"link\": true.".to_owned());
            }
            let (id, label) = assembly::insert(
                doc,
                &mut model,
                from,
                name.as_deref(),
                placing.as_ref(),
                *fixed,
                link.then_some(!*absolute),
            )?;
            done.component = Some(id);
            label
        }
        Op::UpdateLinks => {
            links::unavailable()?;
            let (updated, problems) = links::refresh(&mut model, links::folder(doc).as_deref(), 0);
            done.data = links::report(&updated, &problems);
            if updated.is_empty() {
                return Ok(done);
            }
            "Update Links".to_owned()
        }
        Op::Link {
            part,
            path,
            absolute,
        } => {
            let id = part.resolve(doc)?;
            assembly::link(doc, &mut model, id, path, !*absolute)?
        }
        Op::Unlink { part } => {
            let id = part.resolve(doc)?;
            assembly::unlink(doc, &mut model, id)?
        }
        Op::Component { component, change } => {
            let id = component.resolve(doc)?;
            let label = assembly::change(doc, &mut model, id, change)?;
            if *change == ComponentChange::Delete {
                done.data.insert(
                    "deleted".to_owned(),
                    json!([label.trim_start_matches("Delete ")]),
                );
            } else {
                done.component = Some(id);
            }
            label
        }
        Op::SetPart { part, model: new } => assembly::set_part(&mut model, *part, new)?,
        Op::Mate {
            kind,
            a,
            b,
            flip,
            name,
        } => {
            let (id, label) = mate::add(doc, &mut model, kind, a, b, *flip, name.as_deref())?;
            done.mate = Some(id);
            label
        }
        Op::EditMate { mate, change } => {
            let id = mate.resolve(doc)?;
            let label = mate::change(doc, &mut model, id, change)?;
            if *change == MateChange::Delete {
                done.data.insert(
                    "deleted".to_owned(),
                    json!([label.trim_start_matches("Delete ")]),
                );
            } else {
                done.mate = Some(id);
            }
            label
        }
        Op::OpenComponent { .. } => return Err(session::needs_session(op.word())),
        Op::Drag {
            component,
            point,
            to,
        } => {
            // Not a change to the model that is then rebuilt: the rebuild itself is
            // what moves the components (see `apply_with`).
            let (drag, label) = assembly::drag(doc, component, point.as_ref(), to)?;
            done.component = Some(drag.component);
            done.drag = Some((drag, label));
            return Ok(done);
        }
        Op::Query(q) => {
            done.data = query(host, doc, q)?;
            return Ok(done);
        }
        Op::App(command) => {
            done.data = host.app(command)?;
            return Ok(done);
        }
        Op::Session(command) => return Err(session::needs_session(command.word())),
        Op::New { keep: true, .. }
        | Op::Open { keep: true, .. }
        | Op::OpenSample { keep: true, .. } => {
            return Err(format!(
                "{} (Leave 'keep' out to open it in place of this document.)",
                session::needs_session(op.word())
            ));
        }
        Op::New {
            discard, assembly, ..
        } => {
            guard_unsaved(doc, *discard, "new")?;
            *doc = if *assembly {
                Document::from_model(Model::new_assembly(), None)
            } else {
                Document::default()
            };
            done.replaced = true;
            return Ok(done);
        }
        Op::Open { file, discard, .. } => {
            guard_unsaved(doc, *discard, "open")?;
            let bytes = file.read()?;
            let mut opened = peet_io::document::open(&bytes)
                .map_err(|e| format!("Couldn't open {}: {}", file.shown(), e.message))?;
            // An assembly's linked parts are read from their files as it is opened (its
            // links being relative to where it is).
            if links::unavailable().is_ok() {
                let beside = file.path.as_ref().and_then(|p| p.parent());
                opened.model.from_file(beside);
                let (updated, problems) = links::refresh(&mut opened.model, beside, 0);
                if !updated.is_empty() {
                    done.data.insert("updated".to_owned(), json!(updated));
                }
                opened.warnings.extend(problems);
            }
            if !opened.warnings.is_empty() {
                done.data
                    .insert("warnings".to_owned(), json!(opened.warnings));
            }
            *doc = Document::from_opened(
                opened,
                Some(peet_document::FileLocation {
                    name: file.name.clone(),
                    path: file.path.clone(),
                }),
            );
            doc.finish_loading();
            done.replaced = true;
            return Ok(done);
        }
        Op::OpenSample {
            sample, discard, ..
        } => {
            guard_unsaved(doc, *discard, "open_sample")?;
            *doc = Document::from_model(sample.model(), None);
            done.replaced = true;
            return Ok(done);
        }
        Op::FlatPattern { on } => {
            if !doc.has_sheet_metal() {
                return Err(
                    "There is no sheet metal body to show flat: start one with base_flange."
                        .to_owned(),
                );
            }
            let flat = on.unwrap_or(!doc.is_flat());
            doc.set_flat(flat);
            done.data.insert("flat".to_owned(), json!(flat));
            return Ok(done);
        }
        Op::SetGauge(gauge) => {
            done.data = library::set_gauge(host.materials(), doc, gauge)?;
            return Ok(done);
        }
        Op::DeleteGauge { material, gauge } => {
            done.data = library::delete_gauge(host.materials(), material, gauge.as_deref())?;
            return Ok(done);
        }
        Op::ImportMaterials { file } => {
            done.data = library::import_materials(host.materials(), file)?;
            return Ok(done);
        }
        Op::ExportMaterials { path } => {
            done.data = library::export_materials(host.materials(), path)?;
            return Ok(done);
        }
        Op::SetCheckRule {
            rule,
            thickness,
            radius,
            constant,
        } => {
            done.data =
                library::set_check_rule(host.check_rules(), *rule, *thickness, *radius, *constant)?;
            return Ok(done);
        }
        Op::ApplyMaterial {
            material,
            gauge,
            thickness,
            feature,
        } => {
            let (id, label, applied) = library::apply_material(
                host.materials(),
                doc,
                &mut model,
                material,
                gauge.as_deref(),
                *thickness,
                feature.as_ref(),
            )?;
            done.feature = Some(id);
            done.data = object(applied);
            label
        }
        Op::SetMaterial { material, density } => {
            let (label, set) =
                library::set_material(host.materials(), &mut model, material.as_deref(), *density)?;
            done.data = set;
            label
        }
        Op::SetColor { color } => {
            model.color = *color;
            done.data.insert(
                "color".to_owned(),
                color.map_or(Value::Null, library::color_out),
            );
            "Change Colour".to_owned()
        }
        Op::ShowDatum { datum, on } => {
            use peet_model::{Datum, StdPlane};
            let datums: Vec<Datum> = match datum {
                DatumSel::Origin => vec![Datum::Origin],
                DatumSel::Front => vec![Datum::Plane(StdPlane::Front)],
                DatumSel::Top => vec![Datum::Plane(StdPlane::Top)],
                DatumSel::Right => vec![Datum::Plane(StdPlane::Right)],
                DatumSel::Planes => StdPlane::ALL.into_iter().map(Datum::Plane).collect(),
            };
            for d in datums {
                model.set_datum_visible(d, *on);
            }
            let word = if *on { "Show" } else { "Hide" };
            match datum {
                DatumSel::Planes => format!("{word} Planes"),
                DatumSel::Origin => format!("{word} Origin"),
                _ => format!("{word} {} Plane", datum.word()),
            }
        }
        Op::ImportDxf {
            file,
            into,
            unit,
            placement,
        } => {
            use peet_io::dxf_import::{self, ImportOptions, Placement};
            let bytes = file.read()?;
            let options = ImportOptions {
                unit: unit.map(dxf_unit),
                placement: match placement {
                    DxfPlacement::Keep => Placement::Keep,
                    DxfPlacement::Centred => Placement::Centred,
                    DxfPlacement::LowerLeft => Placement::LowerLeftAtOrigin,
                },
                ..ImportOptions::flat_pattern()
            };
            let failed =
                |e: dxf_import::ImportError| format!("Couldn't import {}: {e}", file.shown());
            let (id, report, label) = match into {
                DxfTarget::Sketch(sketch) => {
                    let id = sketch_id(doc, sketch)?;
                    let Some(s) = model.feature_mut(id).and_then(|f| f.sketch_mut()) else {
                        return Err("The sketch no longer exists.".to_owned());
                    };
                    let report =
                        dxf_import::import_into(&mut s.sketch, &bytes, &options).map_err(failed)?;
                    done.feature = Some(id);
                    (id, report, format!("Import DXF into {}", model.name_of(id)))
                }
                DxfTarget::New { on, name } => {
                    let (plane, at) = select::plane(doc, on)?;
                    let imported = dxf_import::import(&bytes, &options).map_err(failed)?;
                    let id = model.add_sketch(plane, at);
                    if let Some(name) = name {
                        rename(&mut model, id, name)?;
                    }
                    if let Some(s) = model.feature_mut(id).and_then(|f| f.sketch_mut()) {
                        s.sketch = imported.sketch;
                    }
                    done.created.push(id);
                    (id, imported.report, "Import DXF".to_owned())
                }
            };
            done.sketch = Some(id);
            done.data
                .insert("curves".to_owned(), json!(report.entities_imported));
            done.data
                .insert("unit".to_owned(), json!(report.unit_used.label()));
            if !report.skipped.is_empty() {
                let skipped: Map<String, Value> = report
                    .skipped
                    .iter()
                    .map(|(kind, n)| (kind.clone(), json!(n)))
                    .collect();
                done.data.insert("left_out".to_owned(), json!(skipped));
            }
            if !report.warnings.is_empty() {
                done.data
                    .insert("warnings".to_owned(), json!(report.warnings));
            }
            label
        }
        Op::Export {
            path,
            format,
            body,
            schema,
        } => {
            done.data = export::export(doc, path, *format, *body, *schema)?;
            return Ok(done);
        }
        Op::Save { path, caches } => {
            done.data = export::save(doc, path.as_ref(), *caches)?;
            return Ok(done);
        }
        Op::ImportStep { file } => {
            let bytes = file.read()?;
            let text = String::from_utf8_lossy(&bytes);
            let imported = doc.import_step(&file.name, &text)?;
            done.changed = true;
            done.created.push(imported.feature);
            done.data
                .insert("bodies".to_owned(), json!(imported.bodies));
            if !imported.warnings.is_empty() {
                done.data
                    .insert("warnings".to_owned(), json!(imported.warnings));
            }
            return Ok(done);
        }
        Op::Undo | Op::Redo => {
            let (what, word) = if *op == Op::Undo {
                (doc.undo(), "undo")
            } else {
                (doc.redo(), "redo")
            };
            let what = what.ok_or_else(|| format!("There is nothing to {word}."))?;
            done.changed = true;
            done.data.insert(word.to_owned(), json!(what));
            return Ok(done);
        }

        Op::Add(new) => {
            let ids = add(doc, &mut model, new)?;
            let names: Vec<&str> = ids.iter().map(|i| model.name_of(*i)).collect();
            let label = format!("Add {}", names.join(", "));
            done.created = ids;
            label
        }
        Op::Edit { feature, fields } => {
            let id = feature.resolve(doc)?;
            let Some(f) = model.feature_mut(id) else {
                return Err("The feature no longer exists.".to_owned());
            };
            fields.prepare(&mut f.kind, doc, false)?;
            match fields.set(&mut f.kind, doc, false) {
                Some(result) => result?,
                None => {
                    // The fields of another form of the same feature (an offset plane
                    // made an angled one) replace its definition.
                    let other = fields
                        .blank()
                        .filter(|k| std::mem::discriminant(k) == std::mem::discriminant(&f.kind));
                    let Some(mut other) = other else {
                        return Err(format!(
                            "{} ({}) doesn't have the fields of '{}'.",
                            f.name,
                            f.kind.type_name(),
                            fields.word()
                        ));
                    };
                    fields.prepare(&mut other, doc, true)?;
                    if let Some(result) = fields.set(&mut other, doc, true) {
                        result?;
                    }
                    f.kind = other;
                }
            }
            done.feature = Some(id);
            format!("Edit {}", model.name_of(id))
        }
        Op::Sketch { on, name, draw } => {
            let (plane, placement) = select::plane(doc, on)?;
            let id = model.add_sketch(plane, placement);
            if let Some(name) = name
                && model.name_of(id) != name
            {
                rename(&mut model, id, name)?;
            }
            if !draw.is_empty()
                && let Some(s) = model.feature_mut(id).and_then(|f| f.sketch_mut())
            {
                let drawn = sketch::draw(&mut s.sketch, draw, doc)?;
                done.data.insert("drawn".to_owned(), json!(drawn));
            }
            done.created.push(id);
            done.sketch = Some(id);
            format!("Add {}", model.name_of(id))
        }
        Op::Draw { sketch, draw } => {
            let id = sketch_id(doc, sketch)?;
            if let Some(s) = model.feature_mut(id).and_then(|f| f.sketch_mut()) {
                let drawn = sketch::draw(&mut s.sketch, draw, doc)?;
                done.data.insert("drawn".to_owned(), json!(drawn));
            }
            done.feature = Some(id);
            done.sketch = Some(id);
            format!("Edit {}", model.name_of(id))
        }
        Op::SetDimension {
            sketch,
            name,
            value,
        } => {
            let id = sketch_id(doc, sketch)?;
            if let Some(s) = model.feature_mut(id).and_then(|f| f.sketch_mut()) {
                sketch::set_dimension(&mut s.sketch, doc, name, value)?;
            }
            done.feature = Some(id);
            done.sketch = Some(id);
            format!("Edit {}", model.name_of(id))
        }
        Op::Rename { feature, name } => {
            let id = feature.resolve(doc)?;
            rename(&mut model, id, name)?;
            done.feature = Some(id);
            format!("Rename {}", doc.model.name_of(id))
        }
        Op::Suppress { feature, on } => {
            let id = feature.resolve(doc)?;
            if let Some(f) = model.feature_mut(id) {
                f.suppressed = *on;
            }
            done.feature = Some(id);
            let word = if *on { "Suppress" } else { "Unsuppress" };
            format!("{word} {}", model.name_of(id))
        }
        Op::Show { feature, on } => {
            let id = feature.resolve(doc)?;
            if let Some(f) = model.feature_mut(id) {
                f.visible = *on;
            }
            done.feature = Some(id);
            let word = if *on { "Show" } else { "Hide" };
            format!("{word} {}", model.name_of(id))
        }
        Op::Delete { features } => {
            let ids: Vec<FeatureId> = features
                .iter()
                .map(|f| f.resolve(doc))
                .collect::<Result<_, _>>()?;
            if ids.is_empty() {
                return Err("There is nothing to delete.".to_owned());
            }
            let names: Vec<String> = ids.iter().map(|i| model.name_of(*i).to_owned()).collect();
            for id in &ids {
                model.remove(*id);
            }
            done.data.insert("deleted".to_owned(), json!(names));
            format!("Delete {}", names.join(", "))
        }
        Op::Move { feature, to } => {
            let id = feature.resolve(doc)?;
            let from = model.index_of(id).unwrap_or(0);
            let beside = |target: &FeatureSel, after: bool| -> Result<usize, String> {
                let t = doc.model.index_of(target.resolve(doc)?).unwrap_or(0);
                Ok(match (from < t, after) {
                    (true, false) => t - 1,
                    (true, true) | (false, false) => t,
                    (false, true) => t + 1,
                })
            };
            let index = match to {
                Place::Before(t) => beside(t, false)?,
                Place::After(t) => beside(t, true)?,
                Place::Index(i) => *i,
            };
            model.move_to(id, index)?;
            done.feature = Some(id);
            format!("Move {}", model.name_of(id))
        }
        Op::Rollback { to } => match to {
            RollTo::End => {
                model.set_rollback(None);
                "Roll to End".to_owned()
            }
            RollTo::Start => {
                model.set_rollback(Some(0));
                "Roll Back".to_owned()
            }
            RollTo::After(f) => {
                let id = f.resolve(doc)?;
                model.set_rollback(model.index_of(id).map(|i| i + 1));
                format!("Roll Back to {}", model.name_of(id))
            }
        },
        Op::SetSketch { sketch, content } => {
            let id = sketch_id(doc, sketch)?;
            if let Some(s) = model.feature_mut(id).and_then(|f| f.sketch_mut()) {
                s.sketch = (**content).clone();
            }
            done.feature = Some(id);
            done.sketch = Some(id);
            format!("Edit {}", model.name_of(id))
        }
        Op::SetParameter { name, value } => {
            model
                .parameters
                .set(name, &value.text())
                .map_err(|e| format!("{name}: {}", e.message))?;
            if let Some(p) = model.parameters.entries.iter().find(|p| p.name == *name) {
                done.data.insert(
                    "parameter".to_owned(),
                    json!({
                        "name": name,
                        "expression": p.expression,
                        "value": p.display(&model.parameters.units),
                    }),
                );
            }
            format!("Set {name}")
        }
        Op::DeleteParameter { name } => {
            if !model.parameters.entries.iter().any(|p| p.name == *name) {
                return Err(format!("There is no parameter called '{name}'."));
            }
            model.parameters.remove(name);
            format!("Delete {name}")
        }
        Op::SetUnits { length } => {
            if let Some((name, e)) = model
                .parameters
                .set_units(Units::new(*length))
                .into_iter()
                .next()
            {
                return Err(format!("{name}: {}", e.message));
            }
            "Change Units".to_owned()
        }
    };
    done.commit = Some((label, model));
    Ok(done)
}

impl Reply {
    /// The reply to an operation called `name` that could not be applied.
    pub fn error(name: &str, error: String) -> Self {
        failed(name, error)
    }
}

fn failed(name: &str, error: String) -> Reply {
    Reply {
        ok: false,
        changed: false,
        created: Vec::new(),
        replaced: false,
        json: json!({ "ok": false, "op": name, "error": error }),
    }
}

/// Applies one operation to the document, with nothing but the document: the built-in
/// material tables and check limits, and no application. See [`apply_in`].
pub fn apply(doc: &mut Document, op: &Op, undo: Undo) -> Reply {
    apply_in(&mut Headless::default(), doc, op, undo)
}

/// Applies one operation to the document, in `host`: the application, or a [`Headless`]
/// kept for a whole script so that changes to the material tables last.
pub fn apply_in(host: &mut dyn Host, doc: &mut Document, op: &Op, undo: Undo) -> Reply {
    apply_with(host, doc, op, undo, None)
}

/// [`apply_in`], with the undo step called `label` instead of what the operation would
/// call it.
pub(crate) fn apply_with(
    host: &mut dyn Host,
    doc: &mut Document,
    op: &Op,
    undo: Undo,
    label: Option<&str>,
) -> Reply {
    // A part shown from a file's caches is rebuilt first: selectors need its bodies.
    doc.finish_loading();
    let name = op.word();
    let mut done = match run(host, doc, op) {
        Ok(d) => d,
        Err(e) => return failed(name, e),
    };
    // Where an assembly's components are, to say which of them the change moved.
    let placements = |doc: &Document| -> Vec<(peet_model::CompId, peet_math::Frame)> {
        doc.model.assembly().map_or_else(Vec::new, |a| {
            a.components().map(|c| (c.id, c.placement)).collect()
        })
    };
    let before = placements(doc);
    if let Some((own, model)) = done.commit.take() {
        let label = label.map_or(own, str::to_owned);
        let set = |m: &mut Model| *m = model;
        done.changed = match undo {
            Undo::Step => doc.change(&label, set),
            Undo::Group(key) => doc.change_merging(&label, key, set),
        };
    }
    if let Some((drag, own)) = done.drag.take() {
        let label = label.map_or(own, str::to_owned);
        let key = match undo {
            Undo::Step => None,
            Undo::Group(key) => Some(key),
        };
        done.changed = doc.drag_component(&label, key, drag);
        // How far the point still is from where it was pulled to, if its mates held
        // it back.
        if let Some(c) = doc
            .model
            .assembly()
            .and_then(|a| a.component(drag.component))
        {
            let short = c.placement.to_world(drag.point).distance(drag.to);
            if short > 1e-3 {
                done.data.insert(
                    "short_by".to_owned(),
                    args::length_out(short, &doc.model.parameters.units),
                );
            }
        }
    }
    if let Some(id) = done.sketch {
        done.data.extend(object(query::sketch_summary(doc, id)));
    }
    let mut out = Map::new();
    out.insert("ok".to_owned(), json!(true));
    out.insert("op".to_owned(), json!(name));
    if !done.created.is_empty() {
        let created: Vec<Value> = done
            .created
            .iter()
            .map(|id| query::brief(doc, *id))
            .collect();
        out.insert("created".to_owned(), json!(created));
    }
    if let Some(id) = done.feature {
        out.insert("feature".to_owned(), query::brief(doc, id));
    }
    if let Some(id) = done.component {
        out.insert("component".to_owned(), assembly::component_out(doc, id));
    }
    if let Some(id) = done.mate {
        out.insert("mate".to_owned(), mate::mate_out(doc, id));
    }
    if done.changed && !done.replaced && doc.is_assembly() {
        // The components that are somewhere else now: what a mate, or a placement that
        // the mates corrected, came to.
        let moved: Vec<Value> = placements(doc)
            .into_iter()
            .filter(|(id, frame)| {
                // Somewhere else by more than rounding.
                let elsewhere = |was: &peet_math::Frame| {
                    was.origin.distance(frame.origin) > 1e-7
                        || was.rotation.angle_between(frame.rotation) > 1e-9
                };
                Some(*id) != done.component
                    && before.iter().any(|(b, was)| b == id && elsewhere(was))
            })
            .map(|(id, _)| assembly::component_out(doc, id))
            .collect();
        if !moved.is_empty() {
            out.insert("moved".to_owned(), json!(moved));
        }
        out.insert("freedom".to_owned(), json!(doc.evaluation().freedom));
    }
    out.extend(done.data);
    if done.changed {
        let failures = query::failures(doc);
        if !failures.is_empty() {
            out.insert("failures".to_owned(), json!(failures));
        }
    }
    if done.replaced {
        out.extend(object(query::status(doc)));
    }
    Reply {
        ok: true,
        changed: done.changed,
        created: done.created,
        replaced: done.replaced,
        json: Value::Object(out),
    }
}

/// Reads a JSON operation into an [`Op`], or says what is wrong with it. The document is
/// needed to know what kind of feature an `edit` is about.
pub fn parse(doc: &Document, op: &Value) -> Result<Op, String> {
    op::from_json(op, doc).1
}

/// Applies one JSON operation to the document, with nothing but the document. See
/// [`apply_json_in`].
pub fn apply_json(doc: &mut Document, op: &Value, undo: Undo) -> Reply {
    apply_json_in(&mut Headless::default(), doc, op, undo)
}

/// Applies one JSON operation to the document, in `host`.
pub fn apply_json_in(host: &mut dyn Host, doc: &mut Document, op: &Value, undo: Undo) -> Reply {
    doc.finish_loading();
    match op::from_json(op, doc) {
        (_, Ok(op)) => apply_in(host, doc, &op, undo),
        (name, Err(e)) => failed(&name, e),
    }
}

/// Applies JSON operations in order, stopping at the first that can't be applied. Returns
/// the replies so far (the last one is the failure, if there was one). The operations
/// share one [`Headless`] host, so a change to the material tables lasts for the run.
pub fn apply_all(doc: &mut Document, ops: &[Value], undo: Undo) -> Vec<Reply> {
    let mut host = Headless::default();
    let mut replies = Vec::with_capacity(ops.len());
    for op in ops {
        let reply = apply_json_in(&mut host, doc, op, undo);
        let ok = reply.ok;
        replies.push(reply);
        if !ok {
            break;
        }
    }
    replies
}

/// Reads a script: a JSON list of operations, or one operation per line (blank lines and
/// lines starting with `#` or `//` are skipped).
pub fn parse_script(text: &str) -> Result<Vec<Value>, String> {
    let trimmed = text.trim_start();
    if trimmed.starts_with('[') {
        return match serde_json::from_str::<Value>(trimmed) {
            Ok(Value::Array(ops)) => Ok(ops),
            Ok(_) => Err("The script is not a list of operations.".to_owned()),
            Err(e) => Err(format!("The script is not valid JSON: {e}")),
        };
    }
    let mut ops = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        match serde_json::from_str(line) {
            Ok(op) => ops.push(op),
            Err(e) => return Err(format!("Line {} is not valid JSON: {e}", i + 1)),
        }
    }
    Ok(ops)
}
