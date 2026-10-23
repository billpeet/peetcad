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
            "The sheet is in {pieces} separate pieces: cuts or reliefs cut part of it off. Check the cuts, the flanges' offsets from the ends of their edges, and the base flange's relief (its type and its ratio)."
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

/// Sheet metal is made of lines and arcs: refuses a sketch whose profile has a spline.
fn refuse_splines(sketch: &Sketch) -> Result<(), FeatureError> {
    let spline = sketch
        .entities()
        .any(|(_, e)| !e.construction && e.kind() == peet_sketch::EntityKind::Spline);
    if spline {
        Err(FeatureError(peet_sheetmetal::SPLINE_REFUSAL.to_owned()))
    } else {
        Ok(())
    }
}

/// A base flange: a new sheet metal body.
pub(crate) fn apply_base_flange(
    input: &BaseFlangeInput<'_>,
    bodies: &[Arc<Body>],
) -> Result<Applied, FeatureError> {
    if input.def.tab {
        return apply_tab(
            input.feature,
            input.model,
            bodies,
            &input.plane,
            input.sketch,
            input.stamp,
        );
    }
    let owner = input.feature.0;
    refuse_splines(input.sketch)?;
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
        // Each bend's own model, where it has one
        let models = input
            .def
            .bends
            .iter()
            .enumerate()
            .map(|(k, b)| {
                bend_model(b.as_ref(), &input.model.parameters)
                    .map_err(|m| FeatureError(format!("Bend {}: {m}", k + 1)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Layout::open_profile_with(
            input.settings,
            owner,
            &input.plane,
            &chain,
            depth,
            input.def.reverse,
            &models,
        )
    }
    .map_err(FeatureError)?;
    let (body, warning) = sheet_body(input.model, layout, input.feature, stamp(input.stamp, 0))?;
    let mut out = bodies.to_vec();
    out.push(Arc::new(body));
    Ok((out, warning))
}

/// The closed regions of a tab's sketch.
pub(crate) fn tab_regions(
    sketch: &Sketch,
) -> Result<Vec<peet_sketch::region::Region>, FeatureError> {
    refuse_splines(sketch)?;
    let profile = find_regions(sketch);
    let regions: Vec<_> = default_regions(&profile)
        .into_iter()
        .map(|i| profile.regions[i].clone())
        .collect();
    if regions.is_empty() {
        return Err(FeatureError(
            "A tab is a closed sketch: close its outline.".to_owned(),
        ));
    }
    Ok(regions)
}

/// A tab: the regions of a closed sketch on a flat face of a sheet metal body (or in its
/// plane) added to that face's flange, with the body's thickness.
pub(crate) fn apply_tab(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    plane: &Plane,
    sketch: &Sketch,
    seed: u64,
) -> Result<Applied, FeatureError> {
    let regions = tab_regions(sketch)?;
    for (index, body) in bodies.iter().enumerate() {
        let Some(sheet) = &body.sheet else {
            continue;
        };
        let t = sheet.layout.settings.thickness;
        let area = |frame: &Frame| {
            Area::from_regions(&regions, feature.0)
                .mapped(&|p| frame.to_local(plane.from_plane_coords(p)).truncate())
        };
        if let Some((piece, area)) = flange_beside(
            &sheet.layout,
            plane.origin(),
            plane.normal(),
            &[0.0, t],
            &area,
        ) {
            let mut layout = sheet.layout.clone();
            layout.add_sheet(piece, area);
            return rebuild_sheet(model, bodies, index, layout, seed);
        }
    }
    Err(FeatureError(
        "A tab adds to a sheet metal body: draw its sketch on a flat face of one (or in the face's plane), touching the sheet."
            .to_owned(),
    ))
}

/// The flange with a face in the plane through `origin` square to `normal` (model
/// coordinates), `levels` above its bottom side, that has sheet next to an area. `area`
/// gives the area in a flange's flat coordinates from the flange's placement; it comes
/// back in the flange's found.
pub(crate) fn flange_beside(
    layout: &Layout,
    origin: DVec3,
    normal: DVec3,
    levels: &[f64],
    area: &dyn Fn(&Frame) -> Area,
) -> Option<(usize, Area)> {
    const NEAR: f64 = 1e-3;
    let dirs = [
        DVec2::X,
        DVec2::Y,
        -DVec2::X,
        -DVec2::Y,
        DVec2::new(1.0, 1.0).normalize(),
        DVec2::new(1.0, -1.0).normalize(),
        DVec2::new(-1.0, 1.0).normalize(),
        DVec2::new(-1.0, -1.0).normalize(),
    ];
    layout.pieces.iter().enumerate().find_map(|(i, p)| {
        if !p.is_flange() || p.frame.z_axis().cross(normal).length() > 1e-9 {
            return None;
        }
        let level = p.frame.to_local(origin).z;
        if !levels.iter().any(|l| (level - l).abs() <= 1e-6) {
            return None;
        }
        let a = area(&p.frame);
        let touches = a.edges().any(|e| {
            (0..=4).any(|k| {
                let q = e.curve.point_at(f64::from(k) / 4.0);
                dirs.iter().any(|d| p.covers(q + *d * NEAR))
            })
        });
        touches.then_some((i, a))
    })
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

/// An edge flange's own relief, evaluated: its type and its size as a multiple of the
/// thickness, each the body's (`settings`) unless the flange has its own. `None` if it
/// has neither.
pub(crate) fn edge_flange_relief(
    def: &EdgeFlangeFeature,
    settings: &peet_sheetmetal::SheetSettings,
    params: &Parameters,
) -> Result<Option<(peet_sheetmetal::ReliefType, f64)>, String> {
    if def.relief.is_none() && def.relief_ratio.is_none() {
        return Ok(None);
    }
    let ratio = match &def.relief_ratio {
        Some(r) => r
            .evaluate(ScalarKind::Number, params)
            .map_err(|m| format!("Relief ratio: {m}."))?,
        None => settings.relief_ratio,
    };
    Ok(Some((def.relief.unwrap_or(settings.relief), ratio)))
}

/// An edge flange's own bend model, evaluated. `None` if it uses the body's.
pub(crate) fn edge_flange_bend(
    def: &EdgeFlangeFeature,
    params: &Parameters,
) -> Result<Option<BendModel>, String> {
    bend_model(def.bend.as_ref(), params)
}

/// A bend model of a feature's own, evaluated. `None` for the body's.
pub(crate) fn bend_model(
    def: Option<&BendModelDef>,
    params: &Parameters,
) -> Result<Option<BendModel>, String> {
    let value = |s: &crate::Scalar, kind, label: &str| {
        s.evaluate(kind, params)
            .map_err(|m| format!("{label}: {m}."))
    };
    Ok(match def {
        None => None,
        Some(BendModelDef::KFactor(k)) => {
            Some(BendModel::KFactor(value(k, ScalarKind::Number, "K-factor")?))
        }
        Some(BendModelDef::Allowance(v)) => Some(BendModel::Allowance(value(
            v,
            ScalarKind::Length,
            "Bend allowance",
        )?)),
        Some(BendModelDef::Deduction(v)) => Some(BendModel::Deduction(value(
            v,
            ScalarKind::Length,
            "Bend deduction",
        )?)),
    })
}

/// An edge flange: adds a flange to the sheet metal body the edge is on.
pub(crate) fn apply_edge_flange(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    def: &EdgeFlangeFeature,
    spec: &EdgeFlangeSpec,
    outline: Option<(Plane, &Sketch)>,
    up_to: Option<peet_math::DVec3>,
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
    let relief =
        edge_flange_relief(def, &layout.settings, &model.parameters).map_err(FeatureError)?;
    let own = peet_sheetmetal::FlangeOwn {
        relief,
        model: edge_flange_bend(def, &model.parameters).map_err(FeatureError)?,
        set_out: edge_flange_position_offset(def, &model.parameters).map_err(FeatureError)?,
    };
    // Up to a point: as long as takes its end there
    let mut spec = *spec;
    if let Some(point) = up_to {
        spec.length = layout
            .edge_flange_length_to(feature.0, 0, &site, &spec, own, point)
            .map_err(FeatureError)?;
    }
    let spec = &spec;
    match outline {
        None => layout.add_edge_flange_with(feature.0, &site, spec, own),
        Some((plane, sketch)) => {
            // The sketch's regions, projected onto the flange: its flat placement takes
            // them to flat coordinates
            refuse_splines(sketch)?;
            let profile = find_regions(sketch);
            let regions: Vec<_> = default_regions(&profile)
                .into_iter()
                .map(|i| profile.regions[i].clone())
                .collect();
            if regions.is_empty() {
                return Err(FeatureError(
                    "Its outline sketch has no closed region: draw the flange's shape.".to_owned(),
                ));
            }
            let shape = |frame: &Frame| {
                let to_flat = |p: DVec2| frame.to_local(plane.from_plane_coords(p)).truncate();
                Area::from_regions(&regions, feature.0).mapped(&to_flat)
            };
            layout.add_edge_flange_outlined(feature.0, 0, &site, spec, own, &shape)
        }
    }
    .map_err(FeatureError)?;
    let (new, warning) = sheet_body(model, layout, body.origin, stamp(seed, found.body))?;
    let mut out = bodies.to_vec();
    out[found.body] = Arc::new(new);
    Ok((out, warning))
}

/// A sheet metal cut: the sketch's regions, mapped into the flat pattern through the face
/// the sketch lies on, cut out of the sheet. Other flanges with a face in the sketch's
/// plane are cut too, each through its own place in the flat pattern: two tabs side by
/// side are far apart in the blank, and a sketch across both means both.
pub(crate) fn apply_sheet_cut(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    face: Option<&FaceRef>,
    plane: &Plane,
    sketch: &Sketch,
    reach: Option<(f64, f64)>,
    seed: u64,
) -> Result<Applied, FeatureError> {
    let found = match face {
        Some(face) => Some(find_face(bodies, face).ok_or_else(|| {
            FeatureError(format!(
                "The face its sketch is on no longer exists ({}).",
                model.describe_face(&face.name)
            ))
        })?),
        None => None,
    };
    if let Some(found) = &found
        && bodies[found.body].sheet.is_none()
    {
        return Err(FeatureError(
            "The sketch is not on a sheet metal body. Use Cut-Extrude for solid bodies.".to_owned(),
        ));
    }
    // A sketch on a plane, or on a face that is not a flange's top or bottom (the sheet's
    // edge): cut from there, as SolidWorks' cut-extrude does from any plane.
    let on_flange = found.as_ref().and_then(|found| {
        let sheet = bodies[found.body].sheet.as_ref()?;
        Some((found.body, sheet, sheet.flange_face(found.id)?))
    });
    let Some((body_index, sheet, (on_piece, frame, top))) = on_flange else {
        return sheet_cut_anywhere(
            feature,
            model,
            bodies,
            found.map(|f| f.body),
            plane,
            sketch,
            reach,
            seed,
        );
    };
    let body = &bodies[body_index];
    refuse_splines(sketch)?;
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
    // The other flanges that lie in the sketch's plane. One that the first mapping already
    // puts in the right place (it is next to the sketch's flange in the blank too) is left.
    let thickness = layout.settings.thickness;
    let origin = plane.from_plane_coords(DVec2::ZERO);
    let probes = [DVec2::ZERO, DVec2::X, DVec2::Y];
    let mut cut = vec![false; layout.pieces.len()];
    cut[on_piece] = true;
    for i in 0..layout.pieces.len() {
        let piece = &layout.pieces[i];
        if i == on_piece || !piece.is_flange() {
            continue;
        }
        let other = piece.frame;
        let level = other.to_local(origin).z;
        let in_plane = other.z_axis().dot(frame.z_axis()).abs() > 1.0 - 1e-9
            && (level.abs() < 1e-6 || (level - thickness).abs() < 1e-6);
        if !in_plane {
            continue;
        }
        cut[i] = true;
        let through = |p: DVec2| other.to_local(plane.from_plane_coords(p)).truncate();
        if probes.iter().all(|p| through(*p).distance(to_flat(*p)) < 1e-6) {
            continue;
        }
        let area = Area::from_regions(&regions, feature.0).mapped(&through);
        layout.add_cut_on(area, vec![i]);
    }
    if let Some(span) = reach {
        // Into the sheet from the face the sketch is on
        let into = frame.z_axis() * if top { -1.0 } else { 1.0 };
        let world = |p: DVec2| plane.from_plane_coords(p);
        for (area, pieces) in cut_beyond(&layout, &cut, &world, into, span, &regions, feature.0) {
            layout.add_cut_on(area, pieces);
        }
    }
    // A shape over sheet that an earlier cut has taken away already: SolidWorks keeps such a
    // cut (a pattern of it can cut elsewhere), so it stays, with a warning.
    let over_sheet = regions.iter().any(|r| {
        let (lo, hi) = r.outer.bounds();
        let c = (lo + hi) / 2.0;
        r.contains(c) && layout.pieces[on_piece].covers(to_flat(c))
    });
    let before = peet_kernel::validate::measure::volume(&body.solid);
    let (new, mut warning) = sheet_body(model, layout, body.origin, stamp(seed, body_index))?;
    let after = peet_kernel::validate::measure::volume(&new.solid);
    if (before - after).abs() <= 1e-9 * before.abs().max(1.0) {
        if !over_sheet {
            return Err(FeatureError(
                "The cut doesn't remove any material: its shape is off the sheet.".to_owned(),
            ));
        }
        warning.get_or_insert_with(|| {
            "The cut removes nothing: an earlier cut has taken the sheet there away already."
                .to_owned()
        });
    }
    let mut out = bodies.to_vec();
    out[body_index] = Arc::new(new);
    Ok((out, warning))
}

/// A sheet metal cut whose sketch is not on a flange's top or bottom: on a plane, or on a
/// face along the sheet's edge. It cuts as SolidWorks' cut-extrude does from such a sketch:
/// through the flanges with a face in the sketch's plane, and with a reach, through every
/// flange the reach takes in ([`cut_beyond`]), going in against the sketch plane's normal
/// (into the face, for a sketch on one). `body` is the body of the face the sketch is on;
/// for a sketch on a plane every sheet metal body is cut.
#[allow(clippy::too_many_arguments)]
fn sheet_cut_anywhere(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    body: Option<usize>,
    plane: &Plane,
    sketch: &Sketch,
    reach: Option<(f64, f64)>,
    seed: u64,
) -> Result<Applied, FeatureError> {
    refuse_splines(sketch)?;
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
    let world = |p: DVec2| plane.from_plane_coords(p);
    let into = -plane.normal();
    let mut out = bodies.to_vec();
    let mut warning = None;
    let mut cut_any = false;
    for (index, b) in bodies.iter().enumerate() {
        let Some(sheet) = &b.sheet else { continue };
        if body.is_some_and(|i| i != index) {
            continue;
        }
        let cuts = cut_from_plane(&sheet.layout, &world, into, reach, &regions, feature.0);
        if cuts.is_empty() {
            continue;
        }
        let mut layout = sheet.layout.clone();
        for (area, pieces) in cuts {
            layout.add_cut_on(area, pieces);
        }
        let before = peet_kernel::validate::measure::volume(&b.solid);
        let (new, w) = sheet_body(model, layout, b.origin, stamp(seed, index))?;
        let after = peet_kernel::validate::measure::volume(&new.solid);
        if (before - after).abs() > 1e-9 * before.abs().max(1.0) {
            cut_any = true;
            warning = warning.or(w);
            out[index] = Arc::new(new);
        }
    }
    if !cut_any {
        return Err(FeatureError(if reach.is_none() {
            "Its sketch is not on a flange's top or bottom, and no flange has a face in its plane. \
             Sketch the cut on a flat face of the sheet, or give it an end (blind or through all) \
             to cut from where the sketch is."
                .to_owned()
        } else {
            "The cut doesn't remove any material: nothing of the sheet is within its reach \
             under its shape. Check its direction (reverse) and its depth."
                .to_owned()
        }));
    }
    Ok((out, warning))
}

/// Whether a sheet metal cut's sketch is off a flange's top or bottom, and is cut from as
/// [`sheet_cut_anywhere`] does: `Some(None)` for a sketch on a plane, `Some(Some(body))` for
/// one on a face of a sheet metal body that is not a flange's top or bottom (its edge).
pub(crate) fn off_flange(bodies: &[Arc<Body>], face: Option<&FaceRef>) -> Option<Option<usize>> {
    let Some(face) = face else { return Some(None) };
    let found = find_face(bodies, face)?;
    let sheet = bodies[found.body].sheet.as_ref()?;
    sheet.flange_face(found.id).is_none().then_some(Some(found.body))
}

/// The cuts a sheet metal cut makes in `layout` from a sketch placed in the model by
/// `world`, going `into` from its plane, each with the pieces it is cut from: the flanges
/// with a face in the sketch's plane, cut square through by the regions, and with a reach,
/// the others it takes in ([`cut_beyond`]).
pub(crate) fn cut_from_plane(
    layout: &Layout,
    world: &dyn Fn(DVec2) -> DVec3,
    into: DVec3,
    reach: Option<(f64, f64)>,
    regions: &[peet_sketch::region::Region],
    owner: u32,
) -> Vec<(Area, Vec<usize>)> {
    let t = layout.settings.thickness;
    let origin = world(DVec2::ZERO);
    let mut done = vec![false; layout.pieces.len()];
    let mut out = Vec::new();
    for (i, piece) in layout.pieces.iter().enumerate() {
        if !piece.is_flange() || piece.frame.z_axis().cross(into).length() > 1e-9 {
            continue;
        }
        let level = piece.frame.to_local(origin).z;
        if level.abs() < 1e-6 || (level - t).abs() < 1e-6 {
            done[i] = true;
            let through = |p: DVec2| piece.frame.to_local(world(p)).truncate();
            out.push((Area::from_regions(regions, owner).mapped(&through), vec![i]));
        }
    }
    if let Some(span) = reach {
        out.extend(cut_beyond(layout, &done, world, into, span, regions, owner));
    }
    out
}

/// How far out from its edge an edge flange is set, evaluated.
pub(crate) fn edge_flange_position_offset(
    def: &EdgeFlangeFeature,
    params: &Parameters,
) -> Result<f64, String> {
    def.position_offset
        .evaluate(ScalarKind::Length, params)
        .map_err(|m| format!("Position offset: {m}."))
}

/// How far a sheet metal cut reaches along the way into the sheet from the face its sketch
/// is on, measured from that face: `None` for a cut through its own sheet only.
pub(crate) fn sheet_cut_reach(
    c: &crate::feature::SheetCutFeature,
    params: &Parameters,
) -> Result<Option<(f64, f64)>, String> {
    use crate::feature::SheetCutEnd;
    let r = match c.end {
        SheetCutEnd::Sheet => return Ok(None),
        SheetCutEnd::Blind => {
            let d = c
                .depth
                .evaluate(ScalarKind::Length, params)
                .map_err(|m| format!("Depth: {m}."))?;
            if !(d > 0.0) {
                return Err("The depth must be greater than zero.".to_owned());
            }
            d
        }
        SheetCutEnd::ThroughAll => f64::INFINITY,
    };
    let (lo, hi) = if c.both { (-r, r) } else { (0.0, r) };
    Ok(Some(if c.reverse { (-hi, -lo) } else { (lo, hi) }))
}

/// The cuts a sheet metal cut with a reach makes in flanges other than the ones with a
/// face in its sketch's plane (`done`), each with the pieces it is cut from. `world`
/// places the sketch's points in the model, `into` is the way into the sheet, and `span`
/// the reach along it from the sketch's plane.
///
/// A flange parallel to the sketch is cut square through by the regions if it lies
/// wholly within the reach. A flange square to the sketch is cut where the regions cross
/// its middle, as far along `into` as the reach and the flange both go. Flanges at other
/// angles are left alone.
pub(crate) fn cut_beyond(
    layout: &Layout,
    done: &[bool],
    world: &dyn Fn(DVec2) -> DVec3,
    into: DVec3,
    span: (f64, f64),
    regions: &[peet_sketch::region::Region],
    owner: u32,
) -> Vec<(Area, Vec<usize>)> {
    let t = layout.settings.thickness;
    let origin = world(DVec2::ZERO);
    let (ax, ay) = (world(DVec2::X) - origin, world(DVec2::Y) - origin);
    let mut out = Vec::new();
    for (i, piece) in layout.pieces.iter().enumerate() {
        if done.get(i).copied().unwrap_or(true) || !piece.is_flange() {
            continue;
        }
        let z = piece.frame.z_axis();
        let along = z.dot(into);
        let level = (piece.frame.origin - origin).dot(into);
        if along.abs() > 1.0 - 1e-9 {
            let (a, b) = (level, level + t * along);
            if a.min(b) < span.0 - 1e-6 || a.max(b) > span.1 + 1e-6 {
                continue;
            }
            let through = |p: DVec2| piece.frame.to_local(world(p)).truncate();
            out.push((Area::from_regions(regions, owner).mapped(&through), vec![i]));
        } else if along.abs() < 1e-9 {
            // The flange's middle crosses the sketch's plane in a line: m c + u s
            let m = DVec2::new(ax.dot(z), ay.dot(z));
            if m.length() < 1e-9 {
                continue;
            }
            let m = m.normalize();
            let c = t / 2.0 - (origin - piece.frame.origin).dot(z);
            let u = m.perp();
            let at = |s: f64| m * c + u * s;
            // How far along `into` the flange goes
            let mut extent = (f64::INFINITY, f64::NEG_INFINITY);
            for e in piece.outline.edges() {
                for p in [e.curve.start(), e.curve.end()] {
                    let k = (piece.frame.to_world(p.extend(t / 2.0)) - origin).dot(into);
                    extent = (extent.0.min(k), extent.1.max(k));
                }
            }
            let (k0, k1) = (span.0.max(extent.0), span.1.min(extent.1));
            if k1 <= k0 {
                continue;
            }
            // Where the reach goes on past an end of the flange into a bend, the band runs
            // on across the bend in the flat pattern: the bend at that end, and how far.
            let flat_into = piece.frame.to_local(piece.frame.origin + into).truncate();
            let past = |end: f64, ahead: DVec2| -> Option<(usize, f64)> {
                if end <= 1e-6 {
                    return None;
                }
                layout.bends().find_map(|(j, b)| {
                    let out = if b.parent == i {
                        b.across
                    } else if b.child == Some(i) {
                        -b.across
                    } else {
                        return None;
                    };
                    (out.dot(ahead) > 1.0 - 1e-6).then_some((j, b.width()))
                })
            };
            let beyond = [
                past(extent.0 - span.0, -flat_into),
                past(span.1 - extent.1, flat_into),
            ];
            // Where the line goes in and out of the regions
            let size = regions
                .iter()
                .map(|r| {
                    let (lo, hi) = r.outer.bounds();
                    lo.abs().max(hi.abs()).max_element()
                })
                .fold(0.0_f64, f64::max)
                + c.abs()
                + 1.0;
            let line = Curve::Line {
                a: at(-size),
                b: at(size),
            };
            let mut stops = vec![-size, size];
            for r in regions {
                for l in std::iter::once(&r.outer).chain(&r.holes) {
                    for e in &l.edges {
                        let hits = line.intersect(&e.curve);
                        stops.extend(hits.iter().map(|h| (h.t_a - 0.5) * 2.0 * size));
                    }
                }
            }
            stops.sort_by(f64::total_cmp);
            let mut bands: Vec<(f64, f64)> = Vec::new();
            for w in stops.windows(2) {
                let inside = regions.iter().any(|r| r.contains(at((w[0] + w[1]) / 2.0)));
                if w[1] - w[0] < 1e-9 || !inside {
                    continue;
                }
                match bands.last_mut() {
                    Some(last) if (last.1 - w[0]).abs() < 1e-9 => last.1 = w[1],
                    _ => bands.push((w[0], w[1])),
                }
            }
            for (b, (s0, s1)) in bands.into_iter().enumerate() {
                let corner =
                    |s: f64, k: f64| piece.frame.to_local(world(at(s)) + into * k).truncate();
                let back = beyond[0].map_or(DVec2::ZERO, |(_, w)| -flat_into * w);
                let ahead = beyond[1].map_or(DVec2::ZERO, |(_, w)| flat_into * w);
                let points = [
                    corner(s0, k0) + back,
                    corner(s1, k0) + back,
                    corner(s1, k1) + ahead,
                    corner(s0, k1) + ahead,
                ];
                // Numbered apart from the flanges' own parts: one per flange and band
                let part = (1u32 << 31) | ((i as u32) << 8) | b as u32;
                let tag = |j: usize| CurveTag::Generated {
                    owner,
                    part,
                    index: j as u8,
                };
                let mut pieces = vec![i];
                pieces.extend(beyond.iter().flatten().map(|(j, _)| *j));
                out.push((Area::polygon(&points, tag), pieces));
            }
        }
    }
    out
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
        pc.is_flange() && pc.covers(p)
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
    let parent = &sheet.layout.pieces[sites[0].piece];
    let t = sheet.layout.settings.thickness;
    let profile = miter_profile(&sites, &parent.frame, plane, sketch, t)?;
    let mut layout = sheet.layout.clone();
    layout
        .add_miter_flange(feature.0, &sites, &profile, offsets, gap)
        .map_err(FeatureError)?;
    rebuild_sheet(model, bodies, index, layout, seed)
}

/// A mitre flange's profile across its edges: its points as distances out from the edge
/// it is drawn square to and heights above the bottom side of the sheet, starting at the
/// edge. `frame` is the placement of the face the edges are on.
pub(crate) fn miter_profile(
    sites: &[EdgeSite],
    frame: &Frame,
    plane: &Plane,
    sketch: &Sketch,
    t: f64,
) -> Result<Vec<DVec2>, FeatureError> {
    // The profile, in the flat pattern's frame of the face the edges are on.
    let chain = open_chain(sketch).map_err(|m| {
        FeatureError(format!(
            "The profile: {m} Draw connected lines square to one of the edges, starting at it."
        ))
    })?;
    let mut points: Vec<DVec2> = vec![chain[0].a];
    points.extend(chain.iter().map(|l| l.b));
    let local: Vec<DVec3> = points
        .iter()
        .map(|&p| frame.to_local(plane.from_plane_coords(p)))
        .collect();
    // The edge the profile is drawn square to, and the profile in its plane.
    let tol = 1e-4;
    let mut profile = None;
    for s in sites {
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
    profile.ok_or_else(|| {
        FeatureError(
            "The profile must be drawn square to one of the edges, starting at that edge's top or bottom corner. Sketch it on the face at the end of the edge, or on a plane square to it."
                .to_owned(),
        )
    })
}

/// Puts edges in chain order: each starting where the one before ends (they may close a
/// loop).
pub(crate) fn chain_sites(mut sites: Vec<EdgeSite>) -> Result<Vec<EdgeSite>, String> {
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

/// A break corner: rounds or chamfers the picked corners of the sheet, in the flat pattern.
pub(crate) fn apply_break_corner(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    def: &crate::feature::BreakCornerFeature,
    size: f64,
    seed: u64,
) -> Result<Applied, FeatureError> {
    if def.edges.is_empty() {
        return Err(FeatureError(
            "Pick the corners to break: the short edges through the sheet where two sides of a flat face meet."
                .to_owned(),
        ));
    }
    let mut index = None;
    let mut cuts = Vec::new();
    for (n, edge) in def.edges.iter().enumerate() {
        let found = find_edge(bodies, edge).ok_or_else(|| {
            FeatureError(
                "A corner it refers to no longer exists. Edit it and pick the corners again."
                    .to_owned(),
            )
        })?;
        if index.is_some_and(|i| i != found.body) {
            return Err(FeatureError(
                "The corners must all be on one sheet metal body.".to_owned(),
            ));
        }
        index = Some(found.body);
        let body = &bodies[found.body];
        let sheet = body.sheet.as_ref().ok_or_else(|| {
            FeatureError(
                "Break corner is for sheet metal bodies. Use Fillet or Chamfer on a solid."
                    .to_owned(),
            )
        })?;
        let tag = |i: u8| CurveTag::Generated {
            owner: feature.0,
            part: n as u32,
            index: i,
        };
        cuts.push(
            sheet
                .corner_break(&body.solid, found.id, def.kind, size, tag)
                .map_err(|m| FeatureError(format!("Corner {}: {m}", n + 1)))?,
        );
    }
    let index = index.expect("there are corners");
    // Two corners at the ends of one side share it: what they take of it must not overlap.
    let overlap = |p: &[DVec2; 2], q: &[DVec2; 2]| {
        let d = p[1] - p[0];
        let length = d.length();
        let d = d / length;
        let on_line = |x: DVec2| (x - p[0]).perp_dot(d).abs() < 1e-6;
        if !(on_line(q[0]) && on_line(q[1])) {
            return false;
        }
        let (u0, u1) = ((q[0] - p[0]).dot(d), (q[1] - p[0]).dot(d));
        u0.max(u1).min(length) - u0.min(u1).max(0.0) > 1e-6
    };
    for i in 0..cuts.len() {
        for j in i + 1..cuts.len() {
            if cuts[i].0 == cuts[j].0
                && cuts[i].2.iter().any(|p| cuts[j].2.iter().any(|q| overlap(p, q)))
            {
                return Err(FeatureError(format!(
                    "Corners {} and {} are at the two ends of one side, which is too short for both at that size.",
                    i + 1,
                    j + 1
                )));
            }
        }
    }
    let mut layout = bodies[index].sheet.as_ref().expect("checked").layout.clone();
    for (piece, area, _, adds) in cuts {
        if adds {
            // A rounded inside corner fills in what earlier cuts took there.
            layout.add_sheet(piece, area);
        } else {
            layout.add_cut_on(area, vec![piece]);
        }
    }
    rebuild_sheet(model, bodies, index, layout, seed)
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
        let tag = &sheet.faces[found.id.index()];
        let end = match *tag {
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
            Curve::Line { .. } | Curve::Spline(_) => return None,
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
    refuse_splines(sketch)?;
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
