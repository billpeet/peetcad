//! Silhouette lines of cylindrical faces.
//!
//! A cylinder's silhouette is made of generator lines (parallel to the axis) at the angles
//! where the surface normal is perpendicular to the view direction: two angles for a given
//! view, the same along the whole axis (also in perspective, where the condition
//! `normal · (eye − point) = 0` reduces to `ρ(θ) · (eye − axis origin) = r`).
//! Each generator is clipped to the face by intersecting the vertical line `θ = const`
//! with the face's loops in `(r θ, v)` parameter space (even–odd rule).

use std::f64::consts::TAU;

use peet_math::{DVec2, DVec3, tolerance};

use super::{View, face_boundary, sample_edges};
use crate::Solid;
use crate::geom::{Cylinder, Surface};

/// Chord tolerance (mm) for the boundary polygons used for clipping. Lines and circles are
/// straight in a cylinder's parameter space, so this only matters for oblique (elliptical)
/// boundaries.
const BOUNDARY_TOLERANCE: f64 = 0.01;

/// Angular slack (radians, and turns) for generators that fall on a face's boundary.
const ANGLE_SLACK: f64 = 1e-9;

/// One cylindrical face, prepared for silhouette queries.
#[derive(Clone, Debug)]
struct CylinderFace {
    cylinder: Cylinder,
    /// Boundary segments in `(θ, v)` (true angle, not mirrored), all loops together.
    segments: Vec<[DVec2; 2]>,
    theta_min: f64,
    theta_max: f64,
}

/// Precomputed data for silhouette extraction: build once per solid, query per view.
#[derive(Clone, Debug, Default)]
pub struct Silhouettes {
    faces: Vec<CylinderFace>,
}

impl Silhouettes {
    pub fn new(solid: &Solid) -> Self {
        let mut faces = Vec::new();
        if !solid
            .faces
            .iter()
            .any(|f| matches!(f.surface, Surface::Cylinder(_)))
        {
            return Self { faces };
        }
        let samples = sample_edges(solid, BOUNDARY_TOLERANCE);
        for id in solid.face_ids() {
            let Surface::Cylinder(cylinder) = solid.face(id).surface else {
                continue;
            };
            let b = face_boundary(solid, id, &samples);
            let mirror = if b.mirrored { -1.0 } else { 1.0 };
            let to_angle = |p: DVec2| DVec2::new(p.x * mirror / cylinder.radius, p.y);
            let mut segments = Vec::with_capacity(b.param.len());
            let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
            for r in &b.loops {
                let pts = &b.param[r.clone()];
                for i in 0..pts.len() {
                    let (p, q) = (to_angle(pts[i]), to_angle(pts[(i + 1) % pts.len()]));
                    lo = lo.min(p.x);
                    hi = hi.max(p.x);
                    segments.push([p, q]);
                }
            }
            if !segments.is_empty() {
                faces.push(CylinderFace {
                    cylinder,
                    segments,
                    theta_min: lo,
                    theta_max: hi,
                });
            }
        }
        Self { faces }
    }

    /// The silhouette segments for `view`, in model space.
    pub fn lines(&self, view: View) -> Vec<[DVec3; 2]> {
        let mut out = Vec::new();
        let mut crossings: Vec<f64> = Vec::new();
        for f in &self.faces {
            let Some(angles) = silhouette_angles(&f.cylinder, view) else {
                continue;
            };
            for base in angles {
                // Every representative of the angle inside the face's (unwrapped) range
                // `[min, max)`: half open, so a generator on a full cylinder's seam is found
                // once, not at both ends of the range.
                let mut theta = base + ((f.theta_min - base) / TAU - ANGLE_SLACK).ceil() * TAU;
                while theta < f.theta_max - ANGLE_SLACK {
                    let at = theta.max(f.theta_min);
                    crossings.clear();
                    for [p, q] in &f.segments {
                        if (p.x <= at) != (q.x <= at) {
                            let s = (at - p.x) / (q.x - p.x);
                            crossings.push(p.y + (q.y - p.y) * s);
                        }
                    }
                    crossings.sort_by(f64::total_cmp);
                    let surface = Surface::Cylinder(f.cylinder);
                    for pair in crossings.as_chunks::<2>().0 {
                        if pair[1] - pair[0] > tolerance::LINEAR {
                            out.push([
                                surface.point(DVec2::new(theta, pair[0])),
                                surface.point(DVec2::new(theta, pair[1])),
                            ]);
                        }
                    }
                    theta += TAU;
                }
            }
        }
        out
    }
}

/// The two angles (in the cylinder's frame) of its silhouette generators, or `None` when
/// there is no silhouette (looking along the axis, or from inside the cylinder).
fn silhouette_angles(c: &Cylinder, view: View) -> Option<[f64; 2]> {
    match view {
        View::Orthographic { dir } => {
            let d = c.frame.vector_to_local(dir);
            let across = DVec2::new(d.x, d.y);
            if across.length() <= tolerance::ANGULAR * dir.length().max(f64::MIN_POSITIVE)
                || !across.is_finite()
            {
                return None;
            }
            // Normals perpendicular to the view direction.
            let theta = (-d.x).atan2(d.y);
            Some([theta, theta + std::f64::consts::PI])
        }
        View::Perspective { eye } => {
            let e = c.frame.to_local(eye);
            let dist = DVec2::new(e.x, e.y).length();
            if !dist.is_finite() || dist <= c.radius + tolerance::LINEAR {
                return None;
            }
            let phi = e.y.atan2(e.x);
            let half = (c.radius / dist).acos();
            Some([phi - half, phi + half])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extrude::extrude;
    use crate::tessellate::silhouettes;
    use peet_math::Plane;
    use peet_sketch::Sketch;
    use peet_sketch::region::{Region, find_regions};
    use peet_sketch::shapes;

    fn cylinder() -> Solid {
        let mut s = Sketch::new();
        s.add_circle(DVec2::ZERO, 5.0);
        extrude(&Plane::TOP, &find_regions(&s).regions, 0.0, 10.0).unwrap()
    }

    fn sorted(mut lines: Vec<[DVec3; 2]>) -> Vec<[DVec3; 2]> {
        for l in &mut lines {
            if l[0].z > l[1].z {
                l.swap(0, 1);
            }
        }
        lines.sort_by(|a, b| a[0].x.total_cmp(&b[0].x));
        lines
    }

    #[test]
    fn cylinder_from_the_side() {
        let solid = cylinder();
        let lines = sorted(silhouettes(&solid, View::Orthographic { dir: DVec3::Y }));
        assert_eq!(lines.len(), 2);
        assert!(lines[0][0].abs_diff_eq(DVec3::new(-5.0, 0.0, 0.0), 1e-9));
        assert!(lines[0][1].abs_diff_eq(DVec3::new(-5.0, 0.0, 10.0), 1e-9));
        assert!(lines[1][0].abs_diff_eq(DVec3::new(5.0, 0.0, 0.0), 1e-9));
        assert!(lines[1][1].abs_diff_eq(DVec3::new(5.0, 0.0, 10.0), 1e-9));
        // Any direction across the axis gives two generators; along the axis none.
        for k in 0..16 {
            let a = f64::from(k) * 0.4;
            let dir = DVec3::new(a.cos(), a.sin(), 0.3);
            let lines = silhouettes(&solid, View::Orthographic { dir });
            assert_eq!(lines.len(), 2, "direction {dir}");
            for l in lines {
                let n = DVec3::new(l[0].x, l[0].y, 0.0).normalize();
                assert!(n.dot(dir).abs() < 1e-9);
                assert!((l[0].z - l[1].z).abs() > 10.0 - 1e-9);
            }
        }
        assert!(silhouettes(&solid, View::Orthographic { dir: DVec3::Z }).is_empty());
    }

    #[test]
    fn cylinder_in_perspective() {
        let solid = cylinder();
        let eye = DVec3::new(0.0, -10.0, 5.0);
        let lines = sorted(silhouettes(&solid, View::Perspective { eye }));
        assert_eq!(lines.len(), 2);
        for l in &lines {
            for p in l {
                let n = DVec3::new(p.x, p.y, 0.0).normalize();
                assert!(n.dot(eye - *p).abs() < 1e-9, "tangent sight line");
            }
            // Tangent points from a point 10 away from the axis of a radius 5 cylinder.
            assert!((l[0].y + 2.5).abs() < 1e-9);
        }
        // From inside the cylinder there is no silhouette.
        let inside = View::Perspective {
            eye: DVec3::new(1.0, 1.0, 5.0),
        };
        assert!(silhouettes(&solid, inside).is_empty());
    }

    #[test]
    fn generator_through_a_hole_and_on_the_seam() {
        // Seam at angle π; a pocket around angle 0 between z = 3 and z = 7.
        let solid = crate::validate::test_solids::pocketed_cylinder();
        let lines = sorted(silhouettes(&solid, View::Orthographic { dir: DVec3::Y }));
        let spans: Vec<(f64, f64, f64)> = lines.iter().map(|l| (l[0].x, l[0].z, l[1].z)).collect();
        assert_eq!(spans.len(), 3, "{spans:?}");
        let close = |a: (f64, f64, f64), b: (f64, f64, f64)| {
            (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9 && (a.2 - b.2).abs() < 1e-9
        };
        assert!(close(spans[0], (-5.0, 0.0, 10.0)), "{spans:?}");
        let mut right = [spans[1], spans[2]];
        right.sort_by(|a, b| a.1.total_cmp(&b.1));
        assert!(close(right[0], (5.0, 0.0, 3.0)), "{spans:?}");
        assert!(close(right[1], (5.0, 7.0, 10.0)), "{spans:?}");
    }

    #[test]
    fn partial_cylinders_are_clipped() {
        // A slot: two half cylinders. Looking along the slot's axis direction (+X), the
        // silhouette generators are at the top and bottom of the arcs (y = ±2), where the
        // arcs meet the flat sides: on the boundary of both half cylinders.
        let mut s = Sketch::new();
        shapes::slot(&mut s, DVec2::ZERO, DVec2::new(10.0, 0.0), 2.0);
        let regions: Vec<Region> = find_regions(&s).regions;
        let solid = extrude(&Plane::TOP, &regions, 0.0, 4.0).unwrap();
        // Looking along +Y: generators at x = -2 (left arc) and x = 12 (right arc) only.
        let lines = sorted(silhouettes(&solid, View::Orthographic { dir: DVec3::Y }));
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0][0].abs_diff_eq(DVec3::new(-2.0, 0.0, 0.0), 1e-9));
        assert!(lines[1][1].abs_diff_eq(DVec3::new(12.0, 0.0, 4.0), 1e-9));
        // A hole's wall has silhouettes too.
        let mut s = Sketch::new();
        shapes::rectangle(&mut s, DVec2::ZERO, DVec2::new(40.0, 20.0));
        s.add_circle(DVec2::new(10.0, 10.0), 3.0);
        let plate: Vec<Region> = find_regions(&s)
            .regions
            .into_iter()
            .filter(|r| r.holes.len() == 1)
            .collect();
        let solid = extrude(&Plane::TOP, &plate, 0.0, 2.0).unwrap();
        let lines = sorted(silhouettes(&solid, View::Orthographic { dir: DVec3::Y }));
        assert_eq!(lines.len(), 2);
        assert!((lines[0][0].x - 7.0).abs() < 1e-9 && (lines[1][0].x - 13.0).abs() < 1e-9);
    }
}
