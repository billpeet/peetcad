//! Flexible sub-assemblies, as SolidWorks solves them ("Solve as: Flexible").
//!
//! A rigid sub-assembly is one body in its parent, posed as its own file has it. A
//! flexible one is taken apart for the parent's solve: the sub-assembly itself (with
//! whatever is fixed in it) is one *unit*, and each component that is not fixed in it is
//! another, rigid unless it is itself a flexible sub-assembly, which is taken apart in
//! turn. The sub-assembly's own mates are solved with the parent's, between the units, so
//! a hinge's leaves turn as far as the parent's mates ask. Where each moving component
//! ends up is stored on the parent's component ([`crate::Component::poses`]), in the
//! sub-assembly's coordinates, since the instances of one sub-assembly share its file
//! but can each be posed differently.
//!
//! The solver ([`crate::mate::solve`]) is not changed by any of this: a unit is a
//! component to it, under an id of its own, and the mates' ends are rewritten to name the
//! unit and the path below it.

use peet_math::Frame;
use std::collections::HashMap;

use crate::Model;
use crate::assembly::{CompId, Component, Pose};
use crate::feature::{Scalar, ScalarKind};
use crate::mate::{self, Mate, MateEnd, MateId, MateKind, WidthMode};
use crate::regen::{Engine, Evaluation};
use peet_sketch::expr::Parameters;

/// Whether a component is solved as flexible: a sub-assembly marked so. A pattern's copy
/// is placed by its pattern, as one thing.
pub(crate) fn is_flexible(c: &Component, definition: &Model) -> bool {
    c.flexible && c.pattern.is_none() && definition.is_assembly()
}

/// What takes something under `path` (components below a component, in its definition)
/// from where its definition has it to where the component's poses put it, both in the
/// component's coordinates: the pose of the deepest moving component above it (or it).
pub(crate) fn correction(poses: &[Pose], built: &Evaluation, path: &[CompId]) -> Frame {
    poses
        .iter()
        .filter(|p| !p.path.is_empty() && path.starts_with(&p.path))
        .max_by_key(|p| p.path.len())
        .and_then(|p| Some(p.frame.compose(&built.component_frame(&p.path)?.inverse())))
        .unwrap_or(Frame::WORLD)
}

/// The units of a flexible sub-assembly: their paths below it (the first empty, the
/// sub-assembly itself) and names; and the flexible levels in it (the sub-assembly and
/// the flexible sub-assemblies in it that are taken apart), whose mates are solved.
fn units(
    model: &std::sync::Arc<Model>,
    prefix: &mut Vec<CompId>,
    name: &str,
    out: &mut Vec<(Vec<CompId>, String)>,
    levels: &mut Vec<(Vec<CompId>, std::sync::Arc<Model>)>,
) {
    let Some(assembly) = model.assembly() else {
        return;
    };
    levels.push((prefix.clone(), model.clone()));
    for c in assembly.components().filter(|c| !c.suppressed) {
        // In the configuration the component shows.
        let Some(definition) = assembly.model_of(c) else {
            continue;
        };
        prefix.push(c.id);
        let inner = format!("{name}/{}", c.name);
        if !(c.fixed || c.pattern.is_some()) {
            out.push((prefix.clone(), inner.clone()));
        }
        if is_flexible(c, &definition) {
            units(&definition, prefix, &inner, out, levels);
        }
        prefix.pop();
    }
}

/// The unit a path below the component falls in (the one whose path is the longest
/// start of it), and the rest of the path below that unit.
fn unit_of(units: &[(Vec<CompId>, CompId)], path: &[CompId]) -> (CompId, Vec<CompId>) {
    units
        .iter()
        .filter(|(u, _)| path.starts_with(u))
        .max_by_key(|(u, _)| u.len())
        .map(|(u, id)| (*id, path[u.len()..].to_vec()))
        .expect("the sub-assembly's own unit starts every path")
}

/// A flexible sub-assembly taken apart for its parent's solve.
pub(crate) struct Expanded {
    /// Each unit's path below the component and its id in the solve (the component's
    /// own id for the sub-assembly itself).
    pub units: Vec<(Vec<CompId>, CompId)>,
    /// The units as the solve sees them, in the same order.
    pub placed: Vec<mate::Placed>,
    /// The sub-assembly's own mates (and those of the flexible sub-assemblies in it)
    /// between different units, with their ends in the solve's terms.
    pub mates: Vec<Mate>,
}

/// Takes a flexible component apart. `engine` is its definition's, rebuilt; `fixed` is
/// whether the component is held in the parent. New ids for units and mates are taken
/// from `next_id` and `next_mate`, counting down, so they meet none of the parent's.
pub(crate) fn expand(
    c: &Component,
    definition: &std::sync::Arc<Model>,
    engine: &Engine,
    fixed: bool,
    next_id: &mut u32,
    next_mate: &mut u32,
) -> Expanded {
    let built = engine.evaluation();
    let mut found = vec![(Vec::new(), c.name.clone())];
    let mut levels = Vec::new();
    units(
        definition,
        &mut Vec::new(),
        &c.name,
        &mut found,
        &mut levels,
    );
    // A unit whose component wasn't built (suppressed by a pattern) has nothing to move.
    found.retain(|(path, _)| path.is_empty() || built.component_frame(path).is_some());
    let as_built = |path: &[CompId]| {
        if path.is_empty() {
            Frame::WORLD
        } else {
            built.component_frame(path).unwrap_or(Frame::WORLD)
        }
    };
    let ids: Vec<(Vec<CompId>, CompId)> = found
        .iter()
        .map(|(path, _)| {
            let id = if path.is_empty() {
                c.id
            } else {
                *next_id -= 1;
                CompId(*next_id + 1)
            };
            (path.clone(), id)
        })
        .collect();
    let mut placed: Vec<mate::Placed> = found
        .iter()
        .zip(&ids)
        .map(|((path, name), (_, id))| {
            let start = if path.is_empty() {
                Frame::WORLD
            } else {
                c.poses
                    .iter()
                    .find(|p| p.path == *path)
                    .map_or_else(|| as_built(path), |p| p.frame)
            };
            mate::Placed {
                id: *id,
                name: name.clone(),
                frame: c.placement.compose(&start),
                fixed: path.is_empty() && fixed,
                bodies: Vec::new(),
                planes: Vec::new(),
                models: Vec::new(),
            }
        })
        .collect();
    let index = |path: &[CompId]| {
        let (id, rest) = unit_of(&ids, path);
        let k = ids.iter().position(|(_, i)| *i == id).unwrap_or(0);
        (k, rest)
    };
    for body in &built.bodies {
        placed[0]
            .bodies
            .push((Vec::new(), body.clone(), Frame::WORLD));
    }
    for i in &built.instances {
        let (k, rest) = index(&i.path);
        let rel = as_built(&ids[k].0).inverse().compose(&i.frame);
        placed[k].bodies.push((rest, i.body.clone(), rel));
    }
    for (path, reference, plane) in engine.mate_planes(definition) {
        let (k, rest) = index(&path);
        let frame = as_built(&ids[k].0).inverse().compose(&plane.frame);
        placed[k]
            .planes
            .push((rest, reference, peet_math::Plane { frame }));
    }
    // The part models, for mates on their sketches.
    for (path, model, frame) in engine.mate_models(definition) {
        let (k, rest) = index(&path);
        let rel = as_built(&ids[k].0).inverse().compose(&frame);
        placed[k].models.push((rest, model, rel));
    }

    // ---- The mates inside, between units ----
    let mut mates = Vec::new();
    for (level, model) in levels {
        let Some(assembly) = model.assembly() else {
            continue;
        };
        let end = |e: &MateEnd| {
            let mut full = level.clone();
            full.extend(&e.path);
            let (id, rest) = unit_of(&ids, &full);
            let mut path = vec![id];
            path.extend(rest);
            MateEnd {
                path,
                geom: e.geom.clone(),
            }
        };
        let unit_path = |e: &MateEnd| {
            let mut full = level.clone();
            full.extend(&e.path);
            ids[index(&full).0].0.clone()
        };
        for m in assembly.mates().filter(|m| !m.suppressed) {
            let (a, b) = (end(&m.a), end(&m.b));
            // Both on one unit: it holds as the sub-assembly's file has it.
            if a.path[0] == b.path[0] {
                continue;
            }
            let kind = match &m.kind {
                MateKind::Fasten(_) => MateKind::Fasten(
                    as_built(&unit_path(&m.a))
                        .inverse()
                        .compose(&as_built(&unit_path(&m.b))),
                ),
                MateKind::Width { a2, b2, mode } => MateKind::Width {
                    a2: end(a2),
                    b2: b2.as_ref().map(end),
                    mode: match mode {
                        WidthMode::Dimension(s) => {
                            WidthMode::Dimension(frozen(s, ScalarKind::Length, &model.parameters))
                        }
                        WidthMode::Percent(s) => {
                            WidthMode::Percent(frozen(s, ScalarKind::Number, &model.parameters))
                        }
                        other => other.clone(),
                    },
                },
                MateKind::Distance(s) => {
                    MateKind::Distance(frozen(s, ScalarKind::Length, &model.parameters))
                }
                MateKind::Angle(s) => {
                    MateKind::Angle(frozen(s, ScalarKind::Angle, &model.parameters))
                }
                other => other.clone(),
            };
            *next_mate -= 1;
            mates.push(Mate {
                id: MateId(*next_mate + 1),
                name: format!("{}/{}", c.name, m.name),
                kind,
                a,
                b,
                align: m.align,
                flip_dimension: m.flip_dimension,
                suppressed: false,
            });
        }
    }
    Expanded {
        units: ids,
        placed,
        mates,
    }
}

/// A value read with its own assembly's parameters, which the parent's solve doesn't have.
fn frozen(s: &Scalar, kind: ScalarKind, params: &Parameters) -> Scalar {
    Scalar::new(s.evaluate(kind, params).unwrap_or(s.value))
}

/// A mate of the parent with its ends on the units of the flexible components they
/// reach into; `None` if it needs no change. A fasten holds the components as wholes.
pub(crate) fn rewrite(m: &Mate, expanded: &HashMap<CompId, Expanded>) -> Option<Mate> {
    if matches!(m.kind, MateKind::Fasten(_)) {
        return None;
    }
    let end = |e: &MateEnd| -> Option<MateEnd> {
        let x = expanded.get(&e.component()?)?;
        let (id, rest) = unit_of(&x.units, &e.path[1..]);
        let mut path = vec![id];
        path.extend(rest);
        Some(MateEnd {
            path,
            geom: e.geom.clone(),
        })
    };
    let (a, b) = (end(&m.a), end(&m.b));
    let (a2, b2) = match &m.kind {
        MateKind::Width { a2, b2, .. } => (end(a2), b2.as_ref().and_then(end)),
        MateKind::Symmetric { plane } => (end(plane), None),
        _ => (None, None),
    };
    if a.is_none() && b.is_none() && a2.is_none() && b2.is_none() {
        return None;
    }
    let mut out = m.clone();
    out.a = a.unwrap_or_else(|| m.a.clone());
    out.b = b.unwrap_or_else(|| m.b.clone());
    if let MateKind::Width {
        a2: wa2, b2: wb2, ..
    } = &mut out.kind
    {
        if let Some(a2) = a2 {
            *wa2 = a2;
        }
        if let (Some(b2), Some(slot)) = (b2, wb2.as_mut()) {
            *slot = b2;
        }
    } else if let (MateKind::Symmetric { plane }, Some(a2)) = (&mut out.kind, a2) {
        *plane = a2;
    }
    Some(out)
}

/// The poses of a flexible component's moving units after a solve, in its coordinates.
pub(crate) fn poses(frame: Frame, x: &Expanded, frames: &HashMap<CompId, Frame>) -> Vec<Pose> {
    let back = frame.inverse();
    x.units
        .iter()
        .filter(|(path, _)| !path.is_empty())
        .filter_map(|(path, id)| {
            Some(Pose {
                path: path.clone(),
                frame: back.compose(frames.get(id)?),
            })
        })
        .collect()
}

/// Whether two lists of poses are the same to within rounding: a solve that moved
/// nothing leaves them, so the model doesn't change at every rebuild.
pub(crate) fn same_poses(a: &[Pose], b: &[Pose]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(p, q)| {
            p.path == q.path
                && p.frame.origin.distance(q.frame.origin) < 1e-9
                && p.frame.rotation.dot(q.frame.rotation).abs() > 1.0 - 1e-15
        })
}
