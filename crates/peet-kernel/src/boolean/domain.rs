//! Faces seen in a flat 2D domain, and point-in-face classification.
//!
//! The domain of a plane is its own coordinate system; the domain of a cylinder is the
//! cylinder unrolled (`x = r·θ`, `y = height`), which is isometric, so straight rulings and
//! circles stay straight and ellipses become sinusoids. The `x` axis is mirrored for
//! `reversed` faces, so in every domain a face's outer loop runs counter-clockwise and the
//! face lies to the left of its coedges.
//!
//! A cylinder's domain is periodic in `x`. Loops are traced continuously (each point is
//! placed near the previous one), so a full cylinder wall becomes a rectangle with its seam
//! on both sides.

use std::cell::OnceCell;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

use peet_math::tolerance::LINEAR;
use peet_math::{Aabb, DVec2, DVec3};

use super::util::{bounded_distance, curve_bounds, grow};
use crate::geom::{Curve3, Surface};
use crate::topo::{EdgeId, FaceId, Solid};

/// A point this close (mm) to a seam is classified as if it were just beside it: a seam is
/// not a boundary, the face continues on both sides.
const SEAM_NUDGE: f64 = 16.0 * LINEAR;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Domain {
    pub surface: Surface,
    pub flip: bool,
}

impl Domain {
    pub fn new(surface: &Surface, reversed: bool) -> Self {
        Self {
            surface: *surface,
            flip: reversed,
        }
    }

    /// Length of one turn in `x`, for cylinders.
    pub fn period(&self) -> Option<f64> {
        match &self.surface {
            Surface::Plane(_) => None,
            Surface::Cylinder(c) => Some(TAU * c.radius),
        }
    }

    fn scale(&self) -> f64 {
        let s = match &self.surface {
            Surface::Plane(_) => 1.0,
            Surface::Cylinder(c) => c.radius,
        };
        if self.flip { -s } else { s }
    }

    /// Domain coordinates of (the projection of) `p`; cylinder angles in `(−π, π]`.
    pub fn map(&self, p: DVec3) -> DVec2 {
        let uv = self.surface.param(p);
        DVec2::new(uv.x * self.scale(), uv.y)
    }

    /// `q` moved by whole turns so its `x` is as close as possible to `x`.
    pub fn shift_near(&self, q: DVec2, x: f64) -> DVec2 {
        match self.period() {
            Some(period) => DVec2::new(q.x + ((x - q.x) / period).round() * period, q.y),
            None => q,
        }
    }

    pub fn map_near(&self, p: DVec3, x: f64) -> DVec2 {
        self.shift_near(self.map(p), x)
    }

    /// The surface point at domain coordinates `q`.
    pub fn point(&self, q: DVec2) -> DVec3 {
        self.surface.point(DVec2::new(q.x / self.scale(), q.y))
    }

    /// Outward normal of the face at (the projection of) `p`.
    pub fn outward(&self, p: DVec3) -> DVec3 {
        let n = self.surface.normal_at(p);
        if self.flip { -n } else { n }
    }

    /// Whether the image of `curve` in the domain is a straight segment.
    fn is_straight(&self, curve: &Curve3) -> bool {
        matches!(
            (curve, &self.surface),
            (Curve3::Line(_), _) | (Curve3::Circle(_), Surface::Cylinder(_))
        )
    }

    /// How many straight pieces approximate a part of `curve` spanning `span` of parameter.
    pub fn segments(&self, curve: &Curve3, span: f64) -> usize {
        match (curve, &self.surface) {
            (Curve3::Line(_), _) => 1,
            // Straight in the domain, but the angle must be tracked around the turn.
            (Curve3::Circle(_), Surface::Cylinder(_)) => {
                ((span.abs() / (PI / 3.0)).ceil() as usize).max(1)
            }
            // Curved in the domain. Even a short arc gets a few pieces, so thin regions
            // next to a tangency keep their shape.
            _ => ((span.abs() / (PI / 24.0)).ceil() as usize).max(4),
        }
    }

    /// Appends the image of `curve` from `ta` to `tb` to `out`, continuing from its last
    /// point (the curve's start itself is not added unless `out` is empty).
    pub fn trace(&self, curve: &Curve3, ta: f64, tb: f64, out: &mut Vec<DVec2>) {
        let n = self.segments(curve, tb - ta);
        let first = if out.is_empty() { 0 } else { 1 };
        for i in first..=n {
            let t = ta + (tb - ta) * i as f64 / n as f64;
            let p = curve.point(t);
            let q = match out.last() {
                Some(prev) => self.map_near(p, prev.x),
                None => self.map(p),
            };
            out.push(q);
        }
    }
}

/// Twice the signed area of a closed polygon.
pub(crate) fn signed_area(poly: &[DVec2]) -> f64 {
    let mut a = 0.0;
    for i in 0..poly.len() {
        a += poly[i].perp_dot(poly[(i + 1) % poly.len()]);
    }
    0.5 * a
}

/// Winding number of a closed polygon around `q`.
pub(crate) fn polygon_winding(poly: &[DVec2], q: DVec2) -> i32 {
    let mut total = 0.0;
    for i in 0..poly.len() {
        total += (poly[i] - q).angle_to(poly[(i + 1) % poly.len()] - q);
    }
    (total / TAU).round() as i32
}

#[derive(Clone, Debug)]
pub(crate) struct BoundaryEdge {
    pub id: EdgeId,
    pub curve: Curve3,
    pub t0: f64,
    pub t1: f64,
    pub bounds: Aabb,
    /// Used twice by the face: the face continues on both sides.
    pub seam: bool,
}

/// One directed edge traced in the domain: parameters and points in loop direction.
#[derive(Clone, Debug)]
struct Trace {
    curve: Curve3,
    straight: bool,
    ts: Vec<f64>,
    qs: Vec<DVec2>,
}

/// A loop traced in the domain, able to give its exact winding number around a point:
/// the sampling is refined along the true curves wherever a chord could misjudge it.
#[derive(Clone, Debug)]
pub(crate) struct ExactLoop {
    traces: Vec<Trace>,
    /// Bounds of the loop in the domain (generous: the curves bulge past their samples).
    lo: DVec2,
    hi: DVec2,
}

impl ExactLoop {
    /// Traces the directed curve parts `(curve, from, to)` of a loop, in order. The first
    /// point is placed near `start_x` if given (on a cylinder), and each next point near
    /// the one before.
    pub fn new(domain: &Domain, parts: &[(Curve3, f64, f64)], start_x: Option<f64>) -> Self {
        let mut traces = Vec::with_capacity(parts.len());
        let mut last_x = start_x;
        let mut lo = DVec2::splat(f64::INFINITY);
        let mut hi = DVec2::splat(f64::NEG_INFINITY);
        let mut bulge = 0.0_f64;
        for &(curve, ta, tb) in parts {
            let n = domain.segments(&curve, tb - ta);
            let straight = domain.is_straight(&curve);
            let mut ts = Vec::with_capacity(n + 1);
            let mut qs: Vec<DVec2> = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let t = ta + (tb - ta) * i as f64 / n as f64;
                let p = curve.point(t);
                let q = match last_x {
                    Some(x) => domain.map_near(p, x),
                    None => domain.map(p),
                };
                if !straight && i > 0 {
                    // How far the curve leaves its chord, measured at the middle.
                    let (t0, q0): (f64, DVec2) = (ts[i - 1], qs[i - 1]);
                    let mid = domain.map_near(curve.point(0.5 * (t0 + t)), q0.x);
                    bulge = bulge.max(mid.distance(0.5 * (q0 + q)));
                }
                last_x = Some(q.x);
                lo = lo.min(q);
                hi = hi.max(q);
                ts.push(t);
                qs.push(q);
            }
            traces.push(Trace {
                curve,
                straight,
                ts,
                qs,
            });
        }
        let margin = DVec2::splat(2.0 * bulge + LINEAR);
        Self {
            traces,
            lo: lo - margin,
            hi: hi + margin,
        }
    }

    /// Angle the loop sweeps around `q` (2π times its winding number). `q` must be placed
    /// in the same turn as the loop and must not lie on it.
    pub fn sweep(&self, domain: &Domain, q: DVec2) -> f64 {
        // A closed loop doesn't wind around a point outside its bounds.
        if q.cmplt(self.lo).any() || q.cmpgt(self.hi).any() {
            return 0.0;
        }
        let mut total = 0.0;
        let mut prev: Option<DVec2> = None;
        let mut first: Option<DVec2> = None;
        for tr in &self.traces {
            if let Some(pq) = prev {
                total += (pq - q).angle_to(tr.qs[0] - q);
            }
            first.get_or_insert(tr.qs[0]);
            for i in 0..tr.ts.len() - 1 {
                total += sweep(
                    domain,
                    tr,
                    tr.ts[i],
                    tr.qs[i],
                    tr.ts[i + 1],
                    tr.qs[i + 1],
                    q,
                    0,
                );
            }
            prev = tr.qs.last().copied();
        }
        if let (Some(a), Some(b)) = (prev, first) {
            total += (a - q).angle_to(b - q);
        }
        total
    }
}

/// Winding number of a set of loops (already placed in one turn) around `q`.
pub(crate) fn exact_winding(domain: &Domain, loops: &[ExactLoop], q: DVec2) -> i64 {
    let total: f64 = loops.iter().map(|l| l.sweep(domain, q)).sum();
    (total / TAU).round() as i64
}

/// Angle swept around `q` by the image of a curve part. A chord can only misjudge the
/// sweep when it is large, so large sweeps are subdivided until they are not.
#[allow(clippy::too_many_arguments)]
fn sweep(
    domain: &Domain,
    tr: &Trace,
    t0: f64,
    q0: DVec2,
    t1: f64,
    q1: DVec2,
    q: DVec2,
    depth: u32,
) -> f64 {
    let a = (q0 - q).angle_to(q1 - q);
    if tr.straight || a.abs() < FRAC_PI_2 || depth >= 48 {
        return a;
    }
    let tm = 0.5 * (t0 + t1);
    let qm = domain.map_near(tr.curve.point(tm), q0.x);
    sweep(domain, tr, t0, q0, tm, qm, q, depth + 1)
        + sweep(domain, tr, tm, qm, t1, q1, q, depth + 1)
}

/// Where a point of a face's surface lies relative to the face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Where {
    Inside,
    Boundary,
    Outside,
}

/// An input face prepared for intersection and classification queries.
#[derive(Clone, Debug)]
pub(crate) struct FaceGeom {
    pub surface: Surface,
    pub reversed: bool,
    pub domain: Domain,
    pub bounds: Aabb,
    /// The distinct edges of the face's loops.
    pub edges: Vec<BoundaryEdge>,
    /// Positions of the face's vertices.
    pub vertices: Vec<DVec3>,
    /// The loops as directed curve parts, and their traces (made on first use: most faces
    /// of a big body are never asked about).
    parts: Vec<Vec<(Curve3, f64, f64)>>,
    loops: OnceCell<Vec<ExactLoop>>,
}

impl FaceGeom {
    pub fn new(solid: &Solid, id: FaceId) -> Self {
        let face = solid.face(id);
        let domain = Domain::new(&face.surface, face.reversed);
        let mut edges: Vec<BoundaryEdge> = Vec::new();
        let mut vertices = Vec::new();
        let mut loops = Vec::new();
        let mut bounds = Aabb::EMPTY;
        for &l in &face.loops {
            let coedges = solid.loop_coedges(l);
            for &c in &coedges {
                let eid = solid.coedge(c).edge;
                if let Some(known) = edges.iter_mut().find(|e| e.id == eid) {
                    known.seam = true;
                    continue;
                }
                let e = solid.edge(eid);
                let b = curve_bounds(&e.curve, e.t0, e.t1);
                bounds = bounds.union(&b);
                edges.push(BoundaryEdge {
                    id: eid,
                    curve: e.curve,
                    t0: e.t0,
                    t1: e.t1,
                    bounds: b,
                    seam: false,
                });
                for v in [e.start, e.end] {
                    let p = solid.vertex(v).point;
                    if !vertices.contains(&p) {
                        vertices.push(p);
                    }
                }
            }
            let parts: Vec<(Curve3, f64, f64)> = coedges
                .iter()
                .map(|&c| {
                    let co = solid.coedge(c);
                    let e = solid.edge(co.edge);
                    if co.reversed {
                        (e.curve, e.t1, e.t0)
                    } else {
                        (e.curve, e.t0, e.t1)
                    }
                })
                .collect();
            loops.push(parts);
        }
        Self {
            surface: face.surface,
            reversed: face.reversed,
            domain,
            bounds: grow(&bounds, LINEAR),
            edges,
            vertices,
            parts: loops,
            loops: OnceCell::new(),
        }
    }

    /// Classifies a point of the face's surface: inside the face, on its boundary (within
    /// tolerance of an edge), or outside. Exact for curved edges: the boundary test uses the
    /// edge curves themselves, and the winding number refines its sampling near the point.
    pub fn locate(&self, p: DVec3) -> Where {
        if !self.bounds.contains(p) {
            return Where::Outside;
        }
        for e in &self.edges {
            if !e.seam
                && grow(&e.bounds, LINEAR).contains(p)
                && bounded_distance(&e.curve, e.t0, e.t1, p).0 <= LINEAR
            {
                return Where::Boundary;
            }
        }
        let mut q = self.domain.map(p);
        // On a seam the face continues on both sides: judge the point just beside it.
        let on_seam = self.edges.iter().any(|e| {
            e.seam
                && grow(&e.bounds, SEAM_NUDGE).contains(p)
                && bounded_distance(&e.curve, e.t0, e.t1, p).0 < SEAM_NUDGE
        });
        if on_seam {
            q.x += 2.0 * SEAM_NUDGE;
        }
        let mut total = 0.0;
        let loops = self.loops.get_or_init(|| {
            self.parts
                .iter()
                .map(|parts| ExactLoop::new(&self.domain, parts, None))
                .collect()
        });
        for l in loops {
            match self.domain.period() {
                // A traced loop can be wider than one turn (a full ring with a skirt that
                // straddles the seam), so every placement of the point within the loop's
                // extent counts. The face covers each surface point at most once.
                Some(period) => {
                    let first = ((l.lo.x - q.x) / period).ceil() as i64;
                    let last = ((l.hi.x - q.x) / period).floor() as i64;
                    for k in first..=last {
                        total += l.sweep(&self.domain, DVec2::new(q.x + k as f64 * period, q.y));
                    }
                }
                None => total += l.sweep(&self.domain, q),
            }
        }
        if (total / TAU).round() as i64 != 0 {
            Where::Inside
        } else {
            Where::Outside
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boolean::tests::shapes;
    use crate::topo::test_shapes::cuboid;

    #[test]
    fn locate_on_a_box_face() {
        let s = cuboid(DVec3::ZERO, DVec3::new(4.0, 2.0, 1.0));
        // Face 1 is the top (z = 1).
        let f = FaceGeom::new(&s, FaceId(1));
        assert_eq!(f.locate(DVec3::new(1.0, 1.0, 1.0)), Where::Inside);
        assert_eq!(f.locate(DVec3::new(4.0, 1.0, 1.0)), Where::Boundary);
        assert_eq!(f.locate(DVec3::new(4.0, 2.0, 1.0)), Where::Boundary);
        assert_eq!(f.locate(DVec3::new(4.1, 1.0, 1.0)), Where::Outside);
        assert_eq!(f.locate(DVec3::new(3.99999, 1.99999, 1.0)), Where::Inside);
    }

    #[test]
    fn locate_on_a_cylinder() {
        let s = shapes::cylinder(DVec3::ZERO, DVec3::Z, 2.0, 5.0);
        for id in s.face_ids() {
            let f = FaceGeom::new(&s, id);
            match f.surface {
                Surface::Cylinder(_) => {
                    for k in 0..40 {
                        let a = 0.17 * f64::from(k);
                        let at = |z: f64| DVec3::new(2.0 * a.cos(), 2.0 * a.sin(), z);
                        assert_eq!(f.locate(at(2.5)), Where::Inside, "angle {a}");
                        assert_eq!(f.locate(at(5.0)), Where::Boundary);
                        assert_eq!(f.locate(at(5.5)), Where::Outside);
                        assert_eq!(f.locate(at(-0.1)), Where::Outside);
                    }
                    // On the seam itself the face continues.
                    assert_eq!(f.locate(DVec3::new(2.0, 0.0, 1.0)), Where::Inside);
                }
                Surface::Plane(p) => {
                    let z = p.origin().z;
                    assert_eq!(f.locate(DVec3::new(0.5, -1.0, z)), Where::Inside);
                    assert_eq!(f.locate(DVec3::new(1.999, 0.0, z)), Where::Inside);
                    assert_eq!(f.locate(DVec3::new(2.0, 0.0, z)), Where::Boundary);
                    assert_eq!(f.locate(DVec3::new(0.0, 2.001, z)), Where::Outside);
                    // Just inside the arc, where a coarse polygon would say outside.
                    let a = 0.4_f64;
                    let r = 2.0 - 1e-5;
                    assert_eq!(
                        f.locate(DVec3::new(r * a.cos(), r * a.sin(), z)),
                        Where::Inside
                    );
                    let r = 2.0 + 1e-5;
                    assert_eq!(
                        f.locate(DVec3::new(r * a.cos(), r * a.sin(), z)),
                        Where::Outside
                    );
                }
            }
        }
    }
}
