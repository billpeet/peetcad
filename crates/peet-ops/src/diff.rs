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

use crate::assembly::{ComponentChange, InsertSource, Placing};
use crate::fields::{FeatureArgs, SketchPlane, input_of};
use crate::host::Host;
use crate::mate::{MateChange, MateEndSel, MateType};
use crate::op::{DatumSel, New, Op, Place, RollTo};
use crate::value::{Configs, FeatureSel, Input};
use crate::{Undo, apply_with};

fn by_id(id: FeatureId) -> FeatureSel {
    FeatureSel::Id(id)
}

/// What a value set on the model itself applies to: the active configuration if the
/// value differs between configurations, every configuration if it doesn't.
fn as_set_directly(differs: bool) -> Configs {
    if differs { Configs::This } else { Configs::All }
}

/// The operations that turn the document's model into `new`, in the order to apply them.
///
/// The model holds the active configuration, so a tool's change is one to that
/// configuration where it differs from the others, and to all of them where it doesn't.
/// Changes to the configurations themselves are operations from the start, not found
/// here.
///
/// Fails, with the reason, if the change is one the operations can't express.
pub fn diff(doc: &Document, new: &Model) -> Result<Vec<Op>, String> {
    diff_scoped(doc, new, None)
}

/// [`diff`], with the configurations the numeric values that changed (feature values and
/// sketch dimensions) are changed in. `None`: as a change to the model is (see above).
pub fn diff_scoped(
    doc: &Document,
    new: &Model,
    values: Option<&Configs>,
) -> Result<Vec<Op>, String> {
    let old = &doc.model;
    // With one configuration there is nothing to choose.
    let values = values.filter(|_| old.configurations().len() > 1);
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
                    configurations: as_set_directly(old.parameter_differs(&p.name)),
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

    // ---- An assembly's parts and components ----
    match (old.assembly(), new.assembly()) {
        (None, None) => {}
        (Some(before), Some(after)) => {
            for d in after.definitions() {
                let was = before.definition(d.id);
                if was.map_or(d.link.is_some(), |o| o.link != d.link) {
                    return Err(format!(
                        "{} was linked to a file or unlinked, which is done by an operation, not by a change to the model",
                        d.name()
                    ));
                }
                if before.definition(d.id).is_some_and(|o| o.model != d.model) {
                    ops.push(Op::SetPart {
                        part: d.id,
                        model: d.model.clone(),
                    });
                }
            }
            let component = |c: &peet_model::Component, change| Op::Component {
                component: c.id.into(),
                change,
            };
            // The patterns that are gone, first: their copies go with them.
            let edit_pattern =
                |p: &peet_model::ComponentPattern, change| Op::EditComponentPattern {
                    pattern: p.id.into(),
                    change,
                };
            for p in before.patterns() {
                // (A pattern whose originals were all deleted went with them.)
                let originals = p.seeds.iter().any(|s| after.component(*s).is_some());
                if after.pattern(p.id).is_none() && originals {
                    ops.push(edit_pattern(p, crate::PatternChange::Delete));
                }
            }
            for c in before.components() {
                // A pattern's copy goes with its pattern, its original or a lower count.
                if after.component(c.id).is_none() && c.pattern.is_none() {
                    ops.push(component(c, ComponentChange::Delete));
                }
            }
            // New ones in the order they were made, so that they get the same ids.
            let mut added: Vec<&peet_model::Component> = after
                .components()
                .filter(|c| before.component(c.id).is_none() && c.pattern.is_none())
                .collect();
            added.sort_by_key(|c| c.id);
            for c in added {
                let part = after
                    .definition(c.definition)
                    .ok_or_else(|| format!("{} has no part", c.name))?;
                ops.push(Op::Insert {
                    from: InsertSource::Model(part.model.clone()),
                    name: Some(c.name.clone()),
                    placing: Some(Placing::Frame(c.placement)),
                    fixed: Some(c.fixed),
                    link: false,
                    absolute: false,
                });
                if c.suppressed {
                    ops.push(component(c, ComponentChange::Suppress(true)));
                }
                if !c.visible {
                    ops.push(component(c, ComponentChange::Show(false)));
                }
                if c.color.is_some() {
                    ops.push(component(c, ComponentChange::Color(c.color)));
                }
            }
            for c in after.components() {
                let Some(o) = before.component(c.id) else {
                    continue;
                };
                if o.definition != c.definition {
                    let part = after
                        .definition(c.definition)
                        .ok_or_else(|| format!("{} has no part", c.name))?;
                    ops.push(component(
                        c,
                        ComponentChange::Replace(InsertSource::Model(part.model.clone())),
                    ));
                }
                if o.name != c.name {
                    ops.push(component(c, ComponentChange::Rename(c.name.clone())));
                }
                // (A pattern's copy is placed by its pattern.)
                if o.placement != c.placement && c.pattern.is_none() {
                    ops.push(component(
                        c,
                        ComponentChange::Place(Placing::Frame(c.placement)),
                    ));
                }
                if o.fixed != c.fixed && c.pattern.is_none() {
                    ops.push(component(c, ComponentChange::Fix(c.fixed)));
                }
                if o.suppressed != c.suppressed {
                    ops.push(component(c, ComponentChange::Suppress(c.suppressed)));
                }
                if o.visible != c.visible {
                    ops.push(component(c, ComponentChange::Show(c.visible)));
                }
                if o.color != c.color {
                    ops.push(component(c, ComponentChange::Color(c.color)));
                }
            }
            // The patterns: new ones (which make their copies), and changed ones.
            let mut new_patterns: Vec<&peet_model::ComponentPattern> = after
                .patterns()
                .filter(|p| before.pattern(p.id).is_none())
                .collect();
            new_patterns.sort_by_key(|p| p.id);
            for p in new_patterns {
                ops.push(Op::ComponentPattern {
                    components: p.seeds.iter().map(|c| (*c).into()).collect(),
                    kind: crate::PatternSpec::Exact(p.kind.clone()),
                    name: Some(p.name.clone()),
                });
            }
            for p in after.patterns() {
                let Some(o) = before.pattern(p.id) else {
                    continue;
                };
                if o.name != p.name {
                    ops.push(edit_pattern(
                        p,
                        crate::PatternChange::Rename(p.name.clone()),
                    ));
                }
                if o.kind != p.kind {
                    ops.push(edit_pattern(p, crate::PatternChange::Set(p.kind.clone())));
                }
            }
            // The exploded view's steps. What a deleted component took with it (itself
            // out of its steps, and the steps it left empty) needs no operation.
            let edit = |s: &peet_model::ExplodeStep, change| Op::EditExplodeStep {
                step: s.id.into(),
                change,
            };
            let left = |s: &peet_model::ExplodeStep| -> Vec<peet_model::CompId> {
                s.components
                    .iter()
                    .copied()
                    .filter(|c| after.component(*c).is_some())
                    .collect()
            };
            let selectors = |ids: &[peet_model::CompId]| -> Vec<crate::CompSel> {
                ids.iter().map(|c| (*c).into()).collect()
            };
            for s in before.explode_steps() {
                if after.explode_step(s.id).is_none() && !left(s).is_empty() {
                    ops.push(edit(s, crate::ExplodeChange::Delete));
                }
            }
            let mut added: Vec<&peet_model::ExplodeStep> = after
                .explode_steps()
                .filter(|s| before.explode_step(s.id).is_none())
                .collect();
            added.sort_by_key(|s| s.id);
            for s in added {
                ops.push(Op::ExplodeStep {
                    components: selectors(&s.components),
                    by: crate::Point3::Mm(s.offset),
                    name: Some(s.name.clone()),
                });
            }
            for s in after.explode_steps() {
                let Some(o) = before.explode_step(s.id) else {
                    continue;
                };
                if o.name != s.name {
                    ops.push(edit(s, crate::ExplodeChange::Rename(s.name.clone())));
                }
                let components = (left(o) != s.components).then(|| selectors(&s.components));
                let by = (o.offset != s.offset).then_some(crate::Point3::Mm(s.offset));
                if by.is_some() || components.is_some() {
                    ops.push(edit(s, crate::ExplodeChange::Edit { by, components }));
                }
            }
        }
        _ => {}
    }
    if let (Some(before), Some(after)) = (old.assembly(), new.assembly()) {
        use peet_model::MateKind;
        let edit = |m: &peet_model::Mate, change| Op::EditMate {
            mate: m.id.into(),
            change,
        };
        for m in before.mates() {
            // A deleted component's mates went with it.
            let with_component = [&m.a, &m.b]
                .iter()
                .any(|e| e.component().is_some_and(|c| after.component(c).is_none()));
            if after.mate(m.id).is_none() && !with_component {
                ops.push(edit(m, MateChange::Delete));
            }
        }
        let mut added: Vec<&peet_model::Mate> = after
            .mates()
            .filter(|m| before.mate(m.id).is_none())
            .collect();
        added.sort_by_key(|m| m.id);
        for m in added {
            ops.push(Op::Mate {
                kind: match &m.kind {
                    MateKind::Coincident => MateType::Coincident,
                    MateKind::Concentric => MateType::Concentric,
                    MateKind::Parallel => MateType::Parallel,
                    MateKind::Distance(s) => MateType::Distance(input_of(s)),
                    MateKind::Angle(s) => MateType::Angle(input_of(s)),
                    MateKind::Fasten(relative) => MateType::Fasten(Some(*relative)),
                },
                a: MateEndSel::Ref(m.a.clone()),
                b: MateEndSel::Ref(m.b.clone()),
                flip: Some(m.flip),
                name: Some(m.name.clone()),
            });
            if m.suppressed {
                ops.push(edit(m, MateChange::Suppress(true)));
            }
        }
        for m in after.mates() {
            let Some(o) = before.mate(m.id) else {
                continue;
            };
            if o.a != m.a || o.b != m.b {
                return Err(format!(
                    "{} was put on other faces, which no operation does (delete it and add a new one)",
                    m.name
                ));
            }
            if o.name != m.name {
                ops.push(edit(m, MateChange::Rename(m.name.clone())));
            }
            let value = match (&o.kind, &m.kind) {
                (a, b) if a == b => None,
                (MateKind::Distance(_), MateKind::Distance(s))
                | (MateKind::Angle(_), MateKind::Angle(s)) => Some(input_of(s)),
                _ => {
                    return Err(format!(
                        "{} was made another kind of mate, which no operation does",
                        m.name
                    ));
                }
            };
            if value.is_some() || o.flip != m.flip {
                ops.push(edit(
                    m,
                    MateChange::Edit {
                        value,
                        flip: (o.flip != m.flip).then_some(m.flip),
                    },
                ));
            }
            if o.suppressed != m.suppressed {
                ops.push(edit(m, MateChange::Suppress(m.suppressed)));
            }
        }
    }
    match (old.is_assembly(), new.is_assembly()) {
        (a, b) if a == b => {}
        _ => {
            return Err(
                "the document was changed from a part to an assembly or back, which no operation does"
                    .to_owned(),
            );
        }
    }

    // ---- What the part is made of, and its colour ----
    if old.material != new.material {
        ops.push(Op::SetMaterial {
            material: new.material.as_ref().map(|m| m.name.clone()),
            density: new.material.as_ref().map(|m| m.density),
        });
    }
    if old.color != new.color {
        ops.push(Op::SetColor { color: new.color });
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
                        configurations: None,
                    });
                }
                if a.sketch != b.sketch || a.projections != b.projections {
                    ops.push(Op::SetSketch {
                        sketch: by_id(f.id),
                        content: Box::new(b.sketch.clone()),
                        projections: b.projections.clone(),
                        configurations: values.cloned(),
                    });
                }
            }
            (a, b) if a != b => {
                // With a scope for the values, they are changed on their own: the other
                // fields are the same in every configuration.
                let mut half = a.clone();
                if values.is_some() {
                    let now = b.slots();
                    for (name, _, value) in half.slots_mut() {
                        if let Some((_, _, v)) = now.iter().find(|(n, ..)| *n == name) {
                            *value = (*v).clone();
                        }
                    }
                }
                let mut any = false;
                for (from, to, configurations) in [(a, &half, values), (&half, b, None)] {
                    if from == to {
                        continue;
                    }
                    let fields = FeatureArgs::changes(from, to, doc)
                        .filter(|c| !c.is_empty())
                        .ok_or_else(|| {
                            format!("{} was changed in a way no operation does", f.name)
                        })?;
                    ops.push(Op::Edit {
                        feature: by_id(f.id),
                        fields,
                        configurations: configurations.cloned(),
                    });
                    any = true;
                }
                if !any {
                    return Err(format!("{} was changed in a way no operation does", f.name));
                }
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
                        projections: s.projections.clone(),
                        configurations: None,
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
                configurations: as_set_directly(old.suppression_differs(f.id)),
            });
        }
        if f.suppression_expression.as_ref() != was.and_then(|o| o.suppression_expression.as_ref())
        {
            ops.push(Op::SetSuppressionExpression {
                feature: by_id(f.id),
                value: f.suppression_expression.clone(),
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
    apply_model_scoped(host, doc, new, label, key, None)
}

/// [`apply_model`], with the configurations the numeric values that changed (feature
/// values and sketch dimensions) are changed in: what the application's "this
/// configuration only" asks for. `None`: as a change to the model is.
pub fn apply_model_scoped(
    host: &mut dyn Host,
    doc: &mut Document,
    new: Model,
    label: &str,
    key: u64,
    values: Option<&Configs>,
) -> Translation {
    let undo = Undo::Group(key);
    let mut new = new;
    // The model asked for, with the values that changed given their scope.
    if let Some(scope) = values.and_then(|c| c.resolve(doc).ok()) {
        let ids: Vec<FeatureId> = new.features().map(|f| f.id).collect();
        for id in ids {
            for v in new.values(id) {
                if let Some((_, before)) = doc.model.value(id, &v.slot)
                    && before != v.value
                {
                    let _ = new.rescope_value(id, &v.slot, before, &scope);
                }
            }
        }
    }
    new.tidy_configurations();
    let ops = match diff_scoped(doc, &new, values) {
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
