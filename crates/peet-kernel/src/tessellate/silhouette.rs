//! Silhouette lines of curved faces.
//!
//! A cylinder's silhouette is made of generator lines (parallel to the axis) at the angles
//! where the surface normal is perpendicular to the view direction: two angles for a given
//! view, the same along the whole axis (also in perspective, where the condition
//! `normal · (eye − point) = 0` reduces to `ρ(θ) · (eye − axis origin) = r`).
//! A cone's is made of rulings through its apex, found the same way. Each generator is
//! clipped to the face by intersecting the vertical line `θ = const` with the face's loops
//! in `(θ, v)` parameter space (even–odd rule).
//!
//! A sphere's or a torus's silhouette is a curve across the surface. It is traced where
//! `normal · view` changes sign over a grid of the face's parameters (marching squares),
//! and the pieces inside the face's loops are kept.

use std::f64::consts::TAU;

use peet_math::{DVec2, DVec3, tolerance};

use super::{View, face_boundary, sample_edges};
use crate::Solid;
use crate::geom::{Cone, Cylinder, Surface};

/// Chord tolerance (mm) for the boundary polygons used for clipping. Lines and circles are
/// straight in a cylinder's parameter space, so this only matters for oblique (elliptical)
/// boundaries.
const BOUNDARY_TOLERANCE: f64 = 0.01;

/// Angular slack (radians, and turns) for generators that fall on a face's boundary.
const ANGLE_SLACK: f64 = 1e-9;

/// How close (radians) to the ends of a face's angular range a traced silhouette counts
/// as being on the seam.
const SEAM_SLACK: f64 = 1e-6;

/// Grid cells per full turn when tracing a sphere's or a torus's silhouette.
const CELLS_PER_TURN: f64 = 64.0;

/// One curved face, prepared for silhouette queries.
#[derive(Clone, Debug)]
struct CurvedFace {
    surface: Surface,
    /// Boundary segments in `(θ, v)` (true parameters, not mirrored or scaled), all loops
    /// together.
    segments: Vec<[DVec2; 2]>,
    lo: DVec2,
    hi: DVec2,
}

/// Precomputed data for silhouette extraction: build once per solid, query per view.
#[derive(Clone, Debug, Default)]
pub struct Silhouettes {
    faces: Vec<CurvedFace>,
}

impl Silhouettes {
    pub fn new(solid: &Solid) -> Self {
        let mut faces = Vec::new();
        if solid
            .faces
            .iter()
            .all(|f| matches!(f.surface, Surface::Plane(_)))
        {
            return Self { faces };
        }
        let samples = sample_edges(solid, BOUNDARY_TOLERANCE);
        for id in solid.face_ids() {
            let surface = solid.face(id).surface.clone();
            if matches!(surface, Surface::Plane(_)) {
                continue;
            }
            let b = face_boundary(solid, id, &samples, BOUNDARY_TOLERANCE);
            let mirror = if b.mirrored { -1.0 } else { 1.0 };
            let to_param = |p: DVec2| DVec2::new(p.x * mirror / b.scale.x, p.y / b.scale.y);
            let mut segments = Vec::with_capacity(b.param.len());
            let mut lo = DVec2::splat(f64::INFINITY);
            let mut hi = DVec2::splat(f64::NEG_INFINITY);
            for r in &b.loops {
                let pts = &b.param[r.clone()];
                for i in 0..pts.len() {
                    let (p, q) = (to_param(pts[i]), to_param(pts[(i + 1) % pts.len()]));
                    lo = lo.min(p);
                    hi = hi.max(p);
                    segments.push([p, q]);
                }
            }
            if !segments.is_empty() {
                faces.push(CurvedFace {
                    surface,
                    segments,
                    lo,
                    hi,
                });
            }
        }
        Self { faces }
    }

    /// The silhouette segments for `view`, in model space.
    pub fn lines(&self, view: View) -> Vec<[DVec3; 2]> {
        let mut out = Vec::new();
        for f in &self.faces {
            match &f.surface {
                Surface::Cylinder(c) => {
                    if let Some(angles) = silhouette_angles(c, view) {
                        f.generators(&angles, &mut out);
                    }
                }
                Surface::Cone(c) => f.generators(&cone_angles(c, view), &mut out),
                Surface::Sphere(_) | Surface::Torus(_) | Surface::Nurbs(_) => {
                    f.contour(view, &mut out);
                }
                Surface::Plane(_) => {}
            }
        }
        out
    }
}

impl CurvedFace {
    /// The `v` intervals of the generator at `theta` that are inside the face.
    fn crossings(&self, theta: f64) -> Vec<f64> {
        let mut crossings = Vec::new();
        for [p, q] in &self.segments {
            if (p.x <= theta) != (q.x <= theta) {
                let s = (theta - p.x) / (q.x - p.x);
                crossings.push(p.y + (q.y - p.y) * s);
            }
        }
        crossings.sort_by(f64::total_cmp);
        crossings
    }

    /// The generators (lines of constant angle) at `angles`, clipped to the face.
    fn generators(&self, angles: &[f64], out: &mut Vec<[DVec3; 2]>) {
        for &base in angles {
            // Every representative of the angle inside the face's (unwrapped) range
            // `[min, max)`: half open, so a generator on a full turn's seam is found
            // once, not at both ends of the range.
            let mut theta = base + ((self.lo.x - base) / TAU - ANGLE_SLACK).ceil() * TAU;
            while theta < self.hi.x - ANGLE_SLACK {
                let at = theta.max(self.lo.x);
                for pair in self.crossings(at).as_chunks::<2>().0 {
                    let (a, b) = (
                        self.surface.point(DVec2::new(theta, pair[0])),
                        self.surface.point(DVec2::new(theta, pair[1])),
                    );
                    if a.distance(b) > tolerance::LINEAR {
                        out.push([a, b]);
                    }
                }
                theta += TAU;
            }
        }
    }

    /// Whether the parameter point `q` is inside the face's loops.
    fn contains(&self, q: DVec2) -> bool {
        self.crossings(q.x).iter().filter(|&&v| v > q.y).count() % 2 == 1
    }

    /// The silhouette across a doubly curved face: where the normal turns away from the
    /// viewer, traced over a grid of the face's parameters.
    fn contour(&self, view: View, out: &mut Vec<[DVec3; 2]>) {
        let size = self.hi - self.lo;
        if !(size.x > 0.0 && size.y > 0.0) {
            return;
        }
        let step = TAU / CELLS_PER_TURN;
        let nu = ((size.x / step).ceil() as usize).clamp(2, 512);
        let nv = ((size.y / step).ceil() as usize).clamp(2, 512);
        // One more column of cells before the face's range (and one more row where the
        // surface also goes round in `v`): a silhouette that runs along the face's seam
        // (a ball seen from the front, a ring seen along its axis) has its sign change
        // right at the edge of the range, on one side of it or the other.
        let extra_v = usize::from(self.surface.is_periodic_v());
        let at = |i: usize, j: usize| {
            self.lo
                + size
                    * DVec2::new(
                        (i as f64 - 1.0) / nu as f64,
                        (j as f64 - extra_v as f64) / nv as f64,
                    )
        };
        let (nu, nv) = (nu + 1, nv + extra_v);
        // How squarely the surface faces the viewer at each grid point.
        let facing = |uv: DVec2| {
            let n = self.surface.normal(uv);
            match view {
                View::Orthographic { dir } => n.dot(dir),
                View::Perspective { eye } => n.dot(self.surface.point(uv) - eye),
            }
        };
        let values: Vec<f64> = (0..=nu)
            .flat_map(|i| (0..=nv).map(move |j| (i, j)))
            .map(|(i, j)| facing(at(i, j)))
            .collect();
        let value = |i: usize, j: usize| values[i * (nv + 1) + j];
        for i in 0..nu {
            for j in 0..nv {
                // The cell's corners, counter-clockwise, and where the sign changes
                // along its sides.
                let corners = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
                let mut cuts: Vec<DVec2> = Vec::with_capacity(4);
                for k in 0..4 {
                    let (a, b) = (corners[k], corners[(k + 1) % 4]);
                    let (fa, fb) = (value(a.0, a.1), value(b.0, b.1));
                    if (fa < 0.0) != (fb < 0.0) {
                        let s = fa / (fa - fb);
                        cuts.push(at(a.0, a.1).lerp(at(b.0, b.1), s));
                    }
                }
                // Two cuts: one piece. Four (a saddle): two pieces, paired as they come.
                for pair in cuts.as_chunks::<2>().0 {
                    // The range is half open, like the generators': a piece on the seam
                    // counts at the low end and not again at the high end.
                    let mut middle = 0.5 * (pair[0] + pair[1]);
                    if middle.x > self.hi.x - SEAM_SLACK
                        || (extra_v == 1 && middle.y > self.hi.y - SEAM_SLACK)
                    {
                        continue;
                    }
                    middle.x = middle.x.max(self.lo.x + SEAM_SLACK);
                    if extra_v == 1 {
                        middle.y = middle.y.max(self.lo.y + SEAM_SLACK);
                    }
                    if self.contains(middle) {
                        let (a, b) = (self.surface.point(pair[0]), self.surface.point(pair[1]));
                        if a.distance(b) > tolerance::LINEAR {
                            out.push([a, b]);
                        }
                    }
                }
            }
        }
    }
}

/// The angles (in the cone's frame) of its silhouette rulings: two, one or none.
fn cone_angles(c: &Cone, view: View) -> Vec<f64> {
    // The normal along a ruling is (cos u cos α, sin u cos α, −sin α): it is square to
    // the view direction (or, in perspective, to the line from the apex to the eye) where
    // cos(u − φ) = tan α · dz / |d_xy|.
    let d = match view {
        View::Orthographic { dir } => c.frame.vector_to_local(dir),
        View::Perspective { eye } => c.frame.vector_to_local(eye - c.apex()),
    };
    let across = d.x.hypot(d.y);
    if !(across.is_finite() && across > tolerance::ANGULAR * d.length().max(f64::MIN_POSITIVE)) {
        return Vec::new();
    }
    let k = c.slope() * d.z / across;
    if k.abs() >= 1.0 {
        return Vec::new();
    }
    let phi = d.y.atan2(d.x);
    let w = k.acos();
    vec![phi - w, phi + w]
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

    fn total(lines: &[[DVec3; 2]]) -> f64 {
        lines.iter().map(|l| l[0].distance(l[1])).sum()
    }

    #[test]
    fn ball_cone_and_ring() {
        use crate::revolve::{RevolveAxis, revolve};
        let axis = RevolveAxis {
            origin: DVec2::ZERO,
            dir: DVec2::Y,
        };
        // A ball's outline is a great circle, whatever the direction.
        let ball = crate::primitive::ball(&peet_math::Frame::WORLD, 5.0).unwrap();
        for dir in [DVec3::Y, DVec3::Z, DVec3::new(0.3, -0.5, 0.8).normalize()] {
            let lines = silhouettes(&ball, View::Orthographic { dir });
            let length = total(&lines);
            assert!((length - TAU * 5.0).abs() < 0.02 * TAU * 5.0, "{length}");
            for l in &lines {
                for p in l {
                    assert!((p.length() - 5.0).abs() < 1e-9);
                    assert!(p.dot(dir).abs() < 0.05, "{p} is not on the outline");
                }
            }
        }
        // In perspective it is a smaller circle, nearer the eye.
        let eye = DVec3::new(0.0, -13.0, 0.0);
        let lines = silhouettes(&ball, View::Perspective { eye });
        let r = 5.0 * (1.0 - (5.0 / 13.0_f64).powi(2)).sqrt();
        assert!((total(&lines) - TAU * r).abs() < 0.02 * TAU * r);
        // A cone from the side: two rulings from the rim to the apex.
        let mut s = Sketch::new();
        for (a, b) in [
            (DVec2::ZERO, DVec2::new(5.0, 0.0)),
            (DVec2::new(5.0, 0.0), DVec2::new(0.0, 12.0)),
            (DVec2::new(0.0, 12.0), DVec2::ZERO),
        ] {
            s.add_line(a, b);
        }
        let cone = revolve(&Plane::front(), &find_regions(&s).regions, &axis, 0.0, TAU).unwrap();
        let lines = silhouettes(&cone, View::Orthographic { dir: DVec3::Y });
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!((total(&lines) - 2.0 * 13.0).abs() < 1e-9);
        // From above the apex, looking down: the whole cone faces the viewer.
        assert!(silhouettes(&cone, View::Orthographic { dir: -DVec3::Z }).is_empty());
        // A ring seen along its axis: its outer and inner equators.
        let mut s = Sketch::new();
        s.add_circle(DVec2::new(10.0, 0.0), 3.0);
        let ring = revolve(&Plane::front(), &find_regions(&s).regions, &axis, 0.0, TAU).unwrap();
        let lines = silhouettes(&ring, View::Orthographic { dir: DVec3::Z });
        let expected = TAU * 13.0 + TAU * 7.0;
        assert!((total(&lines) - expected).abs() < 0.02 * expected);
    }
}
