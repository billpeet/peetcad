//! Translating a change made to the model directly into operations.
//!
//! The application's tools edit a copy of the model (add a flange, change a depth in the
//! properties, drag a feature up the tree). [`diff`] works out the operations that make
//! the same change, and [`apply_model`] applies them. So everything the application does
//! to a part goes through the operations a script uses: it can be recorded and replayed,
//! and anything the application can do that the operations couldn't express shows up as
//! a failed translation instead of going unnoticed.

use std::collections::HashSet;

use peet_document::Document;
use peet_model::{Datum, FeatureId, FeatureKind, Model, StdPlane};
use peet_sketch::Sketch;

use crate::fields::{FeatureArgs, SketchPlane};
use crate::host::Host;
use crate::op::{DatumSel, New, Op, Place, RollTo};
use crate::value::{FeatureSel, Input};
use crate::{Undo, apply_with};

fn by_id(id: FeatureId) -> FeatureSel {
    FeatureSel::Id(id)
}

/// The operations that turn the document's model into `new`, in the order to apply them.
///
/// Fails, with the reason, if the change is one the operations can't express.
pub fn diff(doc: &Document, new: &Model) -> Result<Vec<Op>, String> {
    let old = &doc.model;
    let mut ops = Vec::new();
    if old.name != new.name {
        return Err("the part was renamed, which no operation does".to_owned());
    }

    // ---- Parameters ----
    let (before, after) = (&old.parameters, &new.parameters);
    if before.units != after.units {
        ops.push(Op::SetUnits {
            length: after.units.length,
        });
    }
    let mut names: Vec<&str> = Vec::new();
    for p in &before.entries {
        if after.entries.iter().any(|n| n.name == p.name) {
            names.push(&p.name);
        } else {
            ops.push(Op::DeleteParameter {
                name: p.name.clone(),
            });
        }
    }
    for p in &after.entries {
        match before.entries.iter().find(|o| o.name == p.name) {
            Some(o) if o.expression == p.expression => {}
            known => {
                if known.is_none() {
                    names.push(&p.name);
                }
                ops.push(Op::SetParameter {
                    name: p.name.clone(),
                    value: Input::Expr(p.expression.clone()),
                });
            }
        }
    }
    if !names
        .iter()
        .copied()
        .eq(after.entries.iter().map(|p| p.name.as_str()))
    {
        return Err("the parameters were put in another order, which no operation does".to_owned());
    }

    // ---- Features removed ----
    // The tree's order and the rollback bar are followed through the operations, to see
    // at the end what is left to move.
    let mut order: Vec<FeatureId> = old.features().map(|f| f.id).collect();
    let mut built = old.is_rolled_back().then(|| old.rollback_index());
    let removed: Vec<FeatureId> = old
        .features()
        .map(|f| f.id)
        .filter(|id| new.feature(*id).is_none())
        .collect();
    for id in &removed {
        if let Some(index) = order.iter().position(|o| o == id) {
            order.remove(index);
            if let Some(b) = &mut built
                && index < *b
            {
                *b -= 1;
            }
        }
    }
    if !removed.is_empty() {
        ops.push(Op::Delete {
            features: removed.into_iter().map(by_id).collect(),
        });
    }

    // ---- Features changed ----
    for f in new.features() {
        let Some(o) = old.feature(f.id) else {
            continue;
        };
        if o.name != f.name {
            ops.push(Op::Rename {
                feature: by_id(f.id),
                name: f.name.clone(),
            });
        }
        match (&o.kind, &f.kind) {
            (FeatureKind::Sketch(a), FeatureKind::Sketch(b)) => {
                if a.plane != b.plane {
                    ops.push(Op::Edit {
                        feature: by_id(f.id),
                        fields: FeatureArgs::SketchPlane(SketchPlane {
                            on: Some(b.plane.clone().into()),
                        }),
                    });
                }
                if a.sketch != b.sketch {
                    ops.push(Op::SetSketch {
                        sketch: by_id(f.id),
                        content: Box::new(b.sketch.clone()),
                    });
                }
            }
            (a, b) if a != b => {
                let fields = FeatureArgs::changes(a, b, doc)
                    .filter(|c| !c.is_empty())
                    .ok_or_else(|| format!("{} was changed in a way no operation does", f.name))?;
                ops.push(Op::Edit {
                    feature: by_id(f.id),
                    fields,
                });
            }
            _ => {}
        }
    }

    // ---- Features added ----
    // In the order they were made (their ids), so that the operations give them the same
    // ids and automatic names.
    let mut added: Vec<&peet_model::Feature> = new
        .features()
        .filter(|f| old.feature(f.id).is_none())
        .collect();
    added.sort_by_key(|f| f.id);
    // Sketches a new feature is made from are hidden by adding it.
    let mut hidden: HashSet<FeatureId> = HashSet::new();
    let mut batch: Vec<New> = Vec::new();
    let mut in_batch: Vec<FeatureId> = Vec::new();
    for f in &added {
        // A feature built on one of the same batch is added after it exists.
        if f.kind.dependencies().iter().any(|d| in_batch.contains(d)) {
            ops.push(Op::Add(std::mem::take(&mut batch)));
            in_batch.clear();
        }
        match &f.kind {
            FeatureKind::Sketch(s) => {
                if !batch.is_empty() {
                    ops.push(Op::Add(std::mem::take(&mut batch)));
                    in_batch.clear();
                }
                ops.push(Op::Sketch {
                    on: s.plane.clone().into(),
                    name: Some(f.name.clone()),
                    draw: Vec::new(),
                });
                if s.sketch != Sketch::new() {
                    ops.push(Op::SetSketch {
                        sketch: by_id(f.id),
                        content: Box::new(s.sketch.clone()),
                    });
                }
            }
            kind => {
                let feature = FeatureArgs::of(kind, doc).ok_or_else(|| {
                    format!(
                        "{} ({}) is not added by setting fields",
                        f.name,
                        kind.type_name()
                    )
                })?;
                batch.push(New {
                    name: Some(f.name.clone()),
                    feature,
                });
                in_batch.push(f.id);
                hidden.extend(kind.sketch());
                if let FeatureKind::Sweep(sweep) = kind {
                    hidden.extend(sweep.path);
                }
            }
        }
        let at = built.map_or(order.len(), |b| b.min(order.len()));
        order.insert(at, f.id);
        if let Some(b) = &mut built {
            *b = at + 1;
        }
    }
    if !batch.is_empty() {
        ops.push(Op::Add(batch));
    }

    // ---- Suppressed, shown ----
    for f in new.features() {
        let was = old.feature(f.id);
        let suppressed = was.is_some_and(|o| o.suppressed);
        let visible = !hidden.contains(&f.id) && was.is_none_or(|o| o.visible);
        if f.suppressed != suppressed {
            ops.push(Op::Suppress {
                feature: by_id(f.id),
                on: f.suppressed,
            });
        }
        if f.visible != visible {
            ops.push(Op::Show {
                feature: by_id(f.id),
                on: f.visible,
            });
        }
    }

    // ---- Order ----
    let target: Vec<FeatureId> = new.features().map(|f| f.id).collect();
    if order.len() != target.len() {
        return Err("the features don't add up".to_owned());
    }
    for (i, id) in target.iter().enumerate() {
        if order[i] != *id {
            let Some(from) = order.iter().position(|o| o == id) else {
                return Err("the features don't add up".to_owned());
            };
            order.remove(from);
            order.insert(i, *id);
            ops.push(Op::Move {
                feature: by_id(*id),
                to: Place::Index(i),
            });
        }
    }

    // ---- The rollback bar ----
    let built = built.filter(|b| *b < order.len());
    let wanted = new.is_rolled_back().then(|| new.rollback_index());
    if built != wanted {
        ops.push(Op::Rollback {
            to: match wanted {
                None => RollTo::End,
                Some(0) => RollTo::Start,
                Some(b) => RollTo::After(by_id(target[b - 1])),
            },
        });
    }

    // ---- The origin and the standard planes ----
    for (datum, sel) in [
        (Datum::Origin, DatumSel::Origin),
        (Datum::Plane(StdPlane::Front), DatumSel::Front),
        (Datum::Plane(StdPlane::Top), DatumSel::Top),
        (Datum::Plane(StdPlane::Right), DatumSel::Right),
    ] {
        if old.datum_visible(datum) != new.datum_visible(datum) {
            ops.push(Op::ShowDatum {
                datum: sel,
                on: new.datum_visible(datum),
            });
        }
    }
    Ok(ops)
}

/// What became of a change to the model that was applied as operations.
#[derive(Clone, Debug, PartialEq)]
pub struct Translation {
    /// The operations that were applied (none if the model didn't change).
    pub ops: Vec<Op>,
    /// Whether the part changed.
    pub changed: bool,
    /// Why the change could not be made by operations, if it couldn't: it was then made
    /// directly, so the part is as asked either way. This is a gap in the operations.
    pub untranslated: Option<String>,
}

/// Makes the document's model `new`, as one undo step called `label`, by applying the
/// operations that make that change (see [`diff`]).
///
/// Changes with the same `key`, one after the other, are one undo step (a drag): call
/// [`Document::seal_history`] when they end. Use a key no other change uses for a change
/// on its own.
///
/// The part ends up as asked whatever happens: if the operations can't express the
/// change, or don't arrive at it, it is made directly and
/// [`Translation::untranslated`] says why.
pub fn apply_model(
    host: &mut dyn Host,
    doc: &mut Document,
    new: Model,
    label: &str,
    key: u64,
) -> Translation {
    let undo = Undo::Group(key);
    let ops = match diff(doc, &new) {
        Ok(ops) => ops,
        Err(reason) => {
            let changed = doc.change_merging(label, key, |m| *m = new);
            return Translation {
                ops: Vec::new(),
                changed,
                untranslated: Some(reason),
            };
        }
    };
    let mut changed = false;
    let mut untranslated = None;
    for op in &ops {
        let reply = apply_with(host, doc, op, undo, Some(label));
        changed |= reply.changed;
        if !reply.ok {
            untranslated = Some(format!(
                "{}: {}",
                op.word(),
                reply.json["error"].as_str().unwrap_or("it failed")
            ));
            break;
        }
    }
    // In development builds, check the operations arrived at the model that was asked
    // for: the same rebuild, from scratch, of that model.
    if untranslated.is_none() && cfg!(debug_assertions) {
        let mut wanted = new.clone();
        peet_model::Engine::new().regenerate(&mut wanted);
        if wanted != doc.model {
            untranslated =
                Some("the operations arrived at a different model than was asked for".to_owned());
        }
    }
    if untranslated.is_some() {
        changed |= doc.change_merging(label, key, |m| *m = new);
    }
    Translation {
        ops,
        changed,
        untranslated,
    }
}
