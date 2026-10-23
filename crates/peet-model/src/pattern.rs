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

use peet_math::{DQuat, DVec3, Frame, Plane};
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
}

impl Motion {
    pub fn point(&self, p: DVec3) -> DVec3 {
        match *self {
            Self::Rigid(f) => f.to_world(p),
            Self::Mirror { point, normal } => p - normal * (2.0 * (p - point).dot(normal)),
        }
    }

    pub fn vector(&self, v: DVec3) -> DVec3 {
        match *self {
            Self::Rigid(f) => f.vector_to_world(v),
            Self::Mirror { normal, .. } => v - normal * (2.0 * v.dot(normal)),
        }
    }

    pub fn is_mirror(&self) -> bool {
        matches!(self, Self::Mirror { .. })
    }

    /// Where a sketch plane goes. A reflection can't be a frame, so for a mirror the
    /// copy's sketch is mirrored too (its y negated): the plane's y axis is turned round
    /// to make up for it, which keeps the normal where the reflection puts it.
    pub fn sketch_plane(&self, plane: &Plane) -> Option<Plane> {
        match *self {
            Self::Rigid(f) => Some(Plane {
                frame: f.compose(&plane.frame),
            }),
            Self::Mirror { .. } => {
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

/// A feature to copy, with its references resolved.
pub(crate) enum Seed<'a> {
    Extrude {
        plane: Plane,
        sketch: &'a Sketch,
        params: &'a Extrude,
        depth: f64,
        draft: f64,
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
}

/// The number of a copy, for its faces' names.
fn copy_number(seed: usize, copy: usize) -> u32 {
    ((seed as u32) << 16) | (copy as u32 & 0xffff)
}

/// Builds the copies: every seed (named for messages) at every motion.
pub(crate) fn apply_copies(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    seeds: &[(String, Seed<'_>, usize)],
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
        let (name, seed, _) = &seeds[si];
        let key = crate::hash::combine(stamp, si as u64);
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
            draft,
            up_to,
        } => apply_extrude(&ExtrudeInput {
            feature,
            bodies,
            plane: &moved(plane)?,
            sketch,
            params,
            depth: *depth,
            draft: *draft,
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
        Seed::SheetCut { .. } | Seed::Form { .. } | Seed::EdgeFlange { .. } => {
            unreachable!("copied in the flat pattern")
        }
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
        Seed::Extrude { .. } | Seed::Revolve { .. } | Seed::Hole { .. } | Seed::EdgeFlange { .. } => {
            unreachable!()
        }
    };
    let on = sketch_face(model, bodies, face, "the copied feature")?;
    let mut layout = on.sheet.layout.clone();
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
        let target = if moved_n.cross(n).length() <= 1e-9 && off.abs() <= 1e-6 {
            Some((on.piece, on.frame, on.top))
        } else if matches!(seed, Seed::SheetCut { .. }) {
            layout.pieces.iter().enumerate().find_map(|(i, p)| {
                let z = p.frame.z_axis();
                let level = p.frame.to_local(moved_o).z;
                (p.is_flange() && moved_n.cross(z).length() <= 1e-9)
                    .then_some(())
                    .and_then(|_| {
                        if level.abs() <= 1e-6 {
                            Some((i, p.frame, false))
                        } else if (level - t).abs() <= 1e-6 {
                            Some((i, p.frame, true))
                        } else {
                            None
                        }
                    })
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
                if piece == on.piece {
                    layout.add_cut(area);
                } else {
                    layout.add_cut_on(area, vec![piece]);
                }
                // As far past its own sheet as the original goes
                if let Some(span) = *reach {
                    let mut done = vec![false; layout.pieces.len()];
                    done[piece] = true;
                    // Into the sheet from the copy's face, which is the original's moved
                    let into = frame.z_axis() * if top { -1.0 } else { 1.0 };
                    let world = |p: peet_math::DVec2| m.point(plane.from_plane_coords(p));
                    let beyond =
                        crate::sheet::cut_beyond(&layout, &done, &world, into, span, &regions, feature.0);
                    for (area, pieces) in beyond {
                        let area = area.retagged(|t| match t {
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
            Seed::Extrude { .. } | Seed::Revolve { .. } | Seed::Hole { .. } | Seed::EdgeFlange { .. } => {
                unreachable!()
            }
        }
    }
    if made == 0 {
        return Ok((bodies.to_vec(), 0, None));
    }
    let (out, warning) = rebuild_sheet(model, bodies, on.body, layout, stamp)?;
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
    use peet_math::DVec2;

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
        // The flange whose middle the moved edge lies in, along its boundary
        let target = layout.pieces.iter().enumerate().find_map(|(i, p)| {
            if !p.is_flange() {
                return None;
            }
            let local = moved.map(|q| p.frame.to_local(q));
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
        });
        let Some((piece, a, b, turned)) = target else {
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
