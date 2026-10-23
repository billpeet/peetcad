//! Patterns and mirrors: copies of features, moved or reflected.
//!
//! A pattern or a mirror copies *features*, not geometry: each copy is the copied
//! feature built again from a moved (or reflected) sketch, so a copy of a cut is a cut,
//! and a copy of a sheet metal cut or a form is made in the flat pattern like the
//! original. The features that can be copied are extrusions (bosses and cuts), revolves,
//! holes (a bolt circle is a circular pattern of one hole), sheet metal cuts, forms and
//! edge flanges.
//!
//! **Names.** Each copy is built as the pattern feature itself, and every face a copy
//! makes also carries [`FaceRole::Instance`] with the copy's number, so the faces of
//! different copies have different names and can be referred to like any other.
//!
//! **Sheet metal copies** stay on the face their original is sketched on: the copy's
//! sketch must land in the same plane (a pattern along the face, or a mirror square to
//! it). A copy that can't be made (a cut that misses the part, a form that runs off the
//! face) is left out with a warning, as long as one copy can be made.
//!
//! **Edge flange copies** go on the edge the motion takes the original's edge to, as
//! SolidWorks' mirrors and patterns of flanges do: the same flange, set back the same from
//! the edge's ends (the other way round for a mirror, which turns the edge round), bent
//! to the side the motion takes the original's bend to.

use std::sync::Arc;

use peet_math::{DMat3, DQuat, DVec2, DVec3, Frame, Plane};
use peet_sheetmetal::{Area, CurveTag, Form, FormKind};
use peet_sketch::Sketch;
use peet_sketch::region::find_regions;

use crate::extrude::{Extrude, ExtrudeInput, apply_extrude, default_regions};
use crate::feature::{Axis, FeatureId, FormFeature};
use crate::hole::{HoleFeature, HoleInput, HoleSizes, apply_hole};
use crate::naming::{Body, FaceName, FaceRef, FaceRole};
use crate::revolve::{RevolveFeature, RevolveInput, apply_revolve};
use crate::sheet::{Applied, form_outlines, map_shape, rebuild_sheet, sketch_face};
use crate::{FeatureError, Model};

/// Where a copy goes: a rigid motion, or a reflection in a plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Motion {
    Rigid(Frame),
    Mirror { point: DVec3, normal: DVec3 },
    /// A reflection in the XY plane, then a rigid motion: what a mirror followed by a
    /// pattern (a copy of a mirror) comes to.
    Reflected(Frame),
}

/// The reflection in the XY plane.
const FLIP_Z: DMat3 = DMat3::from_diagonal(DVec3::new(1.0, 1.0, -1.0));

impl Motion {
    pub fn point(&self, p: DVec3) -> DVec3 {
        match *self {
            Self::Rigid(f) => f.to_world(p),
            Self::Mirror { point, normal } => p - normal * (2.0 * (p - point).dot(normal)),
            Self::Reflected(f) => f.to_world(FLIP_Z * p),
        }
    }

    pub fn vector(&self, v: DVec3) -> DVec3 {
        match *self {
            Self::Rigid(f) => f.vector_to_world(v),
            Self::Mirror { normal, .. } => v - normal * (2.0 * v.dot(normal)),
            Self::Reflected(f) => f.vector_to_world(FLIP_Z * v),
        }
    }

    pub fn is_mirror(&self) -> bool {
        matches!(self, Self::Mirror { .. } | Self::Reflected(_))
    }

    /// The motion as a matrix and a translation: `p ↦ m·p + t`.
    fn affine(&self) -> (DMat3, DVec3) {
        match *self {
            Self::Rigid(f) => (DMat3::from_quat(f.rotation), f.origin),
            Self::Mirror { point, normal } => (
                DMat3::IDENTITY - 2.0 * outer(normal, normal),
                normal * (2.0 * point.dot(normal)),
            ),
            Self::Reflected(f) => (DMat3::from_quat(f.rotation) * FLIP_Z, f.origin),
        }
    }

    /// `self` after `first`: where a copy made by `first` goes when it is copied by
    /// `self` (a copy of a pattern or a mirror).
    pub fn after(&self, first: &Motion) -> Motion {
        let (a, s) = self.affine();
        let (b, t) = first.affine();
        let m = a * b;
        let origin = a * t + s;
        if m.determinant() > 0.0 {
            Self::Rigid(Frame {
                origin,
                rotation: DQuat::from_mat3(&m).normalize(),
            })
        } else {
            Self::Reflected(Frame {
                origin,
                rotation: DQuat::from_mat3(&(m * FLIP_Z)).normalize(),
            })
        }
    }

    /// Where a sketch plane goes. A reflection can't be a frame, so for a mirror the
    /// copy's sketch is mirrored too (its y negated): the plane's y axis is turned round
    /// to make up for it, which keeps the normal where the reflection puts it.
    pub fn sketch_plane(&self, plane: &Plane) -> Option<Plane> {
        match *self {
            Self::Rigid(f) => Some(Plane {
                frame: f.compose(&plane.frame),
            }),
            Self::Mirror { .. } | Self::Reflected(_) => {
                let x = self.vector(plane.frame.x_axis());
                let y = -self.vector(plane.frame.y_axis());
                let frame = Frame::from_origin_z_x(self.point(plane.origin()), x.cross(y), x)?;
                Some(Plane { frame })
            }
        }
    }

    /// Where a plane goes (only its position and normal matter).
    pub fn plane(&self, plane: &Plane) -> Option<Plane> {
        Plane::from_origin_normal_x(
            self.point(plane.origin()),
            self.vector(plane.normal()),
            self.vector(plane.frame.x_axis()),
        )
    }

    /// A translation.
    pub fn translation(by: DVec3) -> Self {
        Self::Rigid(Frame {
            origin: by,
            rotation: DQuat::IDENTITY,
        })
    }

    /// A turn about the axis through `origin` along `dir` (a unit vector).
    pub fn rotation(origin: DVec3, dir: DVec3, angle: f64) -> Self {
        let rotation = DQuat::from_axis_angle(dir, angle);
        Self::Rigid(Frame {
            origin: origin - rotation * origin,
            rotation,
        })
    }
}

/// The matrix `a·bᵀ`.
fn outer(a: DVec3, b: DVec3) -> DMat3 {
    DMat3::from_cols(a * b.x, a * b.y, a * b.z)
}

/// A feature to copy, with its references resolved.
pub(crate) enum Seed<'a> {
    Extrude {
        plane: Plane,
        sketch: &'a Sketch,
        params: &'a Extrude,
        depth: f64,
        up_to: Option<Plane>,
    },
    Revolve {
        plane: Plane,
        sketch: &'a Sketch,
        def: &'a RevolveFeature,
        angle: f64,
        axis: Option<Axis>,
    },
    Hole {
        plane: Plane,
        sketch: &'a Sketch,
        def: &'a HoleFeature,
        sizes: HoleSizes,
    },
    EdgeFlange {
        /// The original flange, whose place in the flat pattern the copies are moved from.
        original: FeatureId,
        def: &'a crate::feature::EdgeFlangeFeature,
        spec: peet_sheetmetal::EdgeFlangeSpec,
        /// Its outline sketch, which each copy is shaped to, moved with it.
        outline: Option<(Plane, &'a Sketch)>,
    },
    SheetCut {
        plane: Plane,
        sketch: &'a Sketch,
        face: Option<FaceRef>,
        /// How far past its own sheet the cut reaches (see [`crate::sheet::sheet_cut_reach`]).
        reach: Option<(f64, f64)>,
    },
    Form {
        plane: Plane,
        sketch: &'a Sketch,
        face: Option<FaceRef>,
        def: &'a FormFeature,
        height: f64,
    },
    /// A tab (a base flange added to a sheet metal body): its sketch, moved.
    Tab { plane: Plane, sketch: &'a Sketch },
    /// A break corner: what it takes from (or adds to) the sheet, moved.
    BreakCorner { original: FeatureId },
    /// A mitre flange: the same profile along the edges the motion takes the original's to.
    MiterFlange {
        original: FeatureId,
        plane: Plane,
        sketch: &'a Sketch,
        gap: f64,
        offsets: [f64; 2],
    },
}

/// A copied feature: its name (for messages), what it is, its place in the model, and the
/// motions of the pattern or mirror it is a copy of, if it is (none: it is copied as it
/// is). A copy of a pattern copies what the pattern copies, the original and each copy
/// moved on.
pub(crate) type SeedEntry<'a> = (String, Seed<'a>, usize, Vec<Motion>);

/// The number of a copy, for its faces' names.
fn copy_number(seed: usize, copy: usize) -> u32 {
    ((seed as u32) << 16) | (copy as u32 & 0xffff)
}

/// Builds the copies: every seed (named for messages) at every motion.
pub(crate) fn apply_copies(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    seeds: &[SeedEntry<'_>],
    motions: &[Motion],
    stamp: u64,
) -> Result<Applied, FeatureError> {
    if motions.is_empty() {
        return Err(FeatureError(
            "There are no copies to make: the count must be 2 or more.".to_owned(),
        ));
    }
    let mut bodies = bodies.to_vec();
    let mut made = 0;
    let mut skipped: Vec<String> = Vec::new();
    let mut warning = None;
    // In the order they are in the model, as SolidWorks copies them: a flange on a copied
    // flange is copied after it. Each keeps its number for its faces' names.
    let mut order: Vec<usize> = (0..seeds.len()).collect();
    order.sort_by_key(|&i| seeds[i].2);
    for si in order {
        let (name, seed, _, inner) = &seeds[si];
        let key = crate::hash::combine(stamp, si as u64);
        // A copy of a pattern or a mirror: each of its copies, moved on
        let composed: Vec<Motion>;
        let motions = if inner.is_empty() {
            motions
        } else {
            composed = motions
                .iter()
                .flat_map(|m| inner.iter().map(move |i| m.after(i)))
                .collect();
            &composed
        };
        let result = match seed {
            Seed::Extrude { .. } | Seed::Revolve { .. } | Seed::Hole { .. } => {
                let mut ok = 0;
                for (k, m) in motions.iter().enumerate() {
                    match solid_copy(feature, &bodies, seed, m, copy_number(si, k + 1), key) {
                        Ok(b) => {
                            bodies = b;
                            ok += 1;
                        }
                        Err(e) => skipped.push(format!("copy {} of {name} ({})", k + 2, e.0)),
                    }
                }
                made += ok;
                Ok(())
            }
            Seed::EdgeFlange { .. } => flange_copies(
                feature,
                model,
                &bodies,
                seed,
                motions,
                si,
                key,
                name,
                &mut skipped,
            )
            .map(|(b, n, w)| {
                bodies = b;
                made += n;
                if w.is_some() {
                    warning = w;
                }
            }),
            Seed::Tab { .. } | Seed::BreakCorner { .. } => added_copies(
                feature,
                model,
                &bodies,
                seed,
                motions,
                si,
                key,
                name,
                &mut skipped,
            )
            .map(|(b, n, w)| {
                bodies = b;
                made += n;
                if w.is_some() {
                    warning = w;
                }
            }),
            Seed::MiterFlange { .. } => miter_copies(
                feature,
                model,
                &bodies,
                seed,
                motions,
                si,
                key,
                name,
                &mut skipped,
            )
            .map(|(b, n, w)| {
                bodies = b;
                made += n;
                if w.is_some() {
                    warning = w;
                }
            }),
            Seed::SheetCut { .. } | Seed::Form { .. } => sheet_copies(
                feature,
                model,
                &bodies,
                seed,
                motions,
                si,
                key,
                name,
                &mut skipped,
            )
            .map(|(b, n, w)| {
                bodies = b;
                made += n;
                if w.is_some() {
                    warning = w;
                }
            }),
        };
        result.map_err(|e| FeatureError(format!("{name}: {}", e.0)))?;
    }
    if made == 0 {
        return Err(FeatureError(format!(
            "None of the copies could be made: {}.",
            skipped.join("; ")
        )));
    }
    if !skipped.is_empty() {
        warning = Some(format!(
            "Some copies were left out: {}.",
            skipped.join("; ")
        ));
    }
    Ok((bodies, warning))
}

/// One copy of an extrusion, a revolve or a hole feature: the feature built again on its
/// sketch plane moved (or reflected) by `motion`.
fn solid_copy(
    feature: FeatureId,
    bodies: &[Arc<Body>],
    seed: &Seed<'_>,
    motion: &Motion,
    copy: u32,
    stamp: u64,
) -> Result<Vec<Arc<Body>>, FeatureError> {
    let moved = |plane: &Plane| {
        motion
            .sketch_plane(plane)
            .ok_or_else(|| FeatureError("the copy's plane is degenerate".to_owned()))
    };
    let stamp = crate::hash::combine(stamp, u64::from(copy));
    let out = match seed {
        Seed::Extrude {
            plane,
            sketch,
            params,
            depth,
            up_to,
        } => apply_extrude(&ExtrudeInput {
            feature,
            bodies,
            plane: &moved(plane)?,
            sketch,
            params,
            depth: *depth,
            up_to: up_to.and_then(|p| motion.plane(&p)),
            stamp,
            mirror: motion.is_mirror(),
        })?,
        Seed::Revolve {
            plane,
            sketch,
            def,
            angle,
            axis,
        } => apply_revolve(&RevolveInput {
            feature,
            bodies,
            plane: &moved(plane)?,
            sketch,
            def,
            angle: *angle,
            // An axis outside the sketch moves with the copy.
            axis: axis.map(|a| Axis {
                origin: motion.point(a.origin),
                dir: motion.vector(a.dir),
            }),
            stamp,
            mirror: motion.is_mirror(),
        })?,
        Seed::Hole {
            plane,
            sketch,
            def,
            sizes,
        } => {
            // Every hole of every copy gets a number of its own for its faces' names.
            let (out, _) = apply_hole(&HoleInput {
                feature,
                bodies,
                plane: &moved(plane)?,
                sketch,
                def,
                sizes: *sizes,
                stamp,
                instance: copy.wrapping_mul(4096),
                mirror: motion.is_mirror(),
            })?;
            return Ok(out);
        }
        _ => unreachable!("copied in the flat pattern"),
    };
    // Name the copy's faces apart from the other copies'.
    let instance = FaceName::new(feature, FaceRole::Instance(copy));
    Ok(out
        .into_iter()
        .map(|b| {
            let fresh = b.face_names.iter().any(|n| needs_instance(n, feature));
            if !fresh {
                return b;
            }
            let mut body = (*b).clone();
            for n in &mut body.face_names {
                if needs_instance(n, feature) {
                    *n = FaceName::merged([&*n, &instance]);
                }
            }
            Arc::new(body)
        })
        .collect())
}

/// Whether a face was made by `feature` and not yet told which copy it is part of.
fn needs_instance(name: &FaceName, feature: FeatureId) -> bool {
    name.origins().iter().any(|o| o.feature == feature)
        && !name
            .origins()
            .iter()
            .any(|o| o.feature == feature && matches!(o.role, FaceRole::Instance(_)))
}

/// What [`sheet_copies`] made: the bodies, how many copies, and a warning.
type SheetCopies = (Vec<Arc<Body>>, usize, Option<String>);

/// Every copy of a sheet metal cut or a form, made in the flat pattern of the body the
/// original is on, and the body rebuilt once.
#[allow(clippy::too_many_arguments)]
fn sheet_copies(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    seed: &Seed<'_>,
    motions: &[Motion],
    si: usize,
    stamp: u64,
    name: &str,
    skipped: &mut Vec<String>,
) -> Result<SheetCopies, FeatureError> {
    let (plane, sketch, face) = match seed {
        Seed::SheetCut {
            plane,
            sketch,
            face,
            ..
        }
        | Seed::Form {
            plane,
            sketch,
            face,
            ..
        } => (plane, *sketch, face.as_ref()),
        _ => unreachable!(),
    };
    // A cut sketched on a plane or on the sheet's edge: each copy cuts from where the motion
    // takes its sketch, as the original does.
    if let Seed::SheetCut { reach, .. } = seed
        && let Some(body) = crate::sheet::off_flange(bodies, face)
    {
        return Ok(cut_copies_anywhere(
            feature, model, bodies, body, plane, sketch, *reach, motions, si, stamp, name, skipped,
        )?);
    }
    let on = sketch_face(model, bodies, face, "the copied feature")?;
    let mut layout = on.sheet.layout.clone();
    // A point inside the sketch's shape, to tell which flange a copy lands on
    let probe = find_regions(sketch).regions.iter().find_map(|r| {
        let (lo, hi) = r.outer.bounds();
        let c = (lo + hi) / 2.0;
        r.contains(c).then_some(c)
    });
    let mut made = 0;
    for (k, m) in motions.iter().enumerate() {
        let copy = copy_number(si, k + 1);
        // The face the copy is on: the original's, if the copy stays in its plane. A copy of
        // a sheet metal cut can go onto another flange's face instead, as SolidWorks' can (a
        // mirror of a cut that shapes a flange shapes the flange across from it).
        let n = plane.normal();
        let moved_n = m.vector(n);
        let moved_o = m.point(plane.origin());
        let off = (moved_o - plane.origin()).dot(n);
        let t = layout.settings.thickness;
        // A flange with a face in the copy's plane, and which face
        let in_plane = |p: &peet_sheetmetal::Piece| {
            let level = p.frame.to_local(moved_o).z;
            if !p.is_flange() || moved_n.cross(p.frame.z_axis()).length() > 1e-9 {
                None
            } else if level.abs() <= 1e-6 {
                Some(false)
            } else if (level - t).abs() <= 1e-6 {
                Some(true)
            } else {
                None
            }
        };
        // The one under the copy's shape, which may be another flange in the same plane than the
        // original's (a mirror of a hole in one lip of a channel lands on the other lip)
        let under = probe.map(|q| m.point(plane.from_plane_coords(q)));
        let holding = under.and_then(|q| {
            layout.pieces.iter().enumerate().find_map(|(i, p)| {
                let top = in_plane(p)?;
                p.covers(p.frame.to_local(q).truncate())
                    .then_some((i, p.frame, top))
            })
        });
        let target = if moved_n.cross(n).length() <= 1e-9 && off.abs() <= 1e-6 {
            let own = under
                .is_none_or(|q| layout.pieces[on.piece].covers(on.frame.to_local(q).truncate()));
            if own {
                Some((on.piece, on.frame, on.top))
            } else {
                holding.or(Some((on.piece, on.frame, on.top)))
            }
        } else if matches!(seed, Seed::SheetCut { .. }) {
            holding.or_else(|| {
                layout
                    .pieces
                    .iter()
                    .enumerate()
                    .find_map(|(i, p)| in_plane(p).map(|top| (i, p.frame, top)))
            })
        } else {
            None
        };
        let Some((piece, frame, top)) = target else {
            skipped.push(format!(
                "copy {} of {name} (it leaves the plane of the face it is sketched on)",
                k + 2
            ));
            continue;
        };
        let to_flat =
            |p: peet_math::DVec2| frame.to_local(m.point(plane.from_plane_coords(p))).truncate();
        match seed {
            Seed::SheetCut { reach, .. } => {
                let profile = find_regions(sketch);
                let regions: Vec<_> = default_regions(&profile)
                    .into_iter()
                    .map(|i| profile.regions[i].clone())
                    .collect();
                if regions.is_empty() {
                    return Err(FeatureError(
                        "its sketch has no closed region to cut.".to_owned(),
                    ));
                }
                let area = Area::from_regions(&regions, feature.0)
                    .mapped(&to_flat)
                    .retagged(|t| match t {
                        CurveTag::Sketch { owner, entity } => CurveTag::Copy {
                            owner,
                            copy,
                            entity,
                        },
                        other => other,
                    });
                // The other flanges with a face in the copy's plane, each through its own
                // place in the flat pattern, as the original cuts them: a mirror of a cut on
                // a flange cuts the mirrored flange across from it, in the same plane.
                let into = frame.z_axis() * if top { -1.0 } else { 1.0 };
                let world = |p: peet_math::DVec2| m.point(plane.from_plane_coords(p));
                let mut done = vec![false; layout.pieces.len()];
                done[piece] = true;
                // Whether the shape, placed in the flat pattern by `map`, is over the sheet
                let over = |map: &dyn Fn(peet_math::DVec2) -> peet_math::DVec2| {
                    regions.iter().any(|r| {
                        let (lo, hi) = r.outer.bounds();
                        let c = (lo + hi) / 2.0;
                        r.contains(c)
                            .then_some(c)
                            .into_iter()
                            .chain(r.outer.edges.iter().flat_map(|e| {
                                (1..8).map(|k| e.curve.point_at(k as f64 / 8.0))
                            }))
                            .any(|p| layout.pieces.iter().any(|pc| pc.covers(map(p))))
                    })
                };
                let probes = [peet_math::DVec2::ZERO, peet_math::DVec2::X, peet_math::DVec2::Y];
                let mut others = Vec::new();
                for (area, pieces) in
                    crate::sheet::cut_from_plane(&layout, &world, into, None, &regions, feature.0)
                {
                    let i = pieces[0];
                    if i == piece {
                        continue;
                    }
                    done[i] = true;
                    let other = layout.pieces[i].frame;
                    let through = |p: peet_math::DVec2| other.to_local(world(p)).truncate();
                    if probes.iter().all(|p| through(*p).distance(to_flat(*p)) < 1e-6)
                        || !over(&through)
                    {
                        continue;
                    }
                    others.push((area, pieces));
                }
                // A copy whose shape lies wholly off the sheet cuts nothing: say so, as
                // SolidWorks leaves such an instance out.
                if reach.is_none() && others.is_empty() && !over(&to_flat) {
                    skipped.push(format!("copy {} of {name} (it is off the sheet)", k + 2));
                    continue;
                }
                if piece == on.piece {
                    layout.add_cut(area);
                } else {
                    layout.add_cut_on(area, vec![piece]);
                }
                for (area, pieces) in others {
                    let area = area.retagged(|t| match t {
                        CurveTag::Sketch { owner, entity } => CurveTag::Copy {
                            owner,
                            copy,
                            entity,
                        },
                        other => other,
                    });
                    layout.add_cut_on(area, pieces);
                }
                // As far past its own sheet as the original goes
                if let Some(span) = *reach {
                    let beyond =
                        crate::sheet::cut_beyond(&layout, &done, &world, into, span, &regions, feature.0);
                    for (area, pieces) in beyond {
                        let area = area.retagged(|t| match t {
                            CurveTag::Sketch { owner, entity } => CurveTag::Copy {
                                owner,
                                copy,
                                entity,
                            },
                            CurveTag::Generated { owner, part, index } => CurveTag::Generated {
                                owner,
                                part: part ^ (copy << 16),
                                index,
                            },
                            other => other,
                        });
                        layout.add_cut_on(area, pieces);
                    }
                }
                made += 1;
            }
            Seed::Form { def, height, .. } => {
                let (shapes, _) = form_outlines(sketch, def.kind)?;
                let mut ok = true;
                let before = layout.clone();
                for (part, (_, shape)) in shapes.iter().enumerate() {
                    let shape = map_shape(shape, to_flat);
                    let open_side = match &shape {
                        peet_sheetmetal::FormShape::Polygon(p) if def.kind == FormKind::Louver => {
                            Some(def.open_side as usize % p.len())
                        }
                        _ => None,
                    };
                    let piece = flange_holding(&layout, &shape).unwrap_or(on.piece);
                    if let Err(e) = layout.add_form(Form {
                        owner: feature.0,
                        part: (copy << 8) | part as u32,
                        piece,
                        kind: def.kind,
                        shape,
                        height: *height,
                        up: on.top != def.flip,
                        open_side,
                    }) {
                        skipped.push(format!("copy {} of {name} ({e})", k + 2));
                        ok = false;
                        break;
                    }
                }
                if ok {
                    made += 1;
                } else {
                    layout = before;
                }
            }
            _ => unreachable!(),
        }
    }
    if made == 0 {
        return Ok((bodies.to_vec(), 0, None));
    }
    let (out, warning) = rebuild_sheet(model, bodies, on.body, layout, stamp)?;
    Ok((out, made, warning))
}

/// Every copy of a sheet metal cut sketched off a flange's top or bottom (on a plane, or on
/// the sheet's edge): each cuts from where the motion takes the sketch, going the way the
/// motion takes the original's way in. `body` is the body of the face the sketch is on, or
/// `None` for every sheet metal body.
#[allow(clippy::too_many_arguments)]
fn cut_copies_anywhere(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    body: Option<usize>,
    plane: &Plane,
    sketch: &Sketch,
    reach: Option<(f64, f64)>,
    motions: &[Motion],
    si: usize,
    stamp: u64,
    name: &str,
    skipped: &mut Vec<String>,
) -> Result<SheetCopies, FeatureError> {
    let profile = find_regions(sketch);
    let regions: Vec<_> = default_regions(&profile)
        .into_iter()
        .map(|i| profile.regions[i].clone())
        .collect();
    if regions.is_empty() {
        return Err(FeatureError("its sketch has no closed region to cut.".to_owned()));
    }
    let mut out = bodies.to_vec();
    let mut warning = None;
    let mut cut = vec![false; motions.len()];
    for (index, b) in bodies.iter().enumerate() {
        let Some(sheet) = &b.sheet else { continue };
        if body.is_some_and(|i| i != index) {
            continue;
        }
        let mut layout = sheet.layout.clone();
        let mut any = false;
        for (k, m) in motions.iter().enumerate() {
            let copy = copy_number(si, k + 1);
            let world = |p: peet_math::DVec2| m.point(plane.from_plane_coords(p));
            let into = m.vector(-plane.normal());
            let cuts =
                crate::sheet::cut_from_plane(&layout, &world, into, reach, &regions, feature.0);
            for (area, pieces) in cuts {
                let area = area.retagged(|t| match t {
                    CurveTag::Sketch { owner, entity } => CurveTag::Copy {
                        owner,
                        copy,
                        entity,
                    },
                    CurveTag::Generated { owner, part, index } => CurveTag::Generated {
                        owner,
                        part: part ^ (copy << 16),
                        index,
                    },
                    other => other,
                });
                layout.add_cut_on(area, pieces);
                cut[k] = true;
                any = true;
            }
        }
        if any {
            let (rebuilt, w) = rebuild_sheet(model, &out, index, layout, stamp)?;
            out = rebuilt;
            warning = warning.or(w);
        }
    }
    for (k, c) in cut.iter().enumerate() {
        if !c {
            skipped.push(format!("copy {} of {name} (nothing of the sheet is in its way)", k + 2));
        }
    }
    let made = cut.iter().filter(|c| **c).count();
    Ok((out, made, warning))
}

/// The flange whose material holds a form's centre.
fn flange_holding(
    layout: &peet_sheetmetal::Layout,
    shape: &peet_sheetmetal::FormShape,
) -> Option<usize> {
    let c = match shape {
        peet_sheetmetal::FormShape::Circle { center, .. } => *center,
        peet_sheetmetal::FormShape::Polygon(p) => {
            p.iter().copied().sum::<peet_math::DVec2>() / p.len().max(1) as f64
        }
    };
    layout.pieces.iter().position(|pc| {
        pc.is_flange() && pc.covers(c)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_sketch_plane_reflects_points() {
        let m = Motion::Mirror {
            point: DVec3::new(10.0, 0.0, 0.0),
            normal: DVec3::X,
        };
        let plane = Plane::TOP;
        let moved = m.sketch_plane(&plane).unwrap();
        // A sketch point (u, v), mirrored in the sketch to (u, −v), lands on the
        // reflection of where (u, v) was.
        for p in [DVec2::new(3.0, 4.0), DVec2::new(-2.0, 7.5)] {
            let original = plane.from_plane_coords(p);
            let copy = moved.from_plane_coords(DVec2::new(p.x, -p.y));
            assert!(
                copy.abs_diff_eq(m.point(original), 1e-12),
                "{copy} {original}"
            );
        }
        // The normal is reflected too.
        assert!(moved.normal().abs_diff_eq(m.vector(plane.normal()), 1e-12));
    }

    #[test]
    fn composed_motions_move_points_as_one_after_the_other() {
        let mx = Motion::Mirror {
            point: DVec3::new(100.0, 0.0, 0.0),
            normal: DVec3::X,
        };
        let my = Motion::Mirror {
            point: DVec3::new(0.0, 50.0, 0.0),
            normal: DVec3::Y,
        };
        let shift = Motion::translation(DVec3::new(0.0, 0.0, 7.0));
        let turn = Motion::rotation(DVec3::new(5.0, 0.0, 0.0), DVec3::Z, 0.3);
        let p = DVec3::new(20.0, 15.0, 3.0);
        for (outer, inner) in [(my, mx), (shift, mx), (mx, turn), (turn, shift), (mx, mx)] {
            let both = outer.after(&inner);
            let want = outer.point(inner.point(p));
            assert!(both.point(p).abs_diff_eq(want, 1e-9), "{both:?}");
            let v = DVec3::new(0.3, -0.7, 0.2);
            assert!(
                both.vector(v)
                    .abs_diff_eq(outer.vector(inner.vector(v)), 1e-12)
            );
            assert_eq!(both.is_mirror(), outer.is_mirror() != inner.is_mirror());
        }
        // Two mirrors in planes square to each other: a half turn about their line.
        let half = my.after(&mx);
        assert!(
            half.point(p)
                .abs_diff_eq(DVec3::new(180.0, 85.0, 3.0), 1e-9)
        );
        // A reflected motion's sketch plane still lands mirrored points where they go.
        let reflected = shift.after(&mx);
        let moved = reflected.sketch_plane(&Plane::TOP).unwrap();
        let q = DVec2::new(3.0, 4.0);
        assert!(
            moved
                .from_plane_coords(DVec2::new(q.x, -q.y))
                .abs_diff_eq(reflected.point(Plane::TOP.from_plane_coords(q)), 1e-9)
        );
    }

    #[test]
    fn rotation_about_an_offset_axis() {
        let m = Motion::rotation(DVec3::new(5.0, 0.0, 0.0), DVec3::Z, std::f64::consts::PI);
        assert!(
            m.point(DVec3::new(6.0, 0.0, 1.0))
                .abs_diff_eq(DVec3::new(4.0, 0.0, 1.0), 1e-12)
        );
    }
}

/// Every copy of an edge flange, added to the flat pattern of the body the original is on
/// and the body rebuilt once. Each copy goes on the edge the motion takes the original's
/// edge to: an edge of a flange of the body, with the flange's material on one side.
#[allow(clippy::too_many_arguments)]
fn flange_copies(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    seed: &Seed<'_>,
    motions: &[Motion],
    si: usize,
    stamp: u64,
    name: &str,
    skipped: &mut Vec<String>,
) -> Result<SheetCopies, FeatureError> {
    let Seed::EdgeFlange {
        original,
        def,
        spec,
        outline,
    } = seed
    else {
        unreachable!()
    };
    // The body the original is on, and the original's place in its flat pattern
    let found = bodies.iter().enumerate().find_map(|(i, b)| {
        let sheet = b.sheet.as_ref()?;
        let at = sheet
            .layout
            .attachments
            .iter()
            .find(|a| a.owner == original.0)?;
        Some((i, sheet, at.clone()))
    });
    let (index, sheet, at) = found.ok_or_else(|| {
        FeatureError("the flange it copies is not on a sheet metal body".to_owned())
    })?;
    let mut layout = sheet.layout.clone();
    // A flange that runs up to a point: its copies are as long as it came out
    let length = def
        .up_to
        .as_ref()
        .and_then(|_| {
            layout
                .attachments
                .iter()
                .position(|a| a.owner == original.0)
        })
        .and_then(|k| layout.edge_flange_length(k));
    let t = layout.settings.thickness;
    let parent = &layout.pieces[at.parent];
    // The original's bend: which way it goes, in the model
    let up = at
        .pieces
        .first()
        .and_then(|&b| layout.pieces[b].bend())
        .map(|b| b.up)
        .ok_or_else(|| FeatureError("the flange it copies has no bend".to_owned()))?;
    let bend_way = parent.frame.z_axis() * if up { 1.0 } else { -1.0 };
    // The edge's ends, through the middle of the sheet
    let ends = [at.a, at.b].map(|p| parent.frame.to_world(p.extend(t / 2.0)));
    let relief = crate::sheet::edge_flange_relief(def, &layout.settings, &model.parameters)
        .map_err(FeatureError)?;
    let own = peet_sheetmetal::FlangeOwn {
        relief,
        model: crate::sheet::edge_flange_bend(def, &model.parameters).map_err(FeatureError)?,
        set_out: crate::sheet::edge_flange_position_offset(def, &model.parameters)
            .map_err(FeatureError)?,
    };
    let mut made = 0;
    for (k, m) in motions.iter().enumerate() {
        let copy = copy_number(si, k + 1);
        let moved = ends.map(|p| m.point(p));
        let Some((piece, a, b, turned)) = edge_at(&layout, moved) else {
            skipped.push(format!(
                "copy {} of {name} (no edge of the sheet is where it would go)",
                k + 2
            ));
            continue;
        };
        let way = m.vector(bend_way);
        let top = (way.dot(layout.pieces[piece].frame.z_axis()) > 0.0) != spec.flip;
        let site = peet_sheetmetal::EdgeSite { piece, a, b, top };
        let mut copy_spec = *spec;
        if let Some(length) = length {
            copy_spec.length = length;
        }
        if turned {
            copy_spec.offsets = [spec.offsets[1], spec.offsets[0]];
        }
        let added = match outline {
            None => layout.add_edge_flange_part(feature.0, copy << 4, &site, &copy_spec, own),
            Some((plane, sketch)) => {
                let profile = find_regions(sketch);
                let regions: Vec<_> = default_regions(&profile)
                    .into_iter()
                    .map(|i| profile.regions[i].clone())
                    .collect();
                let shape = |frame: &Frame| {
                    let to_flat = |p: peet_math::DVec2| {
                        frame.to_local(m.point(plane.from_plane_coords(p))).truncate()
                    };
                    Area::from_regions(&regions, feature.0).mapped(&to_flat)
                };
                layout.add_edge_flange_outlined(feature.0, copy << 4, &site, &copy_spec, own, &shape)
            }
        };
        match added {
            Ok(()) => made += 1,
            Err(e) => skipped.push(format!("copy {} of {name} ({e})", k + 2)),
        }
    }
    if made == 0 {
        return Ok((bodies.to_vec(), 0, None));
    }
    let (out, warning) = rebuild_sheet(model, bodies, index, layout, stamp)?;
    Ok((out, made, warning))
}

/// The edge of a flange of `layout` between two points through the middle of the sheet
/// (model coordinates): the flange, the edge's ends in flat coordinates with the flange's
/// material on their left, and whether that turned the points round.
fn edge_at(
    layout: &peet_sheetmetal::Layout,
    ends: [DVec3; 2],
) -> Option<(usize, DVec2, DVec2, bool)> {
    let t = layout.settings.thickness;
    layout.pieces.iter().enumerate().find_map(|(i, p)| {
        if !p.is_flange() {
            return None;
        }
        let local = ends.map(|q| p.frame.to_local(q));
        if local.iter().any(|q| (q.z - t / 2.0).abs() > 1e-6) {
            return None;
        }
        let (a, b) = (local[0].truncate(), local[1].truncate());
        let d = (b - a).normalize_or_zero();
        let mid = (a + b) / 2.0;
        let left = p.covers(mid + d.perp() * 1e-3);
        let right = p.covers(mid - d.perp() * 1e-3);
        match (left, right) {
            (true, false) => Some((i, a, b, false)),
            (false, true) => Some((i, b, a, true)),
            _ => None,
        }
    })
}

/// Every copy of a tab or a break corner: sheet added to (or taken from) the flange the
/// motion takes the original's to, in the flat pattern, and each body changed rebuilt once.
#[allow(clippy::too_many_arguments)]
fn added_copies(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    seed: &Seed<'_>,
    motions: &[Motion],
    si: usize,
    stamp: u64,
    name: &str,
    skipped: &mut Vec<String>,
) -> Result<SheetCopies, FeatureError> {
    let mut layouts: Vec<Option<peet_sheetmetal::Layout>> = vec![None; bodies.len()];
    let layout_of = |layouts: &[Option<peet_sheetmetal::Layout>], i: usize| {
        layouts[i]
            .clone()
            .or_else(|| bodies[i].sheet.as_ref().map(|s| s.layout.clone()))
    };
    let mut made = 0;
    match seed {
        Seed::Tab { plane, sketch } => {
            let regions = crate::sheet::tab_regions(sketch)?;
            for (k, m) in motions.iter().enumerate() {
                let copy = copy_number(si, k + 1);
                let world = |p: DVec2| m.point(plane.from_plane_coords(p));
                let area = |frame: &Frame| {
                    Area::from_regions(&regions, feature.0)
                        .mapped(&|p| frame.to_local(world(p)).truncate())
                        .retagged(|t| match t {
                            CurveTag::Sketch { owner, entity } => CurveTag::Copy {
                                owner,
                                copy,
                                entity,
                            },
                            other => other,
                        })
                };
                let origin = m.point(plane.origin());
                let normal = m.vector(plane.normal());
                let found = (0..bodies.len()).find_map(|i| {
                    let layout = layout_of(&layouts, i)?;
                    let t = layout.settings.thickness;
                    let (piece, area) =
                        crate::sheet::flange_beside(&layout, origin, normal, &[0.0, t], &area)?;
                    Some((i, layout, piece, area))
                });
                let Some((i, mut layout, piece, area)) = found else {
                    skipped.push(format!(
                        "copy {} of {name} (no flat face of the sheet is where it would go)",
                        k + 2
                    ));
                    continue;
                };
                layout.add_sheet(piece, area);
                layouts[i] = Some(layout);
                made += 1;
            }
        }
        Seed::BreakCorner { original } => {
            // What the original took or added, on the body it is on
            let owned = |a: &Area| {
                a.edges()
                    .next()
                    .is_some_and(|e| e.tag.owner() == original.0)
            };
            let found = bodies.iter().enumerate().find_map(|(i, b)| {
                let layout = &b.sheet.as_ref()?.layout;
                let mut parts: Vec<(usize, Area, bool)> = Vec::new();
                for (pi, p) in layout.pieces.iter().enumerate() {
                    parts.extend(
                        p.adds
                            .iter()
                            .filter(|a| owned(a))
                            .map(|a| (pi, a.clone(), true)),
                    );
                }
                for c in &layout.cuts {
                    if owned(&c.area)
                        && let Some(&pi) = c.pieces.first()
                    {
                        parts.push((pi, c.area.clone(), false));
                    }
                }
                (!parts.is_empty()).then_some((i, parts))
            });
            let (index, parts) = found.ok_or_else(|| {
                FeatureError("the break corner it copies is not on a sheet metal body".to_owned())
            })?;
            let mut layout = layout_of(&layouts, index).expect("a sheet metal body");
            let t = layout.settings.thickness;
            for (k, m) in motions.iter().enumerate() {
                let copy = copy_number(si, k + 1);
                let mut trial = layout.clone();
                let mut ok = true;
                for (pi, area, add) in &parts {
                    let from = trial.pieces[*pi].frame;
                    let world = |q: DVec2| m.point(from.to_world(q.extend(t / 2.0)));
                    let moved = |frame: &Frame| {
                        area.mapped(&|q| frame.to_local(world(q)).truncate())
                            .retagged(|tag| match tag {
                                CurveTag::Generated { part, index, .. } => CurveTag::Generated {
                                    owner: feature.0,
                                    part: 0x8000_0000 | (copy << 8) | (part & 0xff),
                                    index,
                                },
                                other => other,
                            })
                    };
                    let origin = m.point(from.to_world(DVec3::new(0.0, 0.0, t / 2.0)));
                    let normal = m.vector(from.z_axis());
                    match crate::sheet::flange_beside(&trial, origin, normal, &[t / 2.0], &moved) {
                        Some((piece, a)) if *add => trial.add_sheet(piece, a),
                        Some((piece, a)) => trial.add_cut_on(a, vec![piece]),
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    layout = trial;
                    made += 1;
                } else {
                    skipped.push(format!(
                        "copy {} of {name} (no corner of the sheet is where it would go)",
                        k + 2
                    ));
                }
            }
            layouts[index] = Some(layout);
        }
        _ => unreachable!(),
    }
    if made == 0 {
        return Ok((bodies.to_vec(), 0, None));
    }
    let mut out = bodies.to_vec();
    let mut warning = None;
    for (i, layout) in layouts.into_iter().enumerate() {
        if let Some(layout) = layout {
            let (b, w) = rebuild_sheet(model, &out, i, layout, stamp)?;
            out = b;
            warning = warning.or(w);
        }
    }
    Ok((out, made, warning))
}

/// Every copy of a mitre flange: the same profile along the edges the motion takes the
/// original's to (in the other order for a mirror, which turns them round), added to the
/// flat pattern of the body the original is on and the body rebuilt once.
#[allow(clippy::too_many_arguments)]
fn miter_copies(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    seed: &Seed<'_>,
    motions: &[Motion],
    si: usize,
    stamp: u64,
    name: &str,
    skipped: &mut Vec<String>,
) -> Result<SheetCopies, FeatureError> {
    let Seed::MiterFlange {
        original,
        plane,
        sketch,
        gap,
        offsets,
    } = seed
    else {
        unreachable!()
    };
    // The body the original is on, and its edges in the flat pattern, in order
    let found = bodies.iter().enumerate().find_map(|(i, b)| {
        let sheet = b.sheet.as_ref()?;
        let ats: Vec<_> = sheet
            .layout
            .attachments
            .iter()
            .filter(|a| a.owner == original.0)
            .cloned()
            .collect();
        (!ats.is_empty()).then_some((i, sheet, ats))
    });
    let (index, sheet, ats) = found.ok_or_else(|| {
        FeatureError("the mitre flange it copies is not on a sheet metal body".to_owned())
    })?;
    let mut layout = sheet.layout.clone();
    let t = layout.settings.thickness;
    let parent = layout.pieces[ats[0].parent].frame;
    let sites: Vec<peet_sheetmetal::EdgeSite> = ats
        .iter()
        .map(|a| peet_sheetmetal::EdgeSite {
            piece: a.parent,
            a: a.a,
            b: a.b,
            top: false,
        })
        .collect();
    let profile = crate::sheet::miter_profile(&sites, &parent, plane, sketch, t)?;
    let mut made = 0;
    for (k, m) in motions.iter().enumerate() {
        let copy = copy_number(si, k + 1);
        let mut moved = Vec::new();
        let mut turned = false;
        for at in &ats {
            let frame = layout.pieces[at.parent].frame;
            let ends = [at.a, at.b].map(|p| m.point(frame.to_world(p.extend(t / 2.0))));
            match edge_at(&layout, ends) {
                Some((piece, a, b, turn)) => {
                    turned = turn;
                    moved.push(peet_sheetmetal::EdgeSite {
                        piece,
                        a,
                        b,
                        top: false,
                    });
                }
                None => break,
            }
        }
        if moved.len() != ats.len() {
            skipped.push(format!(
                "copy {} of {name} (no edge of the sheet is where it would go)",
                k + 2
            ));
            continue;
        }
        // The profile's heights from the side of the sheet the original's are from
        let flip = m
            .vector(parent.z_axis())
            .dot(layout.pieces[moved[0].piece].frame.z_axis())
            < 0.0;
        let points: Vec<DVec2> = profile
            .iter()
            .map(|p| if flip { DVec2::new(p.x, t - p.y) } else { *p })
            .collect();
        let offs = if turned {
            [offsets[1], offsets[0]]
        } else {
            *offsets
        };
        let mut trial = layout.clone();
        let added = crate::sheet::chain_sites(moved).and_then(|sites| {
            trial.add_miter_flange_part(feature.0, copy << 12, &sites, &points, offs, *gap)
        });
        match added {
            Ok(()) => {
                layout = trial;
                made += 1;
            }
            Err(e) => skipped.push(format!("copy {} of {name} ({e})", k + 2)),
        }
    }
    if made == 0 {
        return Ok((bodies.to_vec(), 0, None));
    }
    let (out, warning) = rebuild_sheet(model, bodies, index, layout, stamp)?;
    Ok((out, made, warning))
}
