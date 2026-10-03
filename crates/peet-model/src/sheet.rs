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

use peet_math::{DVec2, DVec3, Plane};
use peet_sheetmetal::layout::wall;
use peet_sheetmetal::{
    Area, BendModel, ChainLine, CurveTag, EdgeFlangeSpec, FaceTag, Layout, PieceKind, SheetBody,
    SheetSettings, build,
};
use peet_sketch::expr::Parameters;
use peet_sketch::region::find_regions;
use peet_sketch::{Curve, EntityId, Sketch};

use crate::extrude::{default_regions, stamp};
use crate::feature::{
    BaseFlangeFeature, BendModelDef, EdgeFlangeFeature, FeatureId, ScalarKind, SheetSettingsDef,
};
use crate::naming::{Body, FaceName, FaceRef, FaceRole, find_edge, find_face};
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
                FaceTag::Wall { tag, .. } => match tag {
                    CurveTag::Sketch { owner, entity } => {
                        FaceName::new(FeatureId(owner), FaceRole::Side(EntityId(entity)))
                    }
                    CurveTag::Generated { owner, part, index } => {
                        FaceName::new(FeatureId(owner), FaceRole::Wall(part, index))
                    }
                },
            }
        })
        .collect()
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
        let child = &pieces[bend.child];
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
