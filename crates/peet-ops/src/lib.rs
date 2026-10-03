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

mod args;
mod export;
mod fields;
mod op;
mod query;
pub mod select;
pub mod sketch;
mod value;

pub use fields::{
    AngledPlane, BaseFlange, CircularPattern, CoordinateSystem, CoordinatesPoint, Corner,
    CylinderAxis, EdgeAxis, EdgeFlange, Extrude, FeatureArgs, Form, Hem, Jog, LinearPattern,
    MidPlane, Mirror, MiterFlange, OffsetPlane, PlanesAxis, SheetCut, SketchPlane, SketchedBend,
    VertexPoint,
};
pub use op::{Format, New, Op, Place, Query};
pub use sketch::{Draw, DrawItem, Ent, Measure, Relation};
pub use value::{
    AxisSel, Bend, EdgeQuery, EdgeSel, End, FaceQuery, FaceSel, FeatureSel, Input, PlaneSel,
    PointSel, Regions, Side, VertexQuery, VertexSel,
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
    created: Vec<FeatureId>,
    /// A feature the operation was about, reported with its status.
    feature: Option<FeatureId>,
    /// A sketch whose plane and definition go in the reply.
    sketch: Option<FeatureId>,
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
                "{} can't be copied: patterns and mirrors copy extrusions, cuts, sheet metal cuts and forms.",
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
        if let Some(result) = n.feature.set(&mut kind, doc, true) {
            result?;
        }
        check_copies(doc, &kind)?;
        let source = kind.sketch();
        let id = model.add(kind);
        // A sketch used by a feature is hidden, as in the application.
        if let Some(s) = source.and_then(|s| model.feature_mut(s)) {
            s.visible = false;
        }
        if let Some(name) = &n.name {
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
            name.to_owned(),
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
        "values".to_owned(),
        json!("A number is in document units (degrees for angles). Text is an expression: \"2 * thickness\", \"1in + 5mm\"."),
    );
    out
}

fn query(doc: &Document, q: &Query) -> Result<Map<String, Value>, String> {
    Ok(match q {
        Query::Help => help(),
        Query::Status => object(query::status(doc)),
        Query::Features => object(query::features(doc)),
        Query::Feature(f) => object(query::feature(doc, f.resolve(doc)?)),
        Query::Parameters => object(query::parameters(doc)),
        Query::Bodies => object(query::bodies(doc)),
        Query::Faces { body } => object(query::faces(doc, *body)?),
        Query::Edges { body } => object(query::edges(doc, *body)?),
        Query::BendTable { body } => object(query::bend_table(doc, *body)?),
        Query::Checks { body } => object(query::checks(doc, *body)?),
    })
}

fn run(doc: &mut Document, op: &Op) -> Result<Done, String> {
    let mut done = Done::default();
    let mut model = doc.model.clone();
    let label = match op {
        Op::Query(q) => {
            done.data = query(doc, q)?;
            return Ok(done);
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
            match fields.set(&mut f.kind, doc, false) {
                Some(result) => result?,
                None => {
                    return Err(format!(
                        "{} ({}) doesn't have the fields of '{}'.",
                        f.name,
                        f.kind.type_name(),
                        fields.word()
                    ));
                }
            }
            done.feature = Some(id);
            format!("Edit {}", model.name_of(id))
        }
        Op::Sketch { on, name, draw } => {
            let (plane, placement) = select::plane(doc, on)?;
            let id = model.add_sketch(plane, placement);
            if let Some(name) = name {
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
            None => {
                model.set_rollback(None);
                "Roll to End".to_owned()
            }
            Some(f) => {
                let id = f.resolve(doc)?;
                model.set_rollback(model.index_of(id).map(|i| i + 1));
                format!("Roll Back to {}", model.name_of(id))
            }
        },
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

fn failed(name: &str, error: String) -> Reply {
    Reply {
        ok: false,
        changed: false,
        json: json!({ "ok": false, "op": name, "error": error }),
    }
}

/// Applies one operation to the document.
pub fn apply(doc: &mut Document, op: &Op, undo: Undo) -> Reply {
    // A part shown from a file's caches is rebuilt first: selectors need its bodies.
    doc.finish_loading();
    let name = op.word();
    let mut done = match run(doc, op) {
        Ok(d) => d,
        Err(e) => return failed(name, e),
    };
    if let Some((label, model)) = done.commit.take() {
        let set = |m: &mut Model| *m = model;
        done.changed = match undo {
            Undo::Step => doc.change(&label, set),
            Undo::Group(key) => doc.change_merging(&label, key, set),
        };
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
    out.extend(done.data);
    if done.changed {
        let failures = query::failures(doc);
        if !failures.is_empty() {
            out.insert("failures".to_owned(), json!(failures));
        }
    }
    Reply {
        ok: true,
        changed: done.changed,
        json: Value::Object(out),
    }
}

/// Reads a JSON operation into an [`Op`], or says what is wrong with it. The document is
/// needed to know what kind of feature an `edit` is about.
pub fn parse(doc: &Document, op: &Value) -> Result<Op, String> {
    op::from_json(op, doc).1
}

/// Applies one JSON operation to the document.
pub fn apply_json(doc: &mut Document, op: &Value, undo: Undo) -> Reply {
    doc.finish_loading();
    match op::from_json(op, doc) {
        (_, Ok(op)) => apply(doc, &op, undo),
        (name, Err(e)) => failed(&name, e),
    }
}

/// Applies JSON operations in order, stopping at the first that can't be applied. Returns
/// the replies so far (the last one is the failure, if there was one).
pub fn apply_all(doc: &mut Document, ops: &[Value], undo: Undo) -> Vec<Reply> {
    let mut replies = Vec::with_capacity(ops.len());
    for op in ops {
        let reply = apply_json(doc, op, undo);
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
