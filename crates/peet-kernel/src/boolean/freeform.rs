//! Intersection of a freeform surface with another surface, by marching.
//!
//! The analytic pairs have curves in closed form ([`super::ssi`]). With a NURBS surface
//! `S` there is none, so the curve is traced: the other surface `T` gives every point a
//! signed distance, which makes `f(u, v) = distance(S(u, v), T)` a function on `S`'s
//! parameter rectangle whose zero contour is the intersection.
//!
//! 1. *Seed.* `f` is sampled on a grid over the part of the rectangle that can matter
//!    (where `S` is inside the region asked about). Every sign change along a grid line
//!    is refined to a point of the contour.
//! 2. *Trace.* From a seed the contour is followed both ways with a predictor–corrector:
//!    a step along the contour's direction, then Newton's method back onto `f = 0`. A
//!    branch ends where it leaves `S`'s rectangle or `T`'s own extent (found by
//!    bisection, so the curve ends on that boundary), or closes on itself. Seeds the
//!    branch passes are used up.
//! 3. *Fit.* The traced points, with the contour's exact direction at each (the cross
//!    product of the two normals), define a cubic through them. Where the cubic leaves
//!    either surface by more than a fraction of the linear tolerance, a point is added
//!    in between and the fit repeated.
//!
//! The result is one NURBS curve per branch; a closed branch is given as two halves, so
//! no edge is a closed freeform curve. Surfaces that touch without crossing (the
//! contour's direction vanishes) are reported as unsupported.

use std::sync::Arc;

use peet_math::tolerance::LINEAR;
use peet_math::{Aabb, DVec2, DVec3};

use super::ssi::Ssi;
use crate::geom::{Curve3, Surface};
use crate::nurbs::{NurbsCurve, NurbsSurface};

/// The fitted curve may leave either surface by this much (mm).
const FIT: f64 = 0.2 * LINEAR;

/// A traced point counts as on the contour within this (mm).
const ON: f64 = 1e-10;

/// Grid cells across the part of the surface searched, at least.
const CELLS: usize = 24;

/// A branch is given up after this many steps.
const MAX_STEPS: usize = 20_000;

/// The intersection of the freeform surface `s` with `other`, within `region` (the
/// whole surface if `None`).
pub(crate) fn intersect(s: &Arc<NurbsSurface>, other: &Surface, region: Option<&Aabb>) -> Ssi {
    if let Surface::Nurbs(t) = other
        && (Arc::ptr_eq(s, t) || **s == **t)
    {
        return Ssi::Coincident;
    }
    let march = March::new(s, other, region);
    let Some(march) = march else {
        return Ssi::None;
    };
    match march.curves() {
        Ok(curves) if curves.is_empty() => Ssi::None,
        Ok(curves) => Ssi::Curves(curves),
        Err(()) => Ssi::Unsupported,
    }
}

/// A point of the contour: parameters on `s`, and the point.
#[derive(Clone, Copy, Debug)]
struct Node {
    uv: DVec2,
    point: DVec3,
}

struct March<'a> {
    s: &'a NurbsSurface,
    other: &'a Surface,
    /// The part of `s`'s rectangle searched.
    lo: DVec2,
    hi: DVec2,
    /// Millimetres per unit of each parameter.
    scale: DVec2,
    /// Grid cells in each direction, and their size in parameters.
    cells: (usize, usize),
    cell: DVec2,
}

impl<'a> March<'a> {
    fn new(s: &'a NurbsSurface, other: &'a Surface, region: Option<&Aabb>) -> Option<Self> {
        let (mut lo, mut hi) = s.domain();
        if let Some(region) = region {
            // The part of the rectangle whose points are in the region, from a coarse
            // sampling, with a cell to spare on every side.
            const COARSE: usize = 16;
            let step = (hi - lo) / COARSE as f64;
            let margin = 0.5 * (s.stretch() * step).length();
            let grown = Aabb {
                min: region.min - DVec3::splat(margin),
                max: region.max + DVec3::splat(margin),
            };
            let mut inside = (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY));
            for i in 0..=COARSE {
                for j in 0..=COARSE {
                    let uv = lo + step * DVec2::new(i as f64, j as f64);
                    if grown.contains(s.point(uv)) {
                        inside = (inside.0.min(uv), inside.1.max(uv));
                    }
                }
            }
            if inside.0.x > inside.1.x {
                return None;
            }
            (lo, hi) = ((inside.0 - step).max(lo), (inside.1 + step).min(hi));
        }
        // A few cells per polynomial piece, and no fewer than CELLS across.
        let (bu, bv) = s.breaks();
        let (du, dv) = s.degrees();
        let whole = s.domain();
        let count = |breaks: usize, degree: usize, part: f64, all: f64| {
            let by_spans = (breaks - 1) as f64 * (degree + 1) as f64 * 2.0 * part / all;
            (by_spans.ceil() as usize).clamp(CELLS, 256)
        };
        let cells = (
            count(bu.len(), du, hi.x - lo.x, whole.1.x - whole.0.x),
            count(bv.len(), dv, hi.y - lo.y, whole.1.y - whole.0.y),
        );
        Some(Self {
            s,
            other,
            lo,
            hi,
            scale: s.stretch().max(DVec2::splat(1e-9)),
            cells,
            cell: (hi - lo) / DVec2::new(cells.0 as f64, cells.1 as f64),
        })
    }

    /// The signed distance from the point of `s` at `uv` to the other surface, or
    /// `None` where the other surface has ended (the nearest point of a freeform one is
    /// on its edge, to one side).
    fn distance(&self, uv: DVec2) -> Option<f64> {
        let p = self.s.point(uv);
        match self.other {
            Surface::Nurbs(t) => {
                let at = t.param(p);
                let q = t.point(at);
                let n = t.normal(at);
                let off = p - q;
                let across = off.dot(n);
                let (lo, hi) = t.domain();
                let on_edge = at.cmple(lo).any() || at.cmpge(hi).any();
                if on_edge && (off - n * across).length() > ON.max(1e-9 * off.length()) {
                    return None;
                }
                Some(across)
            }
            analytic => Some(analytic.signed_distance(p)),
        }
    }

    /// The other surface's normal at the point nearest `p`.
    fn other_normal(&self, p: DVec3) -> DVec3 {
        self.other.normal_at(p)
    }

    /// The gradient of the distance in scaled parameters (mm), at `uv`.
    fn gradient(&self, uv: DVec2) -> DVec2 {
        let [p, su, sv, ..] = self.s.evaluate(uv);
        let n = self.other_normal(p);
        DVec2::new(n.dot(su), n.dot(sv)) / self.scale
    }

    fn inside(&self, uv: DVec2) -> bool {
        uv.cmpge(self.lo).all() && uv.cmple(self.hi).all()
    }

    /// Newton's method from `uv` back onto the contour, along the gradient. `None` if it
    /// doesn't get there (the other surface ends, or the surfaces only touch).
    fn correct(&self, mut uv: DVec2) -> Option<DVec2> {
        for _ in 0..20 {
            let f = self.distance(uv)?;
            if f.abs() <= ON {
                return Some(uv);
            }
            let g = self.gradient(uv);
            let g2 = g.length_squared();
            if g2 <= 1e-16 {
                return None;
            }
            // A step of −f g / |g|² in scaled parameters.
            uv -= g * (f / g2) / self.scale;
        }
        None
    }

    /// The contour's direction at `uv` in scaled parameters, turned to go with `like`.
    /// `None` where the gradient vanishes: the surfaces touch.
    fn direction(&self, uv: DVec2, like: Option<DVec2>) -> Option<DVec2> {
        let g = self.gradient(uv);
        let t = DVec2::new(-g.y, g.x).try_normalize()?;
        if g.length() <= 1e-7 {
            return None;
        }
        Some(match like {
            Some(l) if l.dot(t) < 0.0 => -t,
            _ => t,
        })
    }

    /// The seeds: points of the contour on the grid's lines.
    fn seeds(&self) -> Vec<DVec2> {
        let (nu, nv) = self.cells;
        let at = |i: usize, j: usize| self.lo + self.cell * DVec2::new(i as f64, j as f64);
        let values: Vec<Option<f64>> = (0..=nu)
            .flat_map(|i| (0..=nv).map(move |j| (i, j)))
            .map(|(i, j)| self.distance(at(i, j)))
            .collect();
        let value = |i: usize, j: usize| values[i * (nv + 1) + j];
        let mut seeds = Vec::new();
        let mut cross = |a: (usize, usize), b: (usize, usize)| {
            let (Some(fa), Some(fb)) = (value(a.0, a.1), value(b.0, b.1)) else {
                return;
            };
            if (fa < 0.0) == (fb < 0.0) {
                return;
            }
            // Bisect along the grid line.
            let (mut p, mut q) = (at(a.0, a.1), at(b.0, b.1));
            for _ in 0..60 {
                let m = 0.5 * (p + q);
                match self.distance(m) {
                    Some(fm) if (fm < 0.0) == (fa < 0.0) => p = m,
                    Some(_) => q = m,
                    None => return,
                }
            }
            let m = 0.5 * (p + q);
            if self.distance(m).is_some_and(|f| f.abs() <= 1e-7) {
                seeds.push(m);
            }
        };
        for i in 0..=nu {
            for j in 0..=nv {
                if i < nu {
                    cross((i, j), (i + 1, j));
                }
                if j < nv {
                    cross((i, j), (i, j + 1));
                }
            }
        }
        seeds
    }

    /// Follows the contour from `start` in the direction `dir` (scaled parameters).
    /// Returns the points after `start` and whether the branch came back to `start`.
    fn trace(&self, start: DVec2, dir: DVec2) -> Result<(Vec<DVec2>, bool), ()> {
        let h = self.step();
        let mut out = Vec::new();
        let mut uv = start;
        let mut heading = dir;
        for step in 0..MAX_STEPS {
            let t = self.direction(uv, Some(heading)).ok_or(())?;
            // Midpoint rule: the direction halfway along the step.
            let half = uv + t * (0.5 * h) / self.scale;
            let t_mid = if self.inside(half) {
                self.direction(half, Some(t)).unwrap_or(t)
            } else {
                t
            };
            let ahead = uv + t_mid * h / self.scale;
            let next = if self.inside(ahead) {
                self.correct(ahead).filter(|n| self.inside(*n))
            } else {
                None
            };
            let Some(next) = next else {
                // The branch ends within this step: on the rectangle's edge, or where
                // the other surface ends. Bisect the step for the last good point.
                let (mut good, mut bad) = (0.0_f64, 1.0_f64);
                let mut last = None;
                for _ in 0..50 {
                    let mid = 0.5 * (good + bad);
                    let trial = uv + t_mid * (h * mid) / self.scale;
                    let trial = trial.clamp(self.lo, self.hi);
                    match self.correct(trial).filter(|n| self.inside(*n)) {
                        Some(n) => {
                            good = mid;
                            last = Some(n);
                        }
                        None => bad = mid,
                    }
                }
                if let Some(end) = last
                    && ((end - uv) * self.scale).length() > 1e-9
                {
                    out.push(end);
                }
                return Ok((out, false));
            };
            heading = ((next - uv) * self.scale).normalize_or(t_mid);
            uv = next;
            // Back at the start: a closed branch.
            if step > 2 && ((uv - start) * self.scale).length() < 0.75 * h {
                return Ok((out, true));
            }
            out.push(uv);
        }
        Err(())
    }

    /// The length of a tracing step, in millimetres: a small part of the stretch of
    /// surface searched (the fit adds points where the curve needs more), but no finer
    /// than the grid that found the seeds needs.
    fn step(&self) -> f64 {
        const STEPS_ACROSS: f64 = 48.0;
        let extent = (self.hi - self.lo) * self.scale;
        let cell = self.cell * self.scale;
        (extent.max_element() / STEPS_ACROSS).max(cell.min_element() / 3.0)
    }

    /// Every branch of the contour, as fitted curves.
    fn curves(&self) -> Result<Vec<Curve3>, ()> {
        let mut seeds = self.seeds();
        let reach = ((self.cell * self.scale).length() * 0.6).max(self.step());
        let mut curves = Vec::new();
        while let Some(seed) = seeds.pop() {
            let Some(seed) = self.correct(seed) else {
                continue;
            };
            let dir = self.direction(seed, None).ok_or(())?;
            let (forward, closed) = self.trace(seed, dir)?;
            let mut branch: Vec<DVec2> = Vec::new();
            if !closed {
                let (backward, _) = self.trace(seed, -dir)?;
                branch.extend(backward.into_iter().rev());
            }
            branch.push(seed);
            branch.extend(forward);
            if closed {
                branch.push(seed);
            }
            // Seeds on this branch are done with.
            seeds.retain(|other| {
                !branch
                    .iter()
                    .any(|b| ((*b - *other) * self.scale).length() <= reach)
            });
            if branch.len() < 2 {
                continue;
            }
            let nodes: Vec<Node> = branch
                .iter()
                .map(|&uv| Node {
                    uv,
                    point: self.s.point(uv),
                })
                .collect();
            if closed && nodes.len() >= 5 {
                // Two halves, so no edge is a closed freeform curve.
                let middle = nodes.len() / 2;
                curves.extend(self.fit(&nodes[..=middle]));
                curves.extend(self.fit(&nodes[middle..]));
            } else {
                curves.extend(self.fit(&nodes));
            }
        }
        Ok(curves)
    }

    /// The contour's unit direction in space at a node, going with `like`.
    fn tangent(&self, node: &Node, like: DVec3) -> DVec3 {
        let t = self
            .s
            .normal(node.uv)
            .cross(self.other_normal(node.point))
            .normalize_or(like.normalize_or(DVec3::X));
        if t.dot(like) < 0.0 { -t } else { t }
    }

    /// A cubic through the nodes with the contour's direction at each, refined until it
    /// stays on both surfaces.
    fn fit(&self, nodes: &[Node]) -> Option<Curve3> {
        let mut nodes: Vec<Node> = nodes.to_vec();
        // Drop points that coincide (a branch end found twice).
        nodes.dedup_by(|b, a| a.point.distance(b.point) <= 10.0 * ON);
        if nodes.len() < 2 {
            return None;
        }
        for _ in 0..12 {
            let curve = self.hermite(&nodes);
            // Where the cubic is furthest from the surfaces: halfway between nodes.
            let (lo, _) = curve.domain();
            let mut at = lo;
            let mut insert: Vec<usize> = Vec::new();
            for i in 0..nodes.len() - 1 {
                let length = nodes[i].point.distance(nodes[i + 1].point);
                let p = curve.point(at + 0.5 * length);
                let off_s = (p - self.s.point(self.s.param(p))).length();
                let off_t = self.other.signed_distance(p).abs();
                if off_s.max(off_t) > FIT {
                    insert.push(i);
                }
                at += length;
            }
            if insert.is_empty() || nodes.len() > 4000 {
                return Some(Curve3::Nurbs(Arc::new(curve)));
            }
            for &i in insert.iter().rev() {
                let middle = 0.5 * (nodes[i].uv + nodes[i + 1].uv);
                if let Some(uv) = self.correct(middle) {
                    nodes.insert(
                        i + 1,
                        Node {
                            uv,
                            point: self.s.point(uv),
                        },
                    );
                }
            }
        }
        Some(Curve3::Nurbs(Arc::new(self.hermite(&nodes))))
    }

    /// The piecewise cubic through the nodes with the contour's directions: a Bézier
    /// piece per pair of nodes, parametrised by chord length.
    fn hermite(&self, nodes: &[Node]) -> NurbsCurve {
        let n = nodes.len();
        let mut knots = vec![0.0; 4];
        let mut points = vec![nodes[0].point];
        let mut at = 0.0;
        for i in 0..n - 1 {
            let chord = nodes[i + 1].point - nodes[i].point;
            let length = chord.length();
            let t0 = self.tangent(&nodes[i], chord);
            let t1 = self.tangent(&nodes[i + 1], chord);
            points.push(nodes[i].point + t0 * (length / 3.0));
            points.push(nodes[i + 1].point - t1 * (length / 3.0));
            points.push(nodes[i + 1].point);
            at += length;
            let repeats = if i + 2 == n { 4 } else { 3 };
            knots.extend(std::iter::repeat_n(at, repeats));
        }
        NurbsCurve::new(3, knots, points, None).expect("a clamped cubic with a piece per chord")
    }
}
