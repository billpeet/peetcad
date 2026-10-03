//! Sheet metal features: base flanges, edge flanges and sheet metal cuts.
//!
//! A sheet metal body carries its definition ([`peet_sheetmetal::Layout`]) alongside its
//! solid. Each sheet metal feature takes the definition as it is, adds to it (a flange, a
//! cut in flat-pattern coordinates) and rebuilds the folded solid and the flat pattern from
//! scratch, so the flat pattern is always exact.
//!
//! **Names.** Faces are named after the piece or the curve that made them: a flange's top
//! and bottom sides ([`FaceRole::SheetTop`], [`FaceRole::SheetBottom`]), a bend's
//! ([`FaceRole::BendTop`], [`FaceRole::BendBottom`]), walls from sketch curves
//! ([`FaceRole::Side`]) and walls the feature made ([`FaceRole::Wall`]). Pieces and curves
//! carry their feature as an owner number, so names stay the same however the body is
//! rebuilt.

use std::sync::Arc;

use peet_math::{DVec2, DVec3, Frame, Plane};
use peet_sheetmetal::layout::wall;
use peet_sheetmetal::{
    Area, BendModel, ChainLine, CornerSpec, CurveTag, EdgeFlangeSpec, EdgeSite, FaceTag, Form,
    FormKind, FormShape, HemSpec, JogSpec, Layout, PieceKind, SheetBody, SheetSettings,
    SketchedBendSpec, build,
};
use peet_sketch::expr::Parameters;
use peet_sketch::region::find_regions;
use peet_sketch::{Curve, EntityId, Sketch};

use crate::extrude::{default_regions, stamp};
use crate::feature::{
    BaseFlangeFeature, BendModelDef, CornerFeature, EdgeFlangeFeature, FeatureId, FormFeature,
    HemFeature, JogFeature, MiterFlangeFeature, ScalarKind, SheetSettingsDef, SketchedBendFeature,
};
use crate::naming::{Body, EdgeRef, FaceName, FaceRef, FaceRole, find_edge, find_face};
use crate::{FeatureError, Model};

/// What a sheet metal feature produced: the new bodies, and a warning if something needs
/// attention.
pub(crate) type Applied = (Vec<Arc<Body>>, Option<String>);

/// Evaluates a body's settings.
pub fn evaluate_settings(
    def: &SheetSettingsDef,
    params: &Parameters,
) -> Result<SheetSettings, String> {
    let value = |s: &crate::Scalar, kind, label: &str| {
        s.evaluate(kind, params)
            .map_err(|m| format!("{label}: {m}."))
    };
    let settings = SheetSettings {
        thickness: value(&def.thickness, ScalarKind::Length, "Thickness")?,
        radius: value(&def.radius, ScalarKind::Length, "Bend radius")?,
        model: match &def.model {
            BendModelDef::KFactor(k) => {
                BendModel::KFactor(value(k, ScalarKind::Number, "K-factor")?)
            }
            BendModelDef::Allowance(v) => {
                BendModel::Allowance(value(v, ScalarKind::Length, "Bend allowance")?)
            }
            BendModelDef::Deduction(v) => {
                BendModel::Deduction(value(v, ScalarKind::Length, "Bend deduction")?)
            }
        },
        relief: def.relief,
        relief_ratio: value(&def.relief_ratio, ScalarKind::Number, "Relief ratio")?,
    };
    settings.check()?;
    Ok(settings)
}

/// The name of each face of a sheet metal body.
pub(crate) fn face_names(sheet: &SheetBody) -> Vec<FaceName> {
    sheet
        .faces
        .iter()
        .map(|tag| {
            let piece = |p: usize| &sheet.layout.pieces[p];
            match *tag {
                FaceTag::Top { piece: p } | FaceTag::Bottom { piece: p } => {
                    let pc = piece(p);
                    let top = matches!(tag, FaceTag::Top { .. });
                    let role = match (pc.is_flange(), top) {
                        (true, true) => FaceRole::SheetTop(pc.origin.part),
                        (true, false) => FaceRole::SheetBottom(pc.origin.part),
                        (false, true) => FaceRole::BendTop(pc.origin.part),
                        (false, false) => FaceRole::BendBottom(pc.origin.part),
                    };
                    FaceName::new(FeatureId(pc.origin.owner), role)
                }
                FaceTag::Wall { tag, .. } | FaceTag::Form { tag, .. } => tag_name(tag),
            }
        })
        .collect()
}

/// The name of a wall from the tag of its curve.
fn tag_name(tag: CurveTag) -> FaceName {
    match tag {
        CurveTag::Sketch { owner, entity } => {
            FaceName::new(FeatureId(owner), FaceRole::Side(EntityId(entity)))
        }
        CurveTag::Generated { owner, part, index } => {
            FaceName::new(FeatureId(owner), FaceRole::Wall(part, index))
        }
        CurveTag::Copy {
            owner,
            copy,
            entity,
        } => FaceName::merged(&[
            FaceName::new(FeatureId(owner), FaceRole::Side(EntityId(entity))),
            FaceName::new(FeatureId(owner), FaceRole::Instance(copy)),
        ]),
    }
}

/// Builds a sheet metal body from a layout.
fn sheet_body(
    model: &Model,
    layout: Layout,
    origin: FeatureId,
    stamp: u64,
) -> Result<(Body, Option<String>), FeatureError> {
    let (solid, sheet) = build(layout)
        .map_err(|e| FeatureError(e.message(|o| model.name_of(FeatureId(o)).to_owned())))?;
    let pieces = sheet.outline.iter().filter(|l| l.outer).count();
    let warning = (pieces > 1).then(|| {
        format!(
            "The sheet is in {pieces} separate pieces: cuts or reliefs cut part of it off. Check the cuts, the flange offsets and the relief size."
        )
    });
    Ok((
        Body {
            face_names: face_names(&sheet),
            solid,
            origin,
            stamp,
            sheet: Some(Arc::new(sheet)),
        },
        warning,
    ))
}

/// Everything a base flange is built from, with references resolved.
pub(crate) struct BaseFlangeInput<'a> {
    pub feature: FeatureId,
    pub model: &'a Model,
    pub plane: Plane,
    pub sketch: &'a Sketch,
    pub def: &'a BaseFlangeFeature,
    pub settings: SheetSettings,
    /// Evaluated depth (open profiles).
    pub depth: f64,
    pub stamp: u64,
}

/// A base flange: a new sheet metal body.
pub(crate) fn apply_base_flange(
    input: &BaseFlangeInput<'_>,
    bodies: &[Arc<Body>],
) -> Result<Applied, FeatureError> {
    let owner = input.feature.0;
    let profile = find_regions(input.sketch);
    let layout = if !profile.regions.is_empty() {
        let regions: Vec<_> = default_regions(&profile)
            .into_iter()
            .map(|i| profile.regions[i].clone())
            .collect();
        Layout::plate(
            input.settings,
            owner,
            &input.plane,
            &regions,
            input.def.reverse,
        )
    } else {
        let chain = open_chain(input.sketch).map_err(FeatureError)?;
        let d = input.depth;
        if !(d.is_finite() && d > 0.0) {
            return Err(FeatureError(
                "The depth must be greater than zero.".to_owned(),
            ));
        }
        let depth = if input.def.symmetric {
            [-d / 2.0, d / 2.0]
        } else if input.def.flip_depth {
            [-d, 0.0]
        } else {
            [0.0, d]
        };
        Layout::open_profile(
            input.settings,
            owner,
            &input.plane,
            &chain,
            depth,
            input.def.reverse,
        )
    }
    .map_err(FeatureError)?;
    let (body, warning) = sheet_body(input.model, layout, input.feature, stamp(input.stamp, 0))?;
    let mut out = bodies.to_vec();
    out.push(Arc::new(body));
    Ok((out, warning))
}

/// The lines of an open sketch as one chain, in order. The chain starts at the free end of
/// the line with the lowest id, so the order (and the side the thickness goes) doesn't
/// change when the sketch is edited.
fn open_chain(sketch: &Sketch) -> Result<Vec<ChainLine>, String> {
    let mut lines: Vec<(u32, DVec2, DVec2)> = Vec::new();
    for (id, e) in sketch.entities() {
        if e.construction || !e.kind().is_curve() {
            continue;
        }
        match sketch.curve(id) {
            Some(Curve::Line { a, b }) => {
                if a.distance(b) > peet_math::tolerance::LINEAR {
                    lines.push((id.0, a, b));
                }
            }
            Some(_) => {
                return Err(
                    "An open profile can only have lines for now (bends are added at the corners). Replace the arcs with sharp corners, or close the sketch to make a plate."
                        .to_owned(),
                );
            }
            None => {}
        }
    }
    if lines.is_empty() {
        return Err(
            "The sketch is empty: draw a closed shape (a plate) or connected lines (a profile)."
                .to_owned(),
        );
    }
    let same = |p: DVec2, q: DVec2| p.distance(q) <= 1e-6;
    let degree = |p: DVec2| {
        lines
            .iter()
            .map(|&(_, a, b)| usize::from(same(a, p)) + usize::from(same(b, p)))
            .sum::<usize>()
    };
    for &(_, a, b) in &lines {
        if degree(a) > 2 || degree(b) > 2 {
            return Err(
                "The profile branches: an open profile must be a single chain of lines.".to_owned(),
            );
        }
    }
    let start = lines
        .iter()
        .filter_map(|&(id, a, b)| {
            if degree(a) == 1 {
                Some((id, a))
            } else if degree(b) == 1 {
                Some((id, b))
            } else {
                None
            }
        })
        .min_by_key(|(id, _)| *id)
        .ok_or("The lines don't form an open chain.")?;
    let mut used = vec![false; lines.len()];
    let mut chain = Vec::new();
    let mut at = start.1;
    loop {
        let next = lines
            .iter()
            .enumerate()
            .find(|(i, (_, a, b))| !used[*i] && (same(*a, at) || same(*b, at)));
        let Some((i, &(id, a, b))) = next else {
            break;
        };
        used[i] = true;
        let (from, to) = if same(a, at) { (a, b) } else { (b, a) };
        chain.push(ChainLine {
            entity: id,
            a: from,
            b: to,
        });
        at = to;
    }
    if used.iter().any(|u| !u) {
        return Err(
            "The lines aren't all connected: an open profile must be a single chain of lines."
                .to_owned(),
        );
    }
    Ok(chain)
}

/// The values of an edge flange, evaluated.
pub(crate) fn edge_flange_spec(
    def: &EdgeFlangeFeature,
    params: &Parameters,
) -> Result<EdgeFlangeSpec, String> {
    let value = |s: &crate::Scalar, kind, label: &str| {
        s.evaluate(kind, params)
            .map_err(|m| format!("{label}: {m}."))
    };
    Ok(EdgeFlangeSpec {
        length: value(&def.length, ScalarKind::Length, "Length")?,
        angle: value(&def.angle, ScalarKind::Angle, "Angle")?,
        position: def.position,
        offsets: [
            value(&def.offset_start, ScalarKind::Length, "Start offset")?,
            value(&def.offset_end, ScalarKind::Length, "End offset")?,
        ],
        flip: def.flip,
        radius: def
            .radius
            .as_ref()
            .map(|r| value(r, ScalarKind::Length, "Bend radius"))
            .transpose()?,
    })
}

/// An edge flange: adds a flange to the sheet metal body the edge is on.
pub(crate) fn apply_edge_flange(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    def: &EdgeFlangeFeature,
    spec: &EdgeFlangeSpec,
    seed: u64,
) -> Result<Applied, FeatureError> {
    let edge = def.edge.as_ref().ok_or_else(|| {
        FeatureError(
            "Pick the edge of a flat face of the sheet metal part to put the flange on.".to_owned(),
        )
    })?;
    let found = find_edge(bodies, edge).ok_or_else(|| {
        FeatureError(
            "The edge it refers to no longer exists. Edit the flange and pick another edge."
                .to_owned(),
        )
    })?;
    let body = &bodies[found.body];
    let sheet = body.sheet.as_ref().ok_or_else(|| {
        FeatureError(
            "Edge flanges go on sheet metal bodies: start the part with a base flange.".to_owned(),
        )
    })?;
    let site = sheet
        .edge_site(&body.solid, found.id)
        .map_err(FeatureError)?;
    let mut layout = sheet.layout.clone();
    layout
        .add_edge_flange(feature.0, &site, spec)
        .map_err(FeatureError)?;
    let (new, warning) = sheet_body(model, layout, body.origin, stamp(seed, found.body))?;
    let mut out = bodies.to_vec();
    out[found.body] = Arc::new(new);
    Ok((out, warning))
}

/// A sheet metal cut: the sketch's regions, mapped into the flat pattern through the face
/// the sketch lies on, cut out of the sheet.
pub(crate) fn apply_sheet_cut(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    face: Option<&FaceRef>,
    plane: &Plane,
    sketch: &Sketch,
    seed: u64,
) -> Result<Applied, FeatureError> {
    let face = face.ok_or_else(|| {
        FeatureError(
            "Sketch the cut on a flat face of the sheet metal part: a sheet metal cut goes square through the sheet from there."
                .to_owned(),
        )
    })?;
    let found = find_face(bodies, face).ok_or_else(|| {
        FeatureError(format!(
            "The face its sketch is on no longer exists ({}).",
            model.describe_face(&face.name)
        ))
    })?;
    let body = &bodies[found.body];
    let sheet = body.sheet.as_ref().ok_or_else(|| {
        FeatureError(
            "The sketch is not on a sheet metal body. Use Cut-Extrude for solid bodies.".to_owned(),
        )
    })?;
    let (_, frame, _) = sheet.flange_face(found.id).ok_or_else(|| {
        FeatureError(
            "Sketch the cut on a flat face of the sheet (a flange's top or bottom), not on a bend or an edge."
                .to_owned(),
        )
    })?;
    let profile = find_regions(sketch);
    if profile.regions.is_empty() {
        return Err(FeatureError(
            "The sketch has no closed region to cut. Draw a closed shape.".to_owned(),
        ));
    }
    let regions: Vec<_> = default_regions(&profile)
        .into_iter()
        .map(|i| profile.regions[i].clone())
        .collect();
    let to_flat = |p: DVec2| frame.to_local(plane.from_plane_coords(p)).truncate();
    let area = Area::from_regions(&regions, feature.0).mapped(&to_flat);
    let mut layout = sheet.layout.clone();
    layout.add_cut(area);
    let before = peet_kernel::validate::measure::volume(&body.solid);
    let (new, warning) = sheet_body(model, layout, body.origin, stamp(seed, found.body))?;
    let after = peet_kernel::validate::measure::volume(&new.solid);
    if (before - after).abs() <= 1e-9 * before.abs().max(1.0) {
        return Err(FeatureError(
            "The cut doesn't remove any material: its shape is off the sheet.".to_owned(),
        ));
    }
    let mut out = bodies.to_vec();
    out[found.body] = Arc::new(new);
    Ok((out, warning))
}

/// Where to draw the length handle of an edge flange: the middle of its far end (on the
/// mid-surface) and the unit direction the flange runs in.
pub fn edge_flange_handle(bodies: &[Arc<Body>], feature: FeatureId) -> Option<(DVec3, DVec3)> {
    for body in bodies {
        let Some(sheet) = &body.sheet else { continue };
        let pieces = &sheet.layout.pieces;
        let bend = pieces.iter().find_map(|p| match &p.kind {
            PieceKind::Bend(b) if p.origin.owner == feature.0 => Some(*b),
            _ => None,
        });
        let Some(bend) = bend else { continue };
        let child = &pieces[bend.child?];
        let tip = child.outline.edges().find(|e| {
            matches!(e.tag, CurveTag::Generated { owner, index: wall::TIP, .. } if owner == feature.0)
        })?;
        let mid = tip.curve.point_at(0.5);
        let t = sheet.layout.settings.thickness;
        return Some((
            child.frame.to_world(mid.extend(t / 2.0)),
            child.frame.vector_to_world(bend.across.extend(0.0)),
        ));
    }
    None
}

// ---- Features added in Phase 5 ----

/// A flat face of a sheet metal body that a sketch is drawn on.
pub(crate) struct OnFace<'a> {
    pub body: usize,
    pub sheet: &'a SheetBody,
    /// The flange the face belongs to.
    pub piece: usize,
    /// The flange's placement: model from flat.
    pub frame: Frame,
    /// Whether the face is the sheet's top side.
    pub top: bool,
}

/// The flat face of a sheet metal body a feature's sketch is on. `what` names the
/// feature's purpose for messages ("a cut", "a bend line").
pub(crate) fn sketch_face<'a>(
    model: &Model,
    bodies: &'a [Arc<Body>],
    face: Option<&FaceRef>,
    what: &str,
) -> Result<OnFace<'a>, FeatureError> {
    let face = face.ok_or_else(|| {
        FeatureError(format!(
            "Sketch {what} on a flat face of the sheet metal part."
        ))
    })?;
    let found = find_face(bodies, face).ok_or_else(|| {
        FeatureError(format!(
            "The face its sketch is on no longer exists ({}).",
            model.describe_face(&face.name)
        ))
    })?;
    let body = &bodies[found.body];
    let sheet = body.sheet.as_ref().ok_or_else(|| {
        FeatureError(format!(
            "The sketch is not on a sheet metal body: sketch {what} on a face of one."
        ))
    })?;
    let (piece, frame, top) = sheet.flange_face(found.id).ok_or_else(|| {
        FeatureError(format!(
            "Sketch {what} on a flat face of the sheet (a flange's top or bottom), not on a bend or an edge."
        ))
    })?;
    Ok(OnFace {
        body: found.body,
        sheet,
        piece,
        frame,
        top,
    })
}

/// The sketch face a feature's sketch is on, as a reference.
pub(crate) fn sketch_face_ref(model: &Model, sketch: FeatureId) -> Option<FaceRef> {
    match model.sketch(sketch).map(|s| &s.plane) {
        Some(crate::PlaneRef::Face(f)) => Some(f.clone()),
        _ => None,
    }
}

/// Replaces a sheet metal body with one built from `layout`.
pub(crate) fn rebuild_sheet(
    model: &Model,
    bodies: &[Arc<Body>],
    index: usize,
    layout: Layout,
    seed: u64,
) -> Result<Applied, FeatureError> {
    let (new, warning) = sheet_body(model, layout, bodies[index].origin, stamp(seed, index))?;
    let mut out = bodies.to_vec();
    out[index] = Arc::new(new);
    Ok((out, warning))
}

/// A sheet metal body's edge, as a place for a flange or a hem.
fn edge_on_sheet<'a>(
    bodies: &'a [Arc<Body>],
    edge: Option<&EdgeRef>,
    what: &str,
) -> Result<(usize, &'a SheetBody, EdgeSite), FeatureError> {
    let edge = edge.ok_or_else(|| {
        FeatureError(format!(
            "Pick the edge of a flat face of the sheet metal part to put the {what} on."
        ))
    })?;
    let found = find_edge(bodies, edge).ok_or_else(|| {
        FeatureError(format!(
            "The edge it refers to no longer exists. Edit the {what} and pick another edge."
        ))
    })?;
    let body = &bodies[found.body];
    let sheet = body.sheet.as_ref().ok_or_else(|| {
        FeatureError(format!(
            "A {what} goes on a sheet metal body: start the part with a base flange."
        ))
    })?;
    let site = sheet
        .edge_site(&body.solid, found.id)
        .map_err(FeatureError)?;
    Ok((found.body, sheet, site))
}

/// The values of a hem, evaluated.
pub(crate) fn hem_spec(def: &HemFeature, params: &Parameters) -> Result<HemSpec, String> {
    let value = |s: &crate::Scalar, kind, label: &str| {
        s.evaluate(kind, params)
            .map_err(|m| format!("{label}: {m}."))
    };
    Ok(HemSpec {
        kind: def.kind,
        length: value(&def.length, ScalarKind::Length, "Length")?,
        gap: value(&def.gap, ScalarKind::Length, "Gap")?,
        radius: value(&def.radius, ScalarKind::Length, "Radius")?,
        angle: value(&def.angle, ScalarKind::Angle, "Angle")?,
        inside: def.inside,
        offsets: [
            value(&def.offset_start, ScalarKind::Length, "Start offset")?,
            value(&def.offset_end, ScalarKind::Length, "End offset")?,
        ],
        flip: def.flip,
    })
}

/// A hem: folds an edge of a sheet metal body back over it.
pub(crate) fn apply_hem(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    def: &HemFeature,
    spec: &HemSpec,
    seed: u64,
) -> Result<Applied, FeatureError> {
    let (index, sheet, site) = edge_on_sheet(bodies, def.edge.as_ref(), "hem")?;
    let mut layout = sheet.layout.clone();
    layout
        .add_hem(feature.0, &site, spec)
        .map_err(FeatureError)?;
    rebuild_sheet(model, bodies, index, layout, seed)
}

/// The lines of a sketch (not construction geometry): entity, start and end.
fn sketch_lines(sketch: &Sketch) -> Vec<(u32, DVec2, DVec2)> {
    sketch
        .entities()
        .filter(|(_, e)| !e.construction && e.kind().is_curve())
        .filter_map(|(id, _)| match sketch.curve(id) {
            Some(Curve::Line { a, b }) if a.distance(b) > peet_math::tolerance::LINEAR => {
                Some((id.0, a, b))
            }
            _ => None,
        })
        .collect()
}

/// The flange whose material holds a point of the flat pattern.
fn flange_at(layout: &Layout, p: DVec2) -> Option<usize> {
    layout.pieces.iter().position(|pc| {
        pc.is_flange() && pc.outline.contains(p) && !pc.trims.iter().any(|t| t.contains(p))
    })
}

/// A sketch line in flat coordinates: its entity and its two ends.
type FlatLine = (u32, [DVec2; 2]);

/// What a sketched bend or a jog needs: the face, its lines in flat coordinates.
fn bend_lines_on_face<'a>(
    model: &Model,
    bodies: &'a [Arc<Body>],
    face: Option<&FaceRef>,
    plane: &Plane,
    sketch: &Sketch,
) -> Result<(OnFace<'a>, Vec<FlatLine>), FeatureError> {
    let on = sketch_face(model, bodies, face, "the bend line")?;
    let to_flat = |p: DVec2| on.frame.to_local(plane.from_plane_coords(p)).truncate();
    let lines: Vec<FlatLine> = sketch_lines(sketch)
        .into_iter()
        .map(|(e, a, b)| (e, [to_flat(a), to_flat(b)]))
        .collect();
    if lines.is_empty() {
        return Err(FeatureError(
            "The sketch has no line to bend along: draw a line across the face.".to_owned(),
        ));
    }
    Ok((on, lines))
}

/// A sketched bend: bends a flat face along each line of a sketch on it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_sketched_bend(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    face: Option<&FaceRef>,
    plane: &Plane,
    sketch: &Sketch,
    def: &SketchedBendFeature,
    angle: f64,
    radius: Option<f64>,
    seed: u64,
) -> Result<Applied, FeatureError> {
    let (on, lines) = bend_lines_on_face(model, bodies, face, plane, sketch)?;
    let mut layout = on.sheet.layout.clone();
    let spec = SketchedBendSpec {
        angle,
        radius,
        position: def.position,
        up: on.top != def.flip,
        flip_fixed: def.flip_fixed,
    };
    for (entity, line) in lines {
        let mid = (line[0] + line[1]) / 2.0;
        let piece = flange_at(&layout, mid).unwrap_or(on.piece);
        layout
            .add_sketched_bend(feature.0, entity, piece, line, &spec)
            .map_err(FeatureError)?;
    }
    rebuild_sheet(model, bodies, on.body, layout, seed)
}

/// A jog along the one line of a sketch on a flat face.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_jog(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    face: Option<&FaceRef>,
    plane: &Plane,
    sketch: &Sketch,
    def: &JogFeature,
    values: (f64, f64, Option<f64>),
    seed: u64,
) -> Result<Applied, FeatureError> {
    let (on, lines) = bend_lines_on_face(model, bodies, face, plane, sketch)?;
    let [(_, line)] = lines[..] else {
        return Err(FeatureError(
            "A jog follows one line: draw a single line across the face.".to_owned(),
        ));
    };
    let (offset, angle, radius) = values;
    let mut layout = on.sheet.layout.clone();
    let piece = flange_at(&layout, (line[0] + line[1]) / 2.0).unwrap_or(on.piece);
    layout
        .add_jog(
            feature.0,
            piece,
            line,
            &JogSpec {
                offset,
                dimension: def.dimension,
                angle,
                radius,
                position: def.position,
                up: on.top != def.flip,
                flip_fixed: def.flip_fixed,
            },
        )
        .map_err(FeatureError)?;
    rebuild_sheet(model, bodies, on.body, layout, seed)
}

/// A mitre flange: the sketch's profile run along a chain of edges.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_miter_flange(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    def: &MiterFlangeFeature,
    plane: &Plane,
    sketch: &Sketch,
    gap: f64,
    offsets: [f64; 2],
    seed: u64,
) -> Result<Applied, FeatureError> {
    if def.edges.is_empty() {
        return Err(FeatureError(
            "Pick the edges of a flat face for the flange to run along.".to_owned(),
        ));
    }
    let mut body_index = None;
    let mut sites = Vec::new();
    for edge in &def.edges {
        let (index, _, site) = edge_on_sheet(bodies, Some(edge), "flange")?;
        if body_index.is_some_and(|b| b != index) {
            return Err(FeatureError(
                "The edges of a mitre flange must all be on one body.".to_owned(),
            ));
        }
        body_index = Some(index);
        sites.push(site);
    }
    let index = body_index.expect("there are edges");
    let sheet = bodies[index]
        .sheet
        .as_ref()
        .expect("checked by edge_on_sheet");
    let sites = chain_sites(sites).map_err(FeatureError)?;

    // The profile, in the flat pattern's frame of the face the edges are on.
    let chain = open_chain(sketch).map_err(|m| {
        FeatureError(format!(
            "The profile: {m} Draw connected lines square to one of the edges, starting at it."
        ))
    })?;
    let mut points: Vec<DVec2> = vec![chain[0].a];
    points.extend(chain.iter().map(|l| l.b));
    let parent = &sheet.layout.pieces[sites[0].piece];
    let t = sheet.layout.settings.thickness;
    let local: Vec<DVec3> = points
        .iter()
        .map(|&p| parent.frame.to_local(plane.from_plane_coords(p)))
        .collect();
    // The edge the profile is drawn square to, and the profile in its plane.
    let tol = 1e-4;
    let mut profile = None;
    for s in &sites {
        let d = (s.b - s.a).normalize_or_zero();
        let out = DVec2::new(d.y, -d.x);
        let along: Vec<f64> = local.iter().map(|q| (q.truncate() - s.a).dot(d)).collect();
        let spread = along
            .iter()
            .fold(0.0_f64, |m, &w| m.max((w - along[0]).abs()));
        let len = s.a.distance(s.b);
        if spread > tol || along[0] < -tol || along[0] > len + tol {
            continue;
        }
        let pts: Vec<DVec2> = local
            .iter()
            .map(|q| DVec2::new((q.truncate() - s.a).dot(out), q.z))
            .collect();
        let on_edge = |p: DVec2| p.x.abs() <= tol && (p.y.abs() <= tol || (p.y - t).abs() <= tol);
        if on_edge(pts[0]) {
            profile = Some(pts);
        } else if on_edge(pts[pts.len() - 1]) {
            profile = Some(pts.into_iter().rev().collect());
        }
        if profile.is_some() {
            break;
        }
    }
    let profile = profile.ok_or_else(|| {
        FeatureError(
            "The profile must be drawn square to one of the edges, starting at that edge's top or bottom corner. Sketch it on the face at the end of the edge, or on a plane square to it."
                .to_owned(),
        )
    })?;
    let mut layout = sheet.layout.clone();
    layout
        .add_miter_flange(feature.0, &sites, &profile, offsets, gap)
        .map_err(FeatureError)?;
    rebuild_sheet(model, bodies, index, layout, seed)
}

/// Puts edges in chain order: each starting where the one before ends (they may close a
/// loop).
fn chain_sites(mut sites: Vec<EdgeSite>) -> Result<Vec<EdgeSite>, String> {
    let same = |p: DVec2, q: DVec2| p.distance(q) <= 1e-6;
    // Start where no other edge ends (or anywhere, for a closed loop).
    let start = (0..sites.len())
        .find(|&i| !sites.iter().any(|s| same(s.b, sites[i].a)))
        .unwrap_or(0);
    let mut out = vec![sites.remove(start)];
    while !sites.is_empty() {
        let end = out[out.len() - 1].b;
        let next = sites.iter().position(|s| same(s.a, end)).ok_or(
            "The edges must form one connected chain along the face (pick them all on the same side of the sheet).",
        )?;
        out.push(sites.remove(next));
    }
    Ok(out)
}

/// The values of a corner treatment, evaluated.
pub(crate) fn corner_spec(def: &CornerFeature, params: &Parameters) -> Result<CornerSpec, String> {
    let value = |s: &crate::Scalar, label: &str| {
        s.evaluate(ScalarKind::Length, params)
            .map_err(|m| format!("{label}: {m}."))
    };
    let gap = value(&def.gap, "Gap")?;
    let relief_size = value(&def.relief_size, "Relief size")?;
    if gap < 0.0 || relief_size < 0.0 {
        return Err("The gap and the relief size can't be negative.".to_owned());
    }
    Ok(CornerSpec {
        kind: def.kind,
        gap,
        relief: def.relief,
        relief_size,
    })
}

/// A corner treatment: changes how the flanges meet at the corners of the picked faces.
pub(crate) fn apply_corner(
    model: &Model,
    bodies: &[Arc<Body>],
    def: &CornerFeature,
    spec: CornerSpec,
    seed: u64,
) -> Result<Applied, FeatureError> {
    // The faces' body (or the first sheet metal body), and which corners they pick.
    let mut index = None;
    let mut picks: Vec<(usize, usize)> = Vec::new(); // (face's piece, face's end or 2: both)
    for f in &def.faces {
        let found = find_face(bodies, f).ok_or_else(|| {
            FeatureError(format!(
                "The face it refers to no longer exists ({}). Edit it and pick another.",
                model.describe_face(&f.name)
            ))
        })?;
        if index.is_some_and(|i| i != found.body) {
            return Err(FeatureError(
                "The faces must all be on one sheet metal body.".to_owned(),
            ));
        }
        index = Some(found.body);
        let sheet = bodies[found.body].sheet.as_ref().ok_or_else(|| {
            FeatureError("Corners are of sheet metal bodies: pick a flange's face.".to_owned())
        })?;
        let tag = sheet.faces[found.id.index()];
        let end = match tag {
            FaceTag::Wall {
                tag: CurveTag::Generated { index, .. },
                ..
            } if index == wall::FLANGE_START => 0,
            FaceTag::Wall {
                tag: CurveTag::Generated { index, .. },
                ..
            } if index == wall::FLANGE_END => 1,
            _ => 2,
        };
        picks.push((tag.piece(), end));
    }
    let index = match index {
        Some(i) => i,
        None => bodies
            .iter()
            .position(|b| b.sheet.is_some())
            .ok_or_else(|| FeatureError("There is no sheet metal body with corners.".to_owned()))?,
    };
    let sheet = bodies[index].sheet.as_ref().expect("checked");
    let mut layout = sheet.layout.clone();
    let mut changed = 0;
    for c in &mut layout.corners {
        let hit = def.faces.is_empty()
            || picks.iter().any(|&(piece, end)| {
                c.attachments.iter().zip(c.ends).any(|(&a, e)| {
                    let att = &sheet.layout.attachments[a];
                    att.pieces.contains(&piece) && (end == 2 || end == e)
                })
            });
        if hit {
            c.spec = spec;
            changed += 1;
        }
    }
    if changed == 0 {
        return Err(FeatureError(
            if def.faces.is_empty() {
                "The part has no corners where two flanges meet."
            } else {
                "None of the picked faces is at a corner where two flanges meet. Pick the end face of a flange at a corner."
            }
            .to_owned(),
        ));
    }
    rebuild_sheet(model, bodies, index, layout, seed)
}

/// The outlines of a form feature's sketch, in sketch coordinates: circles and convex
/// polygons, in a stable order (by the lowest entity of each).
fn form_shapes(sketch: &Sketch) -> Vec<(u32, FormShape)> {
    let profile = find_regions(sketch);
    let mut out: Vec<(u32, FormShape)> = Vec::new();
    for r in &profile.regions {
        let l = &r.outer;
        let key = l.edges.iter().map(|e| e.entity.0).min().unwrap_or(0);
        let shape = match round(l) {
            Some(circle) => Some(circle),
            None if l
                .edges
                .iter()
                .all(|e| matches!(e.curve, Curve::Line { .. })) =>
            {
                // Start at the side with the lowest entity, so the sides keep their
                // numbers when the sketch is edited.
                let first = l.edges.iter().position(|e| e.entity.0 == key).unwrap_or(0);
                let n = l.edges.len();
                let pts = (0..n)
                    .map(|k| {
                        let e = &l.edges[(first + k) % n];
                        if e.reversed {
                            e.curve.end()
                        } else {
                            e.curve.start()
                        }
                    })
                    .collect();
                Some(FormShape::Polygon(pts))
            }
            None => None,
        };
        if let Some(s) = shape {
            out.push((key, s));
        }
    }
    out.sort_by_key(|(k, _)| *k);
    out
}

/// A loop that is one whole circle (as a circle, or as arcs of it).
fn round(l: &peet_sketch::region::Loop) -> Option<FormShape> {
    let mut circle: Option<(DVec2, f64)> = None;
    let mut turned = 0.0;
    for e in &l.edges {
        let (c, r, sweep) = match e.curve {
            Curve::Circle { center, radius } => (center, radius, std::f64::consts::TAU),
            Curve::Arc {
                center,
                radius,
                sweep,
                ..
            } => (center, radius, sweep),
            Curve::Line { .. } => return None,
        };
        match circle {
            None => circle = Some((c, r)),
            Some((c0, r0)) if c0.distance(c) < 1e-9 && (r0 - r).abs() < 1e-9 => {}
            Some(_) => return None,
        }
        turned += sweep;
    }
    let (center, radius) = circle?;
    ((turned - std::f64::consts::TAU).abs() < 1e-6).then_some(FormShape::Circle { center, radius })
}

/// Maps a form's shape from sketch coordinates through `f`.
pub(crate) fn map_shape(shape: &FormShape, f: impl Fn(DVec2) -> DVec2) -> FormShape {
    match shape {
        FormShape::Circle { center, radius } => FormShape::Circle {
            center: f(*center),
            radius: *radius,
        },
        FormShape::Polygon(p) => FormShape::Polygon(p.iter().map(|&q| f(q)).collect()),
    }
}

/// The outlines of a form feature's sketch, each with the lowest entity it is made of.
pub(crate) type Outlines = Vec<(u32, FormShape)>;

/// The forms a form feature makes, in sketch coordinates, with a warning about shapes
/// it had to leave out.
pub(crate) fn form_outlines(
    sketch: &Sketch,
    kind: FormKind,
) -> Result<(Outlines, Option<String>), FeatureError> {
    let all = form_shapes(sketch);
    let total = find_regions(sketch).regions.len();
    let wanted: Vec<(u32, FormShape)> = all
        .into_iter()
        .filter(|(_, s)| matches!(s, FormShape::Circle { .. }) == (kind == FormKind::Dimple))
        .collect();
    let what = kind.label().to_lowercase();
    if wanted.is_empty() {
        return Err(FeatureError(if kind == FormKind::Dimple {
            "Draw circles for the dimples, on a flat face of the sheet.".to_owned()
        } else {
            format!(
                "Draw closed shapes of straight lines (rectangles, polygons) for the {what}s, on a flat face of the sheet."
            )
        }));
    }
    let left_out = total - wanted.len();
    let warning = (left_out > 0).then(|| {
        format!(
            "{left_out} shape(s) in the sketch can't be a {what} and were left out (dimples are circles; embosses and louvers are polygons of lines)."
        )
    });
    Ok((wanted, warning))
}

/// Forms from the shapes of a sketch on a flat face.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_form(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    face: Option<&FaceRef>,
    plane: &Plane,
    sketch: &Sketch,
    def: &FormFeature,
    height: f64,
    seed: u64,
) -> Result<Applied, FeatureError> {
    let what = def.kind.label().to_lowercase();
    let on = sketch_face(model, bodies, face, &format!("the {what}"))?;
    let (shapes, mut warning) = form_outlines(sketch, def.kind)?;
    let to_flat = |p: DVec2| on.frame.to_local(plane.from_plane_coords(p)).truncate();
    let mut layout = on.sheet.layout.clone();
    for (part, (_, shape)) in shapes.iter().enumerate() {
        let shape = map_shape(shape, to_flat);
        let open_side = match &shape {
            FormShape::Polygon(p) if def.kind == FormKind::Louver => {
                Some(def.open_side as usize % p.len())
            }
            _ => None,
        };
        layout
            .add_form(Form {
                owner: feature.0,
                part: part as u32,
                piece: on.piece,
                kind: def.kind,
                shape,
                height,
                up: on.top != def.flip,
                open_side,
            })
            .map_err(|m| {
                FeatureError(if shapes.len() > 1 {
                    format!("Shape {}: {m}", part + 1)
                } else {
                    m
                })
            })?;
    }
    let (out, w) = rebuild_sheet(model, bodies, on.body, layout, seed)?;
    if w.is_some() {
        warning = w;
    }
    Ok((out, warning))
}
