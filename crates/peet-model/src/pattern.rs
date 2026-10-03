//! Patterns and mirrors: copies of features, moved or reflected.
//!
//! A pattern or a mirror copies *features*, not geometry: each copy is the copied
//! feature built again from a moved (or reflected) sketch, so a copy of a cut is a cut,
//! and a copy of a sheet metal cut or a form is made in the flat pattern like the
//! original. The features that can be copied are extrusions (bosses and cuts), sheet
//! metal cuts and forms.
//!
//! **Names.** Each copy is built as the pattern feature itself, and every face a copy
//! makes also carries [`FaceRole::Instance`] with the copy's number, so the faces of
//! different copies have different names and can be referred to like any other.
//!
//! **Sheet metal copies** stay on the face their original is sketched on: the copy's
//! sketch must land in the same plane (a pattern along the face, or a mirror square to
//! it). A copy that can't be made (a cut that misses the part, a form that runs off the
//! face) is left out with a warning, as long as one copy can be made.

use std::sync::Arc;

use peet_math::{DQuat, DVec3, Frame, Plane};
use peet_sheetmetal::{Area, CurveTag, Form, FormKind};
use peet_sketch::Sketch;
use peet_sketch::region::find_regions;

use crate::extrude::{Extrude, ExtrudeInput, apply_extrude, default_regions};
use crate::feature::{FeatureId, FormFeature};
use crate::naming::{Body, FaceName, FaceRef, FaceRole};
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
        up_to: Option<Plane>,
    },
    SheetCut {
        plane: Plane,
        sketch: &'a Sketch,
        face: Option<FaceRef>,
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
    seeds: &[(String, Seed<'_>)],
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
    for (si, (name, seed)) in seeds.iter().enumerate() {
        let key = crate::hash::combine(stamp, si as u64);
        let result = match seed {
            Seed::Extrude { .. } => {
                let mut ok = 0;
                for (k, m) in motions.iter().enumerate() {
                    match extrude_copy(feature, &bodies, seed, m, copy_number(si, k + 1), key) {
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

/// One copy of an extrusion.
fn extrude_copy(
    feature: FeatureId,
    bodies: &[Arc<Body>],
    seed: &Seed<'_>,
    motion: &Motion,
    copy: u32,
    stamp: u64,
) -> Result<Vec<Arc<Body>>, FeatureError> {
    let Seed::Extrude {
        plane,
        sketch,
        params,
        depth,
        up_to,
    } = seed
    else {
        unreachable!()
    };
    let moved = motion
        .sketch_plane(plane)
        .ok_or_else(|| FeatureError("the copy's plane is degenerate".to_owned()))?;
    let up_to = up_to.and_then(|p| motion.plane(&p));
    let out = apply_extrude(&ExtrudeInput {
        feature,
        bodies,
        plane: &moved,
        sketch,
        params,
        depth: *depth,
        up_to,
        stamp: crate::hash::combine(stamp, u64::from(copy)),
        mirror: motion.is_mirror(),
    })?;
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
        }
        | Seed::Form {
            plane,
            sketch,
            face,
            ..
        } => (plane, *sketch, face.as_ref()),
        Seed::Extrude { .. } => unreachable!(),
    };
    let on = sketch_face(model, bodies, face, "the copied feature")?;
    let mut layout = on.sheet.layout.clone();
    let mut made = 0;
    for (k, m) in motions.iter().enumerate() {
        let copy = copy_number(si, k + 1);
        // The copy must stay in the plane of the face.
        let n = plane.normal();
        let moved_n = m.vector(n);
        let off = (m.point(plane.origin()) - plane.origin()).dot(n);
        if moved_n.cross(n).length() > 1e-9 || off.abs() > 1e-6 {
            skipped.push(format!(
                "copy {} of {name} (it leaves the plane of the face it is sketched on)",
                k + 2
            ));
            continue;
        }
        let to_flat = |p: peet_math::DVec2| {
            on.frame
                .to_local(m.point(plane.from_plane_coords(p)))
                .truncate()
        };
        match seed {
            Seed::SheetCut { .. } => {
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
                layout.add_cut(area);
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
            Seed::Extrude { .. } => unreachable!(),
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
        pc.is_flange() && pc.outline.contains(c) && !pc.trims.iter().any(|t| t.contains(c))
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
