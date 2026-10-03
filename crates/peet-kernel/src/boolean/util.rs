//! Small geometric helpers shared by the boolean stages, and the vertex pool.

use std::collections::HashMap;
use std::f64::consts::{PI, TAU};

use peet_math::tolerance::{self, LINEAR};
use peet_math::{Aabb, DVec3};

use crate::geom::{Curve3, Surface};

/// Second derivative of a curve with respect to its parameter.
pub(crate) fn second_derivative(c: &Curve3, t: f64) -> DVec3 {
    c.second_derivative(t)
}

/// The parameter step that moves a point on `c` by about [`LINEAR`].
pub(crate) fn param_tol(c: &Curve3) -> f64 {
    match c {
        Curve3::Line(_) => LINEAR,
        Curve3::Circle(ci) => LINEAR / ci.radius.max(LINEAR),
        Curve3::Ellipse(e) => LINEAR / e.minor.max(LINEAR),
        Curve3::Nurbs(n) => {
            // By the curve's average speed: the length of its control polygon over its
            // parameter range.
            let (lo, hi) = n.domain();
            let length: f64 = n
                .control_points()
                .windows(2)
                .map(|w| w[0].distance(w[1]))
                .sum();
            LINEAR * (hi - lo) / length.max(LINEAR)
        }
    }
}

/// Parameter of the point of `c` closest to `p`. For periodic curves the result is
/// unwrapped into `[lo − tol, lo − tol + 2π)`, so a point at the start of a range beginning
/// at `lo` maps to (about) `lo` rather than `lo + 2π`.
pub(crate) fn param_from(c: &Curve3, p: DVec3, lo: f64) -> f64 {
    let t = c.param(p);
    if c.period().is_some() {
        let tol = param_tol(c);
        lo - tol + (t - lo + tol).rem_euclid(TAU)
    } else {
        t
    }
}

/// Distance from `p` to the part `t0..=t1` of `c`, and the parameter of the closest point.
pub(crate) fn bounded_distance(c: &Curve3, t0: f64, t1: f64, p: DVec3) -> (f64, f64) {
    let t = param_from(c, p, t0);
    if t >= t0 && t <= t1 {
        (c.point(t).distance(p), t)
    } else {
        let (d0, d1) = (c.point(t0).distance(p), c.point(t1).distance(p));
        if d0 <= d1 { (d0, t0) } else { (d1, t1) }
    }
}

/// Bounds of the part `t0..=t1` of a curve (slightly generous for curved ones).
pub(crate) fn curve_bounds(c: &Curve3, t0: f64, t1: f64) -> Aabb {
    let mut b = Aabb::EMPTY;
    b.extend(c.point(t0));
    b.extend(c.point(t1));
    let (n, sagitta) = match c {
        Curve3::Line(_) => return b,
        Curve3::Circle(_) | Curve3::Ellipse(_) => {
            let size = match c {
                Curve3::Circle(ci) => ci.radius,
                Curve3::Ellipse(e) => e.major,
                _ => 0.0,
            };
            let n = (((t1 - t0) / (PI / 16.0)).ceil() as usize).clamp(1, 64);
            // The curve bulges past its chords by at most the sagitta.
            let step = (t1 - t0) / n as f64;
            (n, size * (1.0 - (step * 0.5).cos()))
        }
        Curve3::Nurbs(nurbs) => {
            // A chord of parameter length h leaves the curve by at most |C''| h² / 8.
            let n = 32;
            let step = (t1 - t0) / n as f64;
            (n, nurbs.bend() * step * step / 8.0)
        }
    };
    let step = (t1 - t0) / n as f64;
    for i in 1..n {
        b.extend(c.point(t0 + step * i as f64));
    }
    grow(&b, sagitta)
}

/// `b` grown by `margin` on every side.
pub(crate) fn grow(b: &Aabb, margin: f64) -> Aabb {
    Aabb {
        min: b.min - DVec3::splat(margin),
        max: b.max + DVec3::splat(margin),
    }
}

pub(crate) fn boxes_overlap(a: &Aabb, b: &Aabb) -> bool {
    a.min.cmple(b.max).all() && b.min.cmple(a.max).all()
}

/// Whether two surfaces are the same point set (within tolerance), whatever their frames.
pub(crate) fn same_surface(a: &Surface, b: &Surface) -> bool {
    match (a, b) {
        (Surface::Plane(p), Surface::Plane(q)) => {
            tolerance::directions_parallel(p.normal(), q.normal())
                && p.signed_distance(q.origin()).abs() <= LINEAR
        }
        (Surface::Cylinder(c), Surface::Cylinder(d)) => {
            if !tolerance::directions_parallel(c.axis(), d.axis())
                || (c.radius - d.radius).abs() > LINEAR
            {
                return false;
            }
            let w = d.axis_origin() - c.axis_origin();
            (w - c.axis() * w.dot(c.axis())).length() <= LINEAR
        }
        (Surface::Plane(_) | Surface::Cylinder(_), _)
        | (_, Surface::Plane(_) | Surface::Cylinder(_)) => false,
        // Freeform surfaces are the same only if they are the same definition.
        (Surface::Nurbs(s), Surface::Nurbs(t)) => std::sync::Arc::ptr_eq(s, t) || s == t,
        (Surface::Nurbs(_), _) | (_, Surface::Nurbs(_)) => false,
        _ => matches!(
            super::ssi::intersect(a, b, DVec3::ZERO),
            super::ssi::Ssi::Coincident
        ),
    }
}

/// Whether the curves of two edges are the same point set near the given ranges: sampled
/// points of each range lie on the other curve.
pub(crate) fn same_carrier(a: &Curve3, a0: f64, a1: f64, b: &Curve3, b0: f64, b1: f64) -> bool {
    [0.25, 0.5, 0.75].iter().all(|&k| {
        a.distance(b.point(b0 + (b1 - b0) * k)) <= LINEAR
            && b.distance(a.point(a0 + (a1 - a0) * k)) <= LINEAR
    })
}

/// All vertices of the operation: points closer than [`LINEAR`] are one vertex. Every
/// stage inserts its points here, so a point computed twice (by two face pairs meeting at
/// an edge, say) becomes a single shared vertex.
pub(crate) struct VertexPool {
    pub points: Vec<DVec3>,
    cells: HashMap<[i64; 3], Vec<u32>>,
}

/// Grid cell size of the pool's spatial hash.
const CELL: f64 = 64.0 * LINEAR;

impl VertexPool {
    pub fn new() -> Self {
        Self {
            points: Vec::new(),
            cells: HashMap::new(),
        }
    }

    fn key(p: DVec3) -> [i64; 3] {
        [
            (p.x / CELL).floor() as i64,
            (p.y / CELL).floor() as i64,
            (p.z / CELL).floor() as i64,
        ]
    }

    /// The pooled vertex within tolerance of `p`, if any (the nearest one).
    pub fn find(&self, p: DVec3) -> Option<u32> {
        let k = Self::key(p);
        let mut best: Option<(f64, u32)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = self.cells.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else {
                        continue;
                    };
                    for &i in list {
                        let d = self.points[i as usize].distance_squared(p);
                        if d <= LINEAR * LINEAR && best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, i));
                        }
                    }
                }
            }
        }
        best.map(|(_, i)| i)
    }

    pub fn insert(&mut self, p: DVec3) -> u32 {
        if let Some(i) = self.find(p) {
            return i;
        }
        let i = self.points.len() as u32;
        self.points.push(p);
        self.cells.entry(Self::key(p)).or_default().push(i);
        i
    }
}

/// An edge of the operation: part `t0..t1` of a curve between two pooled vertices.
#[derive(Clone, Debug)]
pub(crate) struct GEdge {
    pub curve: Curve3,
    pub t0: f64,
    pub t1: f64,
    pub start: u32,
    pub end: u32,
}

impl GEdge {
    pub fn mid(&self) -> DVec3 {
        self.curve.point(0.5 * (self.t0 + self.t1))
    }
}

/// A directed use of an edge: `(edge index, forward)`.
pub(crate) type HalfEdge = (u32, bool);
