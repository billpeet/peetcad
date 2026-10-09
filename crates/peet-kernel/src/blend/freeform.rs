//! Rolling-ball fillets and set-back chamfers on edges the analytic tools can't sweep:
//! any edge curve (line, circle, ellipse, freeform) between any two kinds of face.
//!
//! **Geometry.** A fillet of radius `r` is the surface a ball of that radius leaves as it
//! rolls along the edge touching both faces. Its centre runs along the *spine*: the
//! points `r` from both surfaces, on the material side of a convex edge and on the open
//! side of a concave one. At a *station* (the plane through a point of the edge, square
//! to it) the ball's two contact points and its centre are found by Newton's method on
//! `A(uA, vA) + d·nA = B(uB, vB) + d·nB` with the centre in the station's plane; no
//! offset surface is ever made. The fillet's section there is the arc about the centre
//! between the contact points, which is exactly the circle the ball leaves on the
//! surface it sweeps. The sections, evenly spaced, are skinned into one NURBS surface
//! ([`NurbsSurface`]): `u` across the fillet from the first face to the second, exact
//! rational arcs; `v` along the edge, a cubic spline through the sections with their
//! true rates of change at its two ends. The skin is measured halfway between
//! stations, and the stations are spaced more closely until it leaves the true surface
//! by no more than [`SURFACE_FIT`], leaves neither face along a contact curve by more
//! than [`CURVE_FIT`], and is tangent there to within [`TANGENT_FIT`].
//!
//! A chamfer of distance `d` is the ruled surface between two *set-back curves*, one on
//! each face: at every station, the point of the face in the station's plane at the
//! straight-line distance `d` from the edge point. Between two flat faces that is the
//! analytic chamfer exactly.
//!
//! **Topology.** The result is built directly, without booleans: the edge is replaced
//! by the blend face, and its two faces are cut back to the contact curves (which are
//! the blend surface's own edges `u = 0` and `u = 1`, so they lie on it exactly).
//! Booleans can't do this: the fillet touches both faces along the contact curves, and
//! surfaces that touch can't be intersected by marching.
//!
//! - Edges that carry on from one another smoothly are blended as one *run*. Where a
//!   run passes from one face to the next (the faces meeting smoothly along an edge
//!   that ends on the run, or at a round face's seam), the blend is cut in two at the
//!   ball that touches that edge, which is cut back to the contact point.
//! - Where a run ends, at a vertex of three edges whose third face is flat and crosses
//!   the edge, the blend surface is carried on past the end (the edge and the faces
//!   carried on beyond their own ends as their last pieces continue) and cut off where
//!   it meets that face.
//! - A run that closes on itself is always made of at least two faces, so no face
//!   wraps all the way round.
//!
//! **Refused**, each with what to do instead: a corner where a blend here meets another
//! blend or where the faces beside a run meet at an angle (that needs a mitre between
//! freeform surfaces); a run that stops where its edge carries on; an end on a curved
//! face; a ball that doesn't fit the faces' curvature; a blend wide enough to reach
//! another edge of its faces; faces that become tangent somewhere along the edge.

use std::collections::HashMap;
use std::sync::Arc;

use peet_math::tolerance::LINEAR;
use peet_math::{DVec2, DVec3, DVec4, Frame};

use super::{Blend, BlendFace, Labelled, MIN_ANGLE};
use crate::geom::{Circle3, Curve3, Surface};
use crate::nurbs::{NurbsCurve, NurbsSurface};
use crate::topo::{CoedgeId, EdgeId, FaceId, VertexId};
use crate::{KernelError, Solid};

/// The skinned blend surface may leave the true one by this much between stations (mm).
pub const SURFACE_FIT: f64 = 0.5 * LINEAR;

/// The contact curves, and the curves a blend ends in, may leave the faces they lie on
/// by this much (mm): what the boolean operations hold their freeform curves to.
pub const CURVE_FIT: f64 = 0.2 * LINEAR;

/// A fillet's normal may differ from its neighbour's along a contact curve by this
/// (radians).
pub const TANGENT_FIT: f64 = 1e-5;

/// Blends of neighbouring edges of a run must agree on the ball where they meet to
/// within this (mm); more means the faces beside the run don't carry on smoothly.
const JOIN: f64 = 0.1 * LINEAR;

/// A station is solved when its equations are met to this (mm).
const SOLVED: f64 = 1e-10;

/// Edges whose directions at a shared vertex agree to this (as one minus the cosine)
/// carry on from one another.
const SAME_DIRECTION: f64 = 1e-9;

/// A fillet's arc must span at least this (radians), and at most half a turn less this:
/// outside that the faces are as good as tangent, or folded back on each other.
const MIN_SWEEP: f64 = 1e-2;

/// No blend face is skinned through more stations than this.
const MAX_STATIONS: usize = 1200;

/// A surface's point, derivatives and unit normal with its derivatives.
struct Jet {
    p: DVec3,
    pu: DVec3,
    pv: DVec3,
    n: DVec3,
    nu: DVec3,
    nv: DVec3,
}

/// The B-spline basis functions of `degree` that are not zero in the span starting at
/// `knots[span]`, differentiated `order` times, at `t`. Unlike the surface's own
/// evaluation `t` may lie outside the span: the span's polynomials simply carry on.
fn basis(knots: &[f64], span: usize, degree: usize, order: usize, t: f64) -> Vec<f64> {
    if order > degree {
        return vec![0.0; degree + 1];
    }
    if order == 0 {
        let mut n = vec![0.0; degree + 1];
        let (mut left, mut right) = (vec![0.0; degree + 1], vec![0.0; degree + 1]);
        n[0] = 1.0;
        for j in 1..=degree {
            left[j] = t - knots[span + 1 - j];
            right[j] = knots[span + j] - t;
            let mut saved = 0.0;
            for r in 0..j {
                let temp = n[r] / (right[r + 1] + left[j - r]);
                n[r] = saved + right[r + 1] * temp;
                saved = left[j - r] * temp;
            }
            n[j] = saved;
        }
        return n;
    }
    // From the functions one degree lower, differentiated once less.
    let lower = basis(knots, span, degree - 1, order - 1, t);
    let p = degree as f64;
    (0..=degree)
        .map(|j| {
            // Function `i = span − degree + j`; `lower[j − 1]` is function `i` of the
            // lower degree and `lower[j]` function `i + 1`.
            let i = span + j - degree;
            let mut d = 0.0;
            if j >= 1 {
                d += lower[j - 1] / (knots[i + degree] - knots[i]);
            }
            if j < degree {
                d -= lower[j] / (knots[i + degree + 1] - knots[i + 1]);
            }
            p * d
        })
        .collect()
}

/// A freeform surface's point and derivatives `[S, Su, Sv, Suu, Suv, Svv]` at `uv`,
/// which may be outside its rectangle: there the surface is carried on as the piece
/// next to that edge of the rectangle continues.
fn carried_on(s: &NurbsSurface, uv: DVec2) -> [DVec3; 6] {
    let (lo, hi) = s.domain();
    if uv.cmpge(lo).all() && uv.cmple(hi).all() {
        return s.evaluate(uv);
    }
    let (degree_u, degree_v) = s.degrees();
    let (knots_u, knots_v) = s.knots();
    let (count_u, count_v) = s.counts();
    // The span `t` is in, or the first or last one with a length.
    let span = |knots: &[f64], degree: usize, count: usize, t: f64| {
        let mut i = degree;
        while i + 1 < count && (knots[i + 1] <= t || knots[i + 1] <= knots[i]) {
            i += 1;
        }
        while i > degree && knots[i + 1] <= knots[i] {
            i -= 1;
        }
        i
    };
    let (su, sv) = (
        span(knots_u, degree_u, count_u, uv.x),
        span(knots_v, degree_v, count_v, uv.y),
    );
    let nu: Vec<Vec<f64>> = (0..3)
        .map(|k| basis(knots_u, su, degree_u, k, uv.x))
        .collect();
    let nv: Vec<Vec<f64>> = (0..3)
        .map(|k| basis(knots_v, sv, degree_v, k, uv.y))
        .collect();
    let points = s.control_points();
    let weights = s.weights();
    // Derivatives of the homogeneous surface: `a[i][j]` is ∂^(i+j) / ∂u^i ∂v^j.
    let mut a = [[DVec4::ZERO; 3]; 3];
    // The indices are the formula's: basis functions `i`, `j` of derivatives `du`, `dv`.
    #[allow(clippy::needless_range_loop)]
    for i in 0..=degree_u {
        for j in 0..=degree_v {
            let index = (su - degree_u + i) * count_v + sv - degree_v + j;
            let w = weights.map_or(1.0, |w| w[index]);
            let h = (points[index] * w).extend(w);
            for (du, row) in a.iter_mut().enumerate() {
                for (dv, sum) in row.iter_mut().enumerate().take(3 - du) {
                    *sum += h * (nu[du][i] * nv[dv][j]);
                }
            }
        }
    }
    let w = a[0][0].w;
    let p = a[0][0].truncate() / w;
    let pu = (a[1][0].truncate() - p * a[1][0].w) / w;
    let pv = (a[0][1].truncate() - p * a[0][1].w) / w;
    let puu = (a[2][0].truncate() - pu * (2.0 * a[1][0].w) - p * a[2][0].w) / w;
    let pvv = (a[0][2].truncate() - pv * (2.0 * a[0][1].w) - p * a[0][2].w) / w;
    let puv = (a[1][1].truncate() - pu * a[0][1].w - pv * a[1][0].w - p * a[1][1].w) / w;
    [p, pu, pv, puu, puv, pvv]
}

/// A freeform curve's point and first derivative at `t`, which may be outside its
/// range: there the curve is carried on as its first or last piece continues.
fn curve_carried_on(c: &NurbsCurve, t: f64) -> (DVec3, DVec3) {
    let (lo, hi) = c.domain();
    if t >= lo && t <= hi {
        let [p, d, _] = c.evaluate(t);
        return (p, d);
    }
    let (degree, knots, points) = (c.degree(), c.knots(), c.control_points());
    let mut span = if t < lo { degree } else { points.len() - 1 };
    while span > degree && knots[span + 1] <= knots[span] {
        span -= 1;
    }
    while span + 1 < points.len() && knots[span + 1] <= knots[span] {
        span += 1;
    }
    let (n, dn) = (
        basis(knots, span, degree, 0, t),
        basis(knots, span, degree, 1, t),
    );
    let (mut a, mut da) = (DVec4::ZERO, DVec4::ZERO);
    for j in 0..=degree {
        let index = span - degree + j;
        let w = c.weights().map_or(1.0, |w| w[index]);
        let h = (points[index] * w).extend(w);
        a += h * n[j];
        da += h * dn[j];
    }
    let p = a.truncate() / a.w;
    (p, (da.truncate() - p * da.w) / a.w)
}

/// Evaluates `surface` at `uv`. A freeform surface is carried on past the edges of its
/// rectangle ([`carried_on`]), so a blend can run a little beyond the end of a face.
fn jet(surface: &Surface, uv: DVec2) -> Jet {
    match surface {
        Surface::Plane(pl) => Jet {
            p: pl.from_plane_coords(uv),
            pu: pl.frame.x_axis(),
            pv: pl.frame.y_axis(),
            n: pl.normal(),
            nu: DVec3::ZERO,
            nv: DVec3::ZERO,
        },
        Surface::Nurbs(s) => {
            let [p, pu, pv, suu, suv, svv] = carried_on(s, uv);
            let cross = pu.cross(pv);
            let len = cross.length().max(f64::MIN_POSITIVE);
            let n = cross / len;
            let cu = suu.cross(pv) + pu.cross(suv);
            let cv = suv.cross(pv) + pu.cross(svv);
            Jet {
                p,
                pu,
                pv,
                n,
                nu: (cu - n * n.dot(cu)) / len,
                nv: (cv - n * n.dot(cv)) / len,
            }
        }
        _ => {
            let (Some(frame), Some(m)) = (surface.revolution_frame(), surface.meridian(uv.y))
            else {
                unreachable!("every other surface is a surface of revolution")
            };
            let (pu, pv) = surface.derivatives(uv);
            let (s, c) = uv.x.sin_cos();
            let len = m.dz.hypot(m.drho).max(f64::MIN_POSITIVE);
            let nv = match surface {
                Surface::Sphere(_) | Surface::Torus(_) => {
                    let (sv, cv) = uv.y.sin_cos();
                    frame.vector_to_world(DVec3::new(-c * sv, -s * sv, cv))
                }
                _ => DVec3::ZERO,
            };
            Jet {
                p: surface.point(uv),
                pu,
                pv,
                n: surface.normal(uv),
                nu: frame.vector_to_world(DVec3::new(-s * m.dz, c * m.dz, 0.0) / len),
                nv,
            }
        }
    }
}

/// The largest curvature of the surface at `j` towards the side `towards` (±1 along
/// the natural normal): how tightly it wraps around a ball sitting on that side.
fn curvature_towards(j: &Jet, towards: f64) -> f64 {
    let (e, f, g) = (j.pu.dot(j.pu), j.pu.dot(j.pv), j.pv.dot(j.pv));
    let det = e * g - f * f;
    if det <= f64::MIN_POSITIVE {
        return 0.0;
    }
    // The second fundamental form, from the normal's derivatives.
    let l = -j.nu.dot(j.pu);
    let m = -0.5 * (j.nu.dot(j.pv) + j.nv.dot(j.pu));
    let n = -j.nv.dot(j.pv);
    // The principal curvatures: eigenvalues of I⁻¹ II.
    let mean = 0.5 * (e * n - 2.0 * f * m + g * l) / det;
    let gauss = (l * n - m * m) / det;
    let root = (mean * mean - gauss).max(0.0).sqrt();
    // Curving towards +n is a negative normal curvature in this convention.
    let (k1, k2) = (-(mean + root), -(mean - root));
    (k1 * towards).max(k2 * towards)
}

/// The parameters of the point of `surface` (carried on as [`jet`] does) nearest `p`,
/// by Gauss–Newton from `start`, and how far it is.
fn nearest(surface: &Surface, p: DVec3, start: DVec2) -> (DVec2, f64) {
    let mut uv = start;
    for _ in 0..12 {
        let j = jet(surface, uv);
        let r = p - j.p;
        let (a, b, c) = (j.pu.dot(j.pu), j.pu.dot(j.pv), j.pv.dot(j.pv));
        let det = a * c - b * b;
        if det <= f64::MIN_POSITIVE {
            break;
        }
        let (gu, gv) = (r.dot(j.pu), r.dot(j.pv));
        let step = DVec2::new(c * gu - b * gv, a * gv - b * gu) / det;
        uv += step;
        if (j.pu * step.x + j.pv * step.y).length() <= 1e-13 {
            break;
        }
    }
    (uv, jet(surface, uv).p.distance(p))
}

/// Solves the `N` linear equations `m x = rhs` by elimination with pivoting.
fn solve<const N: usize>(mut m: [[f64; N]; N], mut rhs: [f64; N]) -> Option<[f64; N]> {
    for col in 0..N {
        let pivot = (col..N).max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs()))?;
        if m[pivot][col].abs() <= 1e-14 {
            return None;
        }
        m.swap(col, pivot);
        rhs.swap(col, pivot);
        for row in col + 1..N {
            let f = m[row][col] / m[col][col];
            let (above, below) = m.split_at_mut(row);
            for (x, p) in below[0][col..].iter_mut().zip(&above[col][col..]) {
                *x -= f * p;
            }
            rhs[row] -= f * rhs[col];
        }
    }
    let mut x = [0.0; N];
    for row in (0..N).rev() {
        let mut v = rhs[row];
        for k in row + 1..N {
            v -= m[row][k] * x[k];
        }
        x[row] = v / m[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// The parameter of the point of an edge's curve nearest `p`, taken in the turn of a
/// closed curve that the range `t0..t1` is in.
fn param_on(curve: &Curve3, t0: f64, t1: f64, p: DVec3) -> f64 {
    let t = curve.param(p);
    match curve.period() {
        Some(period) => t + ((0.5 * (t0 + t1) - t) / period).round() * period,
        None => t,
    }
}

/// An edge as the blend runs along it: `s` runs from 0 at the end the run comes in by
/// to [`Path::length`] at the other, in the curve's own parameter units.
#[derive(Clone)]
struct Path {
    curve: Curve3,
    t0: f64,
    t1: f64,
    forward: bool,
}

impl Path {
    fn length(&self) -> f64 {
        self.t1 - self.t0
    }

    /// The point at `s` and the unit direction of the run there. Past its ends the
    /// curve carries on as it is (a freeform one as its end piece continues).
    fn at(&self, s: f64) -> (DVec3, DVec3) {
        let t = if self.forward {
            self.t0 + s
        } else {
            self.t1 - s
        };
        let (p, d) = match &self.curve {
            Curve3::Nurbs(c) => curve_carried_on(c, t),
            other => (other.point(t), other.derivative(t)),
        };
        let d = if self.forward { d } else { -d };
        (p, d.normalize_or(DVec3::X))
    }

    /// Millimetres per unit of `s` at `s`.
    fn speed(&self, s: f64) -> f64 {
        let t = if self.forward {
            self.t0 + s
        } else {
            self.t1 - s
        };
        match &self.curve {
            Curve3::Nurbs(c) => curve_carried_on(c, t).1,
            other => other.derivative(t),
        }
        .length()
        .max(1e-12)
    }
}

/// One of an edge's two faces.
#[derive(Clone)]
struct Side {
    surface: Surface,
    /// +1 if the face's outward normal is its surface's natural normal, else −1.
    out: f64,
    /// How far along the natural normal a fillet's centre is from the surface.
    offset: f64,
}

/// An edge of a run with everything a station needs.
#[derive(Clone)]
struct EdgeCtx {
    path: Path,
    /// The face on the left of the run seen from outside, and the one on its right.
    a: Side,
    b: Side,
    shape: Blend,
}

/// Why a station couldn't be solved.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fail {
    /// Newton's method found no ball (or no set-back point) there.
    NoFit,
    /// The faces are tangent there, or folded back on each other.
    Tangent,
    /// A face curves more tightly than the ball: this is the tightest radius.
    Curvature(f64),
}

/// The blend's section at one place along an edge.
#[derive(Clone, Debug)]
struct Station {
    s: f64,
    /// The edge point and the run's direction there.
    e: DVec3,
    tan: DVec3,
    /// Parameters and points of the contact (or set-back) points on the two faces.
    uva: DVec2,
    uvb: DVec2,
    pa: DVec3,
    pb: DVec3,
    /// The ball's centre (a fillet), or the middle of the section (a chamfer).
    c: DVec3,
}

impl Station {
    /// How far the blend reaches from the edge here.
    fn reach(&self) -> f64 {
        self.pa.distance(self.e).max(self.pb.distance(self.e))
    }

    /// The angle a fillet's arc spans.
    fn sweep(&self) -> f64 {
        (self.pa - self.c).angle_between(self.pb - self.c)
    }

    /// The section's point at the fraction `f` of the way from the first face.
    fn across(&self, shape: Blend, f: f64) -> DVec3 {
        match shape {
            Blend::Chamfer { .. } => self.pa.lerp(self.pb, f),
            Blend::Fillet { .. } => {
                let a = self.pa - self.c;
                let b = self.pb - self.c;
                let Some(axis) = a.cross(b).try_normalize() else {
                    return self.pa;
                };
                let (sin, cos) = (self.sweep() * f).sin_cos();
                self.c + a * cos + axis.cross(a) * sin
            }
        }
    }
}

impl EdgeCtx {
    fn size(&self) -> f64 {
        match self.shape {
            Blend::Fillet { radius } => radius,
            Blend::Chamfer { distance } => distance,
        }
    }

    /// The directions from the edge point `e` along each face, square to the run, and
    /// the parameters of `e` on each face.
    fn corner(&self, e: DVec3, tan: DVec3) -> (DVec3, DVec3, DVec2, DVec2) {
        let (uva, uvb) = (self.a.surface.param(e), self.b.surface.param(e));
        let na = self.a.surface.normal(uva) * self.a.out;
        let nb = self.b.surface.normal(uvb) * self.b.out;
        // A face lies to the left of its own direction along the edge, seen from outside.
        (
            na.cross(tan).normalize_or(DVec3::X),
            nb.cross(-tan).normalize_or(DVec3::X),
            uva,
            uvb,
        )
    }

    /// Where to start Newton's method at a station, from the corner the faces make at
    /// the edge point.
    fn guess(&self, e: DVec3, tan: DVec3) -> Result<(DVec2, DVec2), Fail> {
        let (along_a, along_b, uva, uvb) = self.corner(e, tan);
        let angle = along_a.dot(along_b).clamp(-1.0, 1.0).acos();
        if !(MIN_SWEEP..=std::f64::consts::PI - MIN_SWEEP).contains(&angle) {
            return Err(Fail::Tangent);
        }
        let reach = match self.shape {
            Blend::Fillet { radius } => radius / (0.5 * angle).tan(),
            Blend::Chamfer { distance } => distance,
        };
        Ok((
            nearest(&self.a.surface, e + along_a * reach, uva).0,
            nearest(&self.b.surface, e + along_b * reach, uvb).0,
        ))
    }

    /// Newton's method for a fillet's station: the two offset points coincide, in the
    /// plane through `e` square to `tan`.
    fn solve_fillet(
        &self,
        e: DVec3,
        tan: DVec3,
        mut uva: DVec2,
        mut uvb: DVec2,
    ) -> Option<(DVec2, DVec2)> {
        let radius = self.size();
        let solved = SOLVED + 1e-13 * e.abs().max_element();
        for _ in 0..60 {
            let (ja, jb) = (jet(&self.a.surface, uva), jet(&self.b.surface, uvb));
            let oa = ja.p + ja.n * self.a.offset;
            let ob = jb.p + jb.n * self.b.offset;
            let gap = oa - ob;
            let plane = (oa - e).dot(tan);
            if gap.length().max(plane.abs()) <= solved {
                return Some((uva, uvb));
            }
            let au = ja.pu + ja.nu * self.a.offset;
            let av = ja.pv + ja.nv * self.a.offset;
            let bu = jb.pu + jb.nu * self.b.offset;
            let bv = jb.pv + jb.nv * self.b.offset;
            let x = solve(
                [
                    [au.x, av.x, -bu.x, -bv.x],
                    [au.y, av.y, -bu.y, -bv.y],
                    [au.z, av.z, -bu.z, -bv.z],
                    [au.dot(tan), av.dot(tan), 0.0, 0.0],
                ],
                [-gap.x, -gap.y, -gap.z, -plane],
            )?;
            // No step moves a contact point further than the ball's radius.
            let moved = (ja.pu * x[0] + ja.pv * x[1])
                .length()
                .max((jb.pu * x[2] + jb.pv * x[3]).length());
            let k = (radius / moved.max(f64::MIN_POSITIVE)).min(1.0);
            uva += DVec2::new(x[0], x[1]) * k;
            uvb += DVec2::new(x[2], x[3]) * k;
        }
        None
    }

    /// Newton's method for a chamfer's set-back point on one face: in the plane through
    /// `e` square to `tan`, at the distance `d` from `e`.
    fn solve_setback(
        surface: &Surface,
        e: DVec3,
        tan: DVec3,
        d: f64,
        mut uv: DVec2,
    ) -> Option<DVec2> {
        let solved = SOLVED + 1e-13 * e.abs().max_element();
        for _ in 0..60 {
            let j = jet(surface, uv);
            let w = j.p - e;
            let plane = w.dot(tan);
            let far = (w.length_squared() - d * d) / (2.0 * d);
            if plane.abs().max(far.abs()) <= solved {
                return Some(uv);
            }
            let x = solve(
                [
                    [j.pu.dot(tan), j.pv.dot(tan)],
                    [w.dot(j.pu) / d, w.dot(j.pv) / d],
                ],
                [-plane, -far],
            )?;
            let moved = (j.pu * x[0] + j.pv * x[1]).length();
            let k = (d / moved.max(f64::MIN_POSITIVE)).min(1.0);
            uv += DVec2::new(x[0], x[1]) * k;
        }
        None
    }

    /// The station at `s`, starting Newton's method from `near` (the parameters of a
    /// station close by) or, failing that, from the corner at the edge point.
    fn station(&self, s: f64, near: Option<(DVec2, DVec2)>) -> Result<Station, Fail> {
        let (e, tan) = self.path.at(s);
        let run = |uva: DVec2, uvb: DVec2| -> Option<(DVec2, DVec2)> {
            match self.shape {
                Blend::Fillet { .. } => self.solve_fillet(e, tan, uva, uvb),
                Blend::Chamfer { distance } => Some((
                    Self::solve_setback(&self.a.surface, e, tan, distance, uva)?,
                    Self::solve_setback(&self.b.surface, e, tan, distance, uvb)?,
                )),
            }
        };
        let found = match near.and_then(|(a, b)| run(a, b)) {
            Some(found) => found,
            None => {
                let (a, b) = self.guess(e, tan)?;
                run(a, b).ok_or(Fail::NoFit)?
            }
        };
        let (uva, uvb) = found;
        let (ja, jb) = (jet(&self.a.surface, uva), jet(&self.b.surface, uvb));
        let size = self.size();
        let c = match self.shape {
            Blend::Fillet { .. } => ja.p + ja.n * self.a.offset,
            Blend::Chamfer { .. } => 0.5 * (ja.p + jb.p),
        };
        let st = Station {
            s,
            e,
            tan,
            uva,
            uvb,
            pa: ja.p,
            pb: jb.p,
            c,
        };
        // A solution far from the edge is some other place the surfaces come close.
        if st.reach() > 50.0 * size || !st.reach().is_finite() {
            return Err(Fail::NoFit);
        }
        match self.shape {
            Blend::Fillet { radius } => {
                for (j, side) in [(&ja, &self.a), (&jb, &self.b)] {
                    let k = curvature_towards(j, side.offset.signum());
                    if k * radius >= 1.0 - 1e-6 {
                        return Err(Fail::Curvature(1.0 / k));
                    }
                }
                let sweep = st.sweep();
                if !(MIN_SWEEP..=std::f64::consts::PI - MIN_SWEEP).contains(&sweep) {
                    return Err(Fail::Tangent);
                }
            }
            Blend::Chamfer { .. } => {
                let angle = (st.pa - e).angle_between(st.pb - e);
                if !(MIN_SWEEP..=std::f64::consts::PI - MIN_SWEEP).contains(&angle) {
                    return Err(Fail::Tangent);
                }
            }
        }
        Ok(st)
    }

    /// What to tell the user when a station can't be solved.
    fn refuse(&self, fail: Fail) -> KernelError {
        let (what, size) = match self.shape {
            Blend::Fillet { radius } => ("fillet", format!("radius of {radius} mm")),
            Blend::Chamfer { distance } => ("chamfer", format!("distance of {distance} mm")),
        };
        match fail {
            Fail::NoFit => KernelError::InvalidInput(format!(
                "the {what} doesn't fit along the edge: its faces bend away too sharply, or \
                 end too soon, for a {size}. Try a smaller one"
            )),
            Fail::Tangent => KernelError::Unsupported(format!(
                "the two faces meet smoothly at some point of the edge (they have the same \
                 slope there), so the {what} would shrink to nothing. Only an edge that is a \
                 corner all along its length can take a {what}"
            )),
            Fail::Curvature(tightest) => KernelError::InvalidInput(format!(
                "the fillet's radius is bigger than the radius a face next to the edge curves \
                 with (about {tightest:.3} mm at its tightest), so the fillet would fold over \
                 itself. Use a smaller radius"
            )),
        }
    }
}

/// A place where one blend face ends and the next begins: the section they share.
struct Junction {
    pa: DVec3,
    pb: DVec3,
    c: DVec3,
    /// Edges that end on the run here and are cut back to the contact points: on the
    /// first faces' side and on the second faces' side.
    rails: [Option<Trim>; 2],
}

/// An edge to be cut back at one of its ends.
#[derive(Clone, Copy, Debug)]
struct Trim {
    edge: EdgeId,
    /// The end cut back is the edge's start.
    at_start: bool,
    /// The parameter it now starts or ends at.
    t: f64,
}

/// The end of a run on a flat face that crosses it.
#[derive(Clone, Debug)]
struct OpenEnd {
    vertex: VertexId,
    cap: FaceId,
    /// A point of the cap's plane and its outward normal.
    origin: DVec3,
    normal: DVec3,
    /// The cap's edges with the run's first and second face.
    edge_a: EdgeId,
    edge_b: EdgeId,
    /// The use of an edge in the cap's loop that arrives at the vertex.
    arriving: (EdgeId, bool),
}

/// How a blend face ends.
enum PieceEnd {
    Junction(usize),
    /// Cut off by a flat face: the curve it ends in (from the first face's side to the
    /// second's, over `[0, 1]`) and the blend surface's `v` at its two ends.
    Cap {
        open: OpenEnd,
        curve: Box<NurbsCurve>,
        va: f64,
        vb: f64,
    },
}

/// One face of a blend.
struct Piece {
    /// Which edge of the run it is on.
    slot: usize,
    surface: Arc<NurbsSurface>,
    start: PieceEnd,
    end: PieceEnd,
    stations: Vec<Station>,
}

/// A selected edge in a run.
#[derive(Clone, Copy, Debug)]
struct RunEdge {
    edge: EdgeId,
    /// Its place in the caller's list.
    index: usize,
    /// The run follows the edge from its start to its end.
    forward: bool,
    /// The coedge that goes the run's way (on the first face) and the other one.
    ca: CoedgeId,
    cb: CoedgeId,
}

/// Edges blended as one, in order.
struct Run {
    edges: Vec<RunEdge>,
    closed: bool,
}

/// For each of `edges`, at its start and at its end: the edge that carries on from it
/// there (its place in `edges`, and whether that is the other edge's end).
type Links = Vec<[Option<(usize, bool)>; 2]>;

/// The direction in which an edge leaves one of its ends.
fn leaving(solid: &Solid, edge: EdgeId, end: bool) -> DVec3 {
    let e = solid.edge(edge);
    if end {
        -e.curve.tangent(e.t1)
    } else {
        e.curve.tangent(e.t0)
    }
}

/// Which of `edges` carry on from one another: at a vertex where exactly two of their
/// ends meet, in opposite directions.
fn links(solid: &Solid, edges: &[EdgeId]) -> Links {
    let mut at: HashMap<VertexId, Vec<(usize, bool)>> = HashMap::new();
    for (k, &id) in edges.iter().enumerate() {
        let e = solid.edge(id);
        at.entry(e.start).or_default().push((k, false));
        at.entry(e.end).or_default().push((k, true));
    }
    let mut out: Links = vec![[None, None]; edges.len()];
    for ends in at.values() {
        if let [(i, i_end), (j, j_end)] = ends[..] {
            let d = leaving(solid, edges[i], i_end).dot(leaving(solid, edges[j], j_end));
            if d < -(1.0 - SAME_DIRECTION) {
                out[i][usize::from(i_end)] = Some((j, j_end));
                out[j][usize::from(j_end)] = Some((i, i_end));
            }
        }
    }
    out
}

/// Groups `edges` (which are distinct) into those that carry on from one another: each
/// group lists places in `edges`.
pub(super) fn chains(solid: &Solid, edges: &[EdgeId]) -> Vec<Vec<usize>> {
    let links = links(solid, edges);
    let mut group: Vec<Option<usize>> = vec![None; edges.len()];
    let mut out: Vec<Vec<usize>> = Vec::new();
    for first in 0..edges.len() {
        if group[first].is_some() {
            continue;
        }
        let g = out.len();
        let mut members = Vec::new();
        let mut stack = vec![first];
        while let Some(k) = stack.pop() {
            if group[k].is_some() {
                continue;
            }
            group[k] = Some(g);
            members.push(k);
            stack.extend(links[k].iter().flatten().map(|(other, _)| *other));
        }
        members.sort_unstable();
        out.push(members);
    }
    out
}

/// Blends `edges` (each with its place in the caller's list), which are whole groups
/// from [`chains`], by rolling a ball along them. `others` are the edges of the same
/// call that are blended analytically afterwards.
pub(super) fn blend(
    solid: &Solid,
    edges: &[(EdgeId, usize)],
    others: &[EdgeId],
    shape: Blend,
) -> Result<Labelled, KernelError> {
    let what = match shape {
        Blend::Fillet { .. } => "fillet",
        Blend::Chamfer { .. } => "chamfer",
    };
    let ids: Vec<EdgeId> = edges.iter().map(|(e, _)| *e).collect();
    let links = links(solid, &ids);

    // Corners: a vertex where one of these edges meets another blended edge that does
    // not carry on from it.
    let mut ends_at: HashMap<VertexId, usize> = HashMap::new();
    for &id in ids.iter().chain(others) {
        let e = solid.edge(id);
        *ends_at.entry(e.start).or_default() += 1;
        *ends_at.entry(e.end).or_default() += 1;
    }
    for (k, &id) in ids.iter().enumerate() {
        let e = solid.edge(id);
        for (end, v) in [(false, e.start), (true, e.end)] {
            if ends_at[&v] > 1 && links[k][usize::from(end)].is_none() {
                return Err(KernelError::Unsupported(format!(
                    "two of the edges meet at a corner, and at least one of them is freeform \
                     or lies on a curved face: their {what}s would have to be mitred into \
                     each other, which is only done between flat faces. Blend them in \
                     separate features, or leave one of them out"
                )));
            }
        }
    }

    // Runs, each in order.
    let mut used = vec![false; ids.len()];
    let mut runs = Vec::new();
    for first in 0..ids.len() {
        if used[first] {
            continue;
        }
        // Back to where the run starts (or all the way round).
        let (mut k, mut forward) = (first, true);
        let mut closed = false;
        while let Some((prev, prev_end)) = links[k][usize::from(!forward)] {
            (k, forward) = (prev, prev_end);
            if k == first {
                closed = true;
                break;
            }
        }
        let mut run = Vec::new();
        loop {
            used[k] = true;
            run.push(run_edge(solid, edges[k], forward)?);
            match links[k][usize::from(forward)] {
                Some((next, next_end)) if !used[next] => (k, forward) = (next, !next_end),
                _ => break,
            }
        }
        runs.push(Run { edges: run, closed });
    }

    let mut draft = Draft::new(solid);
    let mut planned = Vec::with_capacity(runs.len());
    for run in &runs {
        planned.push(plan(solid, run, shape)?);
    }
    // Every cut, before anything is checked against the edges that are left.
    let mut trims: Vec<Trim> = Vec::new();
    for p in &planned {
        trims.extend(p.junctions.iter().flat_map(|j| j.rails.iter().flatten()));
        trims.extend(p.cap_trims.iter().copied());
    }
    for (i, a) in trims.iter().enumerate() {
        let e = solid.edge(a.edge);
        if !(a.t > e.t0 && a.t < e.t1)
            || trims[..i]
                .iter()
                .any(|b| b.edge == a.edge && (b.at_start == a.at_start || crossed(a, b)))
        {
            return Err(too_wide(what));
        }
    }
    for (run, p) in runs.iter().zip(&planned) {
        swallowed(solid, run, p, &ids, &trims, what)?;
    }
    for (run, p) in runs.iter().zip(planned) {
        draft.apply(solid, run, p, shape);
    }
    let out = draft.build();
    if let Err(problems) = crate::validate::validate(&out.solid) {
        let list: Vec<&str> = problems.iter().map(|p| p.message.as_str()).collect();
        return Err(KernelError::InvalidResult(format!(
            "The {what} produced an invalid solid (it may be too big for the faces next to \
             the edge; try a smaller one): {}.",
            list.join("; ")
        )));
    }
    Ok(out)
}

/// Whether two cuts of one edge, at its two ends, pass each other.
fn crossed(a: &Trim, b: &Trim) -> bool {
    let (start, end) = if a.at_start { (a, b) } else { (b, a) };
    start.t >= end.t
}

fn too_wide(what: &str) -> KernelError {
    KernelError::InvalidInput(format!(
        "the {what} is too big: it would use up a whole edge next to the one it is on. Try a \
         smaller one"
    ))
}

/// An edge as part of a run.
fn run_edge(
    solid: &Solid,
    (edge, index): (EdgeId, usize),
    forward: bool,
) -> Result<RunEdge, KernelError> {
    let e = solid.edge(edge);
    let [c0, c1] = e.coedges[..] else {
        return Err(KernelError::InvalidInput(
            "the edge is not between two faces".to_owned(),
        ));
    };
    if solid.coedge_face(c0) == solid.coedge_face(c1) {
        return Err(KernelError::Unsupported(
            "the edge is a seam of one face, not a corner between two; pick a sharp edge"
                .to_owned(),
        ));
    }
    // The coedge that runs the way the run does is the one on its left-hand face.
    let (ca, cb) = if solid.coedge(c0).reversed != forward {
        (c0, c1)
    } else {
        (c1, c0)
    };
    Ok(RunEdge {
        edge,
        index,
        forward,
        ca,
        cb,
    })
}

/// Everything worked out for a run before the solid is touched.
struct Planned {
    junctions: Vec<Junction>,
    pieces: Vec<Piece>,
    /// The cuts of the end faces' edges.
    cap_trims: Vec<Trim>,
}

/// How many edge ends meet at a vertex.
fn degree(solid: &Solid, v: VertexId) -> usize {
    solid
        .edges
        .iter()
        .map(|e| usize::from(e.start == v) + usize::from(e.end == v))
        .sum()
}

/// What is known of a junction before its section is solved.
struct Joint {
    /// The edges that end on the run here: beside the first faces, beside the second.
    rail_a: Option<EdgeId>,
    rail_b: Option<EdgeId>,
    vertex: VertexId,
}

/// The rail ending at the vertex between two edges of a run on one side, if any:
/// `leaves` is the coedge after which a rail would leave the vertex and `arrives` the
/// one before which it would come back.
fn rail_between(
    solid: &Solid,
    leaves: CoedgeId,
    arrives: CoedgeId,
) -> Result<Option<EdgeId>, KernelError> {
    let next = solid.coedge(leaves).next;
    if next == arrives {
        return Ok(None);
    }
    let back = solid.twin(next);
    if back.map(|t| solid.coedge(t).next) != Some(arrives) {
        return Err(KernelError::Unsupported(
            "several edges meet the run of edges at one point; a blend can pass a point \
             where one more edge ends on each side, between faces that meet smoothly"
                .to_owned(),
        ));
    }
    Ok(Some(solid.coedge(next).edge))
}

fn plan(solid: &Solid, run: &Run, shape: Blend) -> Result<Planned, KernelError> {
    let what = match shape {
        Blend::Fillet { .. } => "fillet",
        Blend::Chamfer { .. } => "chamfer",
    };
    let size = match shape {
        Blend::Fillet { radius } => radius,
        Blend::Chamfer { distance } => distance,
    };
    // The edges' faces, and whether the corner is convex.
    let mut ctxs: Vec<EdgeCtx> = Vec::with_capacity(run.edges.len());
    let mut convex: Option<bool> = None;
    for re in &run.edges {
        let e = solid.edge(re.edge);
        let side = |c: CoedgeId| {
            let f = solid.face(solid.coedge_face(c));
            Side {
                surface: f.surface.clone(),
                out: if f.reversed { -1.0 } else { 1.0 },
                offset: 0.0,
            }
        };
        let mut ctx = EdgeCtx {
            path: Path {
                curve: e.curve.clone(),
                t0: e.t0,
                t1: e.t1,
                forward: re.forward,
            },
            a: side(re.ca),
            b: side(re.cb),
            shape,
        };
        // The corner in the middle of the edge.
        let (mid, tan) = ctx.path.at(0.5 * ctx.path.length());
        let (along_a, along_b, _, uvb) = ctx.corner(mid, tan);
        let angle = along_a.dot(along_b).clamp(-1.0, 1.0).acos();
        if !(MIN_ANGLE..=std::f64::consts::PI - MIN_ANGLE).contains(&angle) {
            return Err(KernelError::InvalidInput(
                "the faces meet smoothly along the edge: there is no corner to blend".to_owned(),
            ));
        }
        let nb = ctx.b.surface.normal(uvb) * ctx.b.out;
        let here = nb.dot(along_a) < 0.0;
        if *convex.get_or_insert(here) != here {
            return Err(KernelError::Unsupported(format!(
                "the run of edges is an outside corner in one place and an inside corner in \
                 another; {what} the two parts separately"
            )));
        }
        // The ball is inside the material at a convex edge, outside at a concave one.
        let towards = if here { -size } else { size };
        ctx.a.offset = towards * ctx.a.out;
        ctx.b.offset = towards * ctx.b.out;
        ctxs.push(ctx);
    }

    // Where one edge of the run hands over to the next.
    let count = run.edges.len();
    let joints_between = if run.closed { count } else { count - 1 };
    let mut joints = Vec::with_capacity(joints_between);
    for i in 0..joints_between {
        let (here, next) = (&run.edges[i], &run.edges[(i + 1) % count]);
        let vertex = solid.coedge_end(here.ca);
        // On the first faces a rail leaves after this edge's coedge and comes back
        // before the next edge's; on the second faces the coedges run the other way.
        let rail_a = rail_between(solid, here.ca, next.ca)?;
        let rail_b = rail_between(solid, next.cb, here.cb)?;
        let rails = usize::from(rail_a.is_some()) + usize::from(rail_b.is_some());
        if degree(solid, vertex) != 2 + rails {
            return Err(KernelError::Unsupported(
                "several edges meet the run of edges at one point; a blend can pass a point \
                 where one more edge ends on each side, between faces that meet smoothly"
                    .to_owned(),
            ));
        }
        joints.push(Joint {
            rail_a,
            rail_b,
            vertex,
        });
    }

    // The section at each junction, from the edge before it and from the edge after.
    let mut junctions: Vec<Junction> = Vec::with_capacity(joints.len());
    // Per edge: the stations it starts and ends at, where those are junctions.
    let mut bounds: Vec<[Option<(Station, usize)>; 2]> = vec![[None, None]; count];
    for (i, joint) in joints.iter().enumerate() {
        let next = (i + 1) % count;
        let before = junction_station(solid, &ctxs[i], true, joint)?;
        let mut after = junction_station(solid, &ctxs[next], false, joint)?;
        let apart = before
            .c
            .distance(after.c)
            .max(before.pa.distance(after.pa))
            .max(before.pb.distance(after.pb));
        if apart > JOIN {
            return Err(KernelError::Unsupported(format!(
                "the faces beside the run of edges meet at an angle where the {what} passes \
                 from one to the next (the two parts of the {what} would be {apart:.2e} mm \
                 apart there). A blend carries on only across faces that meet smoothly; \
                 blend the edges one feature at a time, or blend the edge between those \
                 faces first"
            )));
        }
        (after.pa, after.pb, after.c) = (before.pa, before.pb, before.c);
        let trim = |rail: Option<EdgeId>, point: DVec3| -> Result<Option<Trim>, KernelError> {
            let Some(edge) = rail else { return Ok(None) };
            let e = solid.edge(edge);
            if e.is_closed() {
                return Err(too_wide(what));
            }
            Ok(Some(Trim {
                edge,
                at_start: e.start == joint.vertex,
                t: param_on(&e.curve, e.t0, e.t1, point),
            }))
        };
        junctions.push(Junction {
            pa: before.pa,
            pb: before.pb,
            c: before.c,
            rails: [
                trim(joint.rail_a, before.pa)?,
                trim(joint.rail_b, before.pb)?,
            ],
        });
        bounds[i][1] = Some((before, i));
        bounds[next][0] = Some((after, i));
    }

    // The ends of an open run.
    let mut opens: [Option<OpenEnd>; 2] = [None, None];
    if !run.closed {
        opens[0] = Some(open_end(solid, &run.edges[0], false, what)?);
        opens[1] = Some(open_end(solid, &run.edges[count - 1], true, what)?);
    }

    let mut pieces = Vec::new();
    let mut cap_trims = Vec::new();
    for (slot, ctx) in ctxs.iter().enumerate() {
        let [start, end] = bounds[slot].clone();
        // An edge that closes on itself is blended in two halves, so no face goes all
        // the way round.
        let whole_turn = matches!((&start, &end), (Some((_, a)), Some((_, b))) if a == b);
        let mut parts: Vec<[Bound; 2]> = Vec::new();
        let bound = |fixed: Option<(Station, usize)>, open: &Option<OpenEnd>| match fixed {
            Some((st, j)) => Bound::Fixed(st, j),
            None => Bound::Open(open.clone().expect("an open run has both its ends")),
        };
        let (first, last) = (bound(start, &opens[0]), bound(end, &opens[1]));
        if whole_turn {
            let (Bound::Fixed(a, _), Bound::Fixed(b, _)) = (&first, &last) else {
                unreachable!("a whole turn is between two junctions")
            };
            let middle = ctx
                .station(0.5 * (a.s + b.s), None)
                .map_err(|f| ctx.refuse(f))?;
            let j = junctions.len();
            junctions.push(Junction {
                pa: middle.pa,
                pb: middle.pb,
                c: middle.c,
                rails: [None, None],
            });
            parts.push([first, Bound::Fixed(middle.clone(), j)]);
            parts.push([Bound::Fixed(middle, j), last]);
        } else {
            parts.push([first, last]);
        }
        for [from, to] in parts {
            let piece = build_piece(ctx, slot, from, to, what)?;
            for end in [&piece.start, &piece.end] {
                if let PieceEnd::Cap { open, curve, .. } = end {
                    let (lo, hi) = curve.domain();
                    for (edge, point) in [
                        (open.edge_a, curve.point(lo)),
                        (open.edge_b, curve.point(hi)),
                    ] {
                        let e = solid.edge(edge);
                        let t = param_on(&e.curve, e.t0, e.t1, point);
                        if e.is_closed() || e.curve.point(t).distance(point) > 20.0 * LINEAR {
                            return Err(KernelError::Unsupported(format!(
                                "the {what} can't be finished where the edge ends: it doesn't \
                                 come out on the edges of the face there"
                            )));
                        }
                        cap_trims.push(Trim {
                            edge,
                            at_start: e.start == open.vertex,
                            t,
                        });
                    }
                }
            }
            pieces.push(piece);
        }
    }
    Ok(Planned {
        junctions,
        pieces,
        cap_trims,
    })
}

/// How a piece is bounded at one end, before it is built.
enum Bound {
    /// At a junction: its station on this edge and its number.
    Fixed(Station, usize),
    Open(OpenEnd),
}

/// What ends a run at its first (`at_end` false) or last edge.
fn open_end(solid: &Solid, re: &RunEdge, at_end: bool, what: &str) -> Result<OpenEnd, KernelError> {
    let carry_on = || {
        KernelError::Unsupported(format!(
            "the {what} would stop where its edge carries on, or where more than three edges \
             meet. Pick the edges that continue it as well (a chain of edges that run on \
             smoothly is blended as one), so that it ends on a face that crosses the edge"
        ))
    };
    // At the end the first face's loop leaves the vertex after the run's coedge, and the
    // second face's arrives before its coedge; at the start it is the other way round.
    let (vertex, on_a, on_b) = if at_end {
        (
            solid.coedge_end(re.ca),
            solid.coedge(re.ca).next,
            solid.coedge(re.cb).prev,
        )
    } else {
        (
            solid.coedge_start(re.ca),
            solid.coedge(re.ca).prev,
            solid.coedge(re.cb).next,
        )
    };
    let (Some(cap_a), Some(cap_b)) = (solid.twin(on_a), solid.twin(on_b)) else {
        return Err(carry_on());
    };
    let (edge_a, edge_b) = (solid.coedge(on_a).edge, solid.coedge(on_b).edge);
    let cap = solid.coedge_face(cap_a);
    if degree(solid, vertex) != 3
        || cap != solid.coedge_face(cap_b)
        || edge_a == edge_b
        || edge_a == re.edge
        || edge_b == re.edge
        || cap == solid.coedge_face(re.ca)
        || cap == solid.coedge_face(re.cb)
    {
        return Err(carry_on());
    }
    let e = solid.edge(re.edge);
    let (point, out) = if at_end == re.forward {
        (e.curve.point(e.t1), e.curve.tangent(e.t1))
    } else {
        (e.curve.point(e.t0), -e.curve.tangent(e.t0))
    };
    let face = solid.face(cap);
    // The face must cut across the edge, not lie beside it.
    if solid.face_normal_at(cap, point).dot(out) < 0.05 {
        return Err(carry_on());
    }
    let Surface::Plane(plane) = &face.surface else {
        return Err(KernelError::Unsupported(format!(
            "the edge ends on a curved face that cuts across it; a {what} on a freeform \
             edge, or on an edge of a curved face, can end on a flat face"
        )));
    };
    let normal = if face.reversed {
        -plane.normal()
    } else {
        plane.normal()
    };
    // In the cap's loop, the coedge that arrives at the vertex.
    let arriving = if at_end { cap_a } else { cap_b };
    let arriving = solid.coedge(arriving);
    Ok(OpenEnd {
        vertex,
        cap,
        origin: point,
        normal,
        edge_a,
        edge_b,
        arriving: (arriving.edge, arriving.reversed),
    })
}

/// The station where the blend on one edge of a run meets the next: at the edge's end
/// (`at_end`) or start if no other edge ends there, else where the contact point on
/// that edge's side reaches it.
fn junction_station(
    solid: &Solid,
    ctx: &EdgeCtx,
    at_end: bool,
    joint: &Joint,
) -> Result<Station, KernelError> {
    let s0 = if at_end { ctx.path.length() } else { 0.0 };
    let first = ctx.station(s0, None).map_err(|f| ctx.refuse(f))?;
    // How far the contact point on one side is from a rail, across it.
    let off = |st: &Station, rail: EdgeId, on_a: bool| -> f64 {
        let e = solid.edge(rail);
        let (p, surface, uv) = if on_a {
            (st.pa, &ctx.a.surface, st.uva)
        } else {
            (st.pb, &ctx.b.surface, st.uvb)
        };
        let t = param_on(&e.curve, e.t0, e.t1, p);
        let across = jet(surface, uv)
            .n
            .cross(e.curve.tangent(t))
            .normalize_or(DVec3::X);
        (p - e.curve.point(t)).dot(across)
    };
    let (rail, on_a) = match (joint.rail_b, joint.rail_a) {
        (Some(rail), _) => (rail, false),
        (None, Some(rail)) => (rail, true),
        (None, None) => return Ok(first),
    };
    // The secant method on the distance, starting a little way inside the edge.
    let limit = 20.0 * first.reach() / ctx.path.speed(s0);
    let inside = if at_end { -1.0 } else { 1.0 };
    let step = (0.05 * first.reach() / ctx.path.speed(s0)).min(0.25 * ctx.path.length());
    let mut a = first;
    let mut ga = off(&a, rail, on_a);
    let mut found = None;
    if ga.abs() <= 1e-11 {
        found = Some(a.clone());
    } else {
        let mut b = ctx
            .station(s0 + inside * step, Some((a.uva, a.uvb)))
            .map_err(|f| ctx.refuse(f))?;
        let mut gb = off(&b, rail, on_a);
        for _ in 0..40 {
            if gb.abs() <= 1e-11 {
                found = Some(b.clone());
                break;
            }
            if (gb - ga).abs() <= f64::MIN_POSITIVE {
                break;
            }
            let s = b.s - gb * (b.s - a.s) / (gb - ga);
            if !s.is_finite() || (s - s0).abs() > limit {
                break;
            }
            let next = ctx
                .station(s, Some((b.uva, b.uvb)))
                .map_err(|f| ctx.refuse(f))?;
            (a, ga) = (b, gb);
            gb = off(&next, rail, on_a);
            b = next;
        }
    }
    let Some(found) = found else {
        return Err(ctx.refuse(Fail::NoFit));
    };
    // With an edge ending on each side, both must be reached at once.
    if let (Some(other), false) = (joint.rail_a, on_a)
        && off(&found, other, true).abs() > JOIN
    {
        return Err(KernelError::Unsupported(
            "the faces on both sides of the run of edges change, and the blend reaches the \
             two changes at different places; blend the edges one at a time"
                .to_owned(),
        ));
    }
    Ok(found)
}

/// A blend's section at a station as control points of a curve across it, in
/// homogeneous coordinates: a line for a chamfer, `pieces` rational quadratic arcs for a
/// fillet. Every station of one face gives the same number, on the same knots.
fn section(shape: Blend, st: &Station, pieces: usize) -> Vec<DVec4> {
    let plain = |p: DVec3| p.extend(1.0);
    match shape {
        Blend::Chamfer { .. } => vec![plain(st.pa), plain(st.pb)],
        Blend::Fillet { .. } => {
            let w = (0.5 * st.sweep() / pieces as f64).cos();
            let mut out = vec![plain(st.pa)];
            for i in 0..pieces {
                // The middle control point: on the arc's bisector, 1/w as far out.
                let on_arc = st.across(shape, (i as f64 + 0.5) / pieces as f64);
                out.push(((st.c + (on_arc - st.c) / w) * w).extend(w));
                out.push(plain(if i + 1 == pieces {
                    st.pb
                } else {
                    st.across(shape, (i + 1) as f64 / pieces as f64)
                }));
            }
            out
        }
    }
}

/// The four cubic B-spline basis functions that are not zero at `t`, which is in the
/// span starting at `knots[span]`.
fn cubic_basis(knots: &[f64], span: usize, t: f64) -> [f64; 4] {
    let mut n = [1.0, 0.0, 0.0, 0.0];
    let (mut left, mut right) = ([0.0; 4], [0.0; 4]);
    for j in 1..=3 {
        left[j] = t - knots[span + 1 - j];
        right[j] = knots[span + j] - t;
        let mut saved = 0.0;
        for r in 0..j {
            let temp = n[r] / (right[r + 1] + left[j - r]);
            n[r] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        n[j] = saved;
    }
    n
}

/// The sections of a blend skinned into a surface: `u` across the blend, `v` along it,
/// each station at the `v` that is its `s`. Along `v` the surface is the cubic spline
/// through the sections with the given rates of change at its two ends (per unit of
/// `s`), which keeps it as true next to its ends as in its middle.
fn skin(
    shape: Blend,
    stations: &[Station],
    pieces: usize,
    slopes: &[Vec<DVec4>; 2],
) -> Result<NurbsSurface, KernelError> {
    let n = stations.len() - 1;
    let sections: Vec<Vec<DVec4>> = stations
        .iter()
        .map(|st| section(shape, st, pieces))
        .collect();
    let count_u = sections[0].len();
    let count_v = n + 3;
    // A knot at every station.
    let mut knots_v = vec![stations[0].s; 4];
    knots_v.extend(stations[1..n].iter().map(|st| st.s));
    knots_v.extend(std::iter::repeat_n(stations[n].s, 4));
    // Row k: the spline passes through section k at its knot.
    let rows: Vec<[f64; 4]> = (1..n)
        .map(|k| cubic_basis(&knots_v, k + 3, stations[k].s))
        .collect();
    let (first, last) = (
        (stations[1].s - stations[0].s) / 3.0,
        (stations[n].s - stations[n - 1].s) / 3.0,
    );
    let mut grid = vec![DVec4::ZERO; count_u * count_v];
    for i in 0..count_u {
        let mut column = vec![DVec4::ZERO; count_v];
        column[0] = sections[0][i];
        column[1] = sections[0][i] + slopes[0][i] * first;
        column[n + 1] = sections[n][i] - slopes[1][i] * last;
        column[n + 2] = sections[n][i];
        if n >= 2 {
            // Unknowns P₂ … Pₙ, three to a row: eliminate downwards, substitute back.
            let m = n - 1;
            let mut diagonal = vec![0.0; m];
            let mut rhs = vec![DVec4::ZERO; m];
            for k in 0..m {
                let [a, b, c, _] = rows[k];
                let mut value = sections[k + 1][i];
                if k == 0 {
                    value -= column[1] * a;
                }
                if k == m - 1 {
                    value -= column[n + 1] * c;
                }
                let mut pivot = b;
                if k > 0 {
                    let factor = a / diagonal[k - 1];
                    pivot -= factor * rows[k - 1][2];
                    value -= rhs[k - 1] * factor;
                }
                diagonal[k] = pivot;
                rhs[k] = value;
            }
            for k in (0..m).rev() {
                let mut value = rhs[k];
                if k + 1 < m {
                    value -= column[k + 3] * rows[k][2];
                }
                column[k + 2] = value / diagonal[k];
            }
        }
        for (j, h) in column.into_iter().enumerate() {
            grid[i * count_v + j] = h;
        }
    }
    if grid.iter().any(|h| !(h.w.is_finite() && h.w > 1e-9)) {
        return Err(KernelError::InvalidResult(
            "the blend's surface can't be made: its sections differ too much".to_owned(),
        ));
    }
    let (degree_u, mut knots_u) = match shape {
        Blend::Chamfer { .. } => (1, vec![0.0; 2]),
        Blend::Fillet { .. } => (2, vec![0.0; 3]),
    };
    for i in 1..pieces {
        let k = i as f64 / pieces as f64;
        knots_u.extend([k, k]);
    }
    knots_u.extend(std::iter::repeat_n(1.0, degree_u + 1));
    NurbsSurface::new(
        degree_u,
        3,
        knots_u,
        knots_v,
        grid.iter().map(|h| h.truncate() / h.w).collect(),
        Some(grid.iter().map(|h| h.w).collect()),
    )
    .map_err(|m| KernelError::InvalidResult(format!("the blend's surface can't be made: {m}")))
}

/// Builds one blend face between two bounds on an edge.
fn build_piece(
    ctx: &EdgeCtx,
    slot: usize,
    from: Bound,
    to: Bound,
    what: &str,
) -> Result<Piece, KernelError> {
    let refuse = |f: Fail| ctx.refuse(f);
    let length = ctx.path.length();
    let (lo, hi) = (
        match &from {
            Bound::Fixed(st, _) => st.s,
            Bound::Open(_) => 0.0,
        },
        match &to {
            Bound::Fixed(st, _) => st.s,
            Bound::Open(_) => length,
        },
    );
    if (hi - lo).is_nan() || hi - lo <= 1e-9 * length.max(1.0) {
        return Err(KernelError::InvalidInput(format!(
            "the edge is too short for a {what} of this size"
        )));
    }
    // The stations the face runs between. Past an open end it is carried on, a stride
    // at a time, until the whole section is beyond the face that ends it.
    let mut first = match &from {
        Bound::Fixed(st, _) => st.clone(),
        Bound::Open(_) => ctx.station(lo, None).map_err(refuse)?,
    };
    let mut last = match &to {
        Bound::Fixed(st, _) => st.clone(),
        Bound::Open(_) => ctx.station(hi, None).map_err(refuse)?,
    };
    let beyond = |st: &Station, open: &OpenEnd, margin: f64| {
        (0..=8).all(|k| {
            let p = st.across(ctx.shape, f64::from(k) / 8.0);
            (p - open.origin).dot(open.normal) > margin
        })
    };
    for (bound, at_end) in [(&from, false), (&to, true)] {
        let Bound::Open(open) = bound else { continue };
        let end = if at_end { &mut last } else { &mut first };
        let stride = 0.5 * end.reach();
        let ds = stride / ctx.path.speed(end.s) * if at_end { 1.0 } else { -1.0 };
        let mut done = false;
        for _ in 0..24 {
            *end = ctx
                .station(end.s + ds, Some((end.uva, end.uvb)))
                .map_err(refuse)?;
            if beyond(end, open, 0.02 * stride) {
                done = true;
                break;
            }
        }
        if !done {
            return Err(KernelError::Unsupported(format!(
                "the face the edge ends on is too slanted to the edge for the {what} to be \
                 cut off by it"
            )));
        }
    }
    let (s0, s1) = (first.s, last.s);

    // Evenly spaced stations between them, each started from the one before; the skin
    // through them is measured halfway between stations, and if it is off there the
    // stations are spaced as much closer as a cubic's error says is needed.
    let mut intervals = 8;
    let mut round = 0;
    let (surface, stations) = loop {
        let mut stations: Vec<Station> = Vec::with_capacity(intervals + 1);
        stations.push(first.clone());
        for k in 1..intervals {
            let prev = &stations[k - 1];
            let s = s0 + (s1 - s0) * k as f64 / intervals as f64;
            let st = ctx.station(s, Some((prev.uva, prev.uvb))).map_err(refuse)?;
            // The ball must keep moving forwards: a spine that turns back is a fold.
            if (st.c - prev.c).dot(st.tan + prev.tan) <= 0.0 {
                return Err(refuse(Fail::NoFit));
            }
            stations.push(st);
        }
        {
            // The far end's own section, with parameters that carry on from here.
            let prev = &stations[intervals - 1];
            let mut st = last.clone();
            st.uva = ctx.a.surface.param_near(st.uva, prev.uva);
            st.uvb = ctx.b.surface.param_near(st.uvb, prev.uvb);
            stations.push(st);
        }
        let widest = stations.iter().map(Station::sweep).fold(0.0, f64::max);
        let pieces = match ctx.shape {
            Blend::Fillet { .. } if widest > 1.75 => 2,
            _ => 1,
        };
        // How fast the section changes at each end, from two stations just inside.
        let h = (s1 - s0) / intervals as f64;
        let delta = 1e-3 * h;
        let mut slopes = [Vec::new(), Vec::new()];
        for (k, (end, inwards)) in [(&stations[0], 1.0), (&stations[intervals], -1.0)]
            .into_iter()
            .enumerate()
        {
            let near = Some((end.uva, end.uvb));
            let one = ctx.station(end.s + inwards * delta, near).map_err(refuse)?;
            let two = ctx
                .station(end.s + inwards * 2.0 * delta, near)
                .map_err(refuse)?;
            let (q0, q1, q2) = (
                section(ctx.shape, end, pieces),
                section(ctx.shape, &one, pieces),
                section(ctx.shape, &two, pieces),
            );
            slopes[k] = (0..q0.len())
                .map(|i| (q1[i] * 4.0 - q0[i] * 3.0 - q2[i]) * (inwards / (2.0 * delta)))
                .collect();
        }
        let surface = skin(ctx.shape, &stations, pieces, &slopes)?;
        let mut worst: f64 = 0.0;
        for i in 0..intervals {
            let (p, q) = (&stations[i], &stations[i + 1]);
            let v = 0.5 * (p.s + q.s);
            let near = (0.5 * (p.uva + q.uva), 0.5 * (p.uvb + q.uvb));
            let middle = ctx.station(v, Some(near)).map_err(refuse)?;
            // The true section against the skin.
            for f in [0.0, 0.5, 1.0] {
                let target = middle.across(ctx.shape, f);
                let uv = surface.param_from(target, DVec2::new(f, v));
                worst = worst.max(surface.point(uv).distance(target) / SURFACE_FIT);
            }
            // The skin's two long edges against the faces they should lie on, and (a
            // fillet) its slope against theirs.
            for (u, side, near) in [(0.0, &ctx.a, middle.uva), (1.0, &ctx.b, middle.uvb)] {
                let at = DVec2::new(u, v);
                let (uv, off) = nearest(&side.surface, surface.point(at), near);
                worst = worst.max(off / CURVE_FIT);
                if matches!(ctx.shape, Blend::Fillet { .. }) {
                    let turn = surface.normal(at).cross(jet(&side.surface, uv).n).length();
                    worst = worst.max(turn / TANGENT_FIT);
                }
            }
        }
        if worst <= 1.0 {
            break (surface, stations);
        }
        round += 1;
        // A cubic's error falls with the fourth power of the spacing.
        let wanted = (intervals as f64 * worst.powf(0.25) * 1.1).ceil() as usize;
        intervals = wanted.clamp(intervals + 1, 4 * intervals);
        if intervals > MAX_STATIONS || round > 12 {
            return Err(KernelError::Unsupported(format!(
                "the {what} can't be made accurately enough on this edge: its faces are too \
                 uneven. Try a different size"
            )));
        }
    };

    // Where a flat face ends the blend: the curve it cuts the surface in.
    let params: Vec<f64> = stations.iter().map(|st| st.s).collect();
    let mut ends = Vec::with_capacity(2);
    for (bound, at_end) in [(from, false), (to, true)] {
        ends.push(match bound {
            Bound::Fixed(_, j) => PieceEnd::Junction(j),
            Bound::Open(open) => {
                let (curve, va, vb) = cap_curve(&surface, &params, &open, at_end, what)?;
                PieceEnd::Cap {
                    open,
                    curve: Box::new(curve),
                    va,
                    vb,
                }
            }
        });
    }
    let end = ends.pop().expect("two ends");
    let start = ends.pop().expect("two ends");
    Ok(Piece {
        slot,
        surface: Arc::new(surface),
        start,
        end,
        stations,
    })
}

/// The curve in which the plane of `open` cuts the blend `surface` near its start or
/// end, as a curve from `u = 0` to `u = 1`, with the `v` it has at those two ends.
fn cap_curve(
    surface: &NurbsSurface,
    params: &[f64],
    open: &OpenEnd,
    at_end: bool,
    what: &str,
) -> Result<(NurbsCurve, f64, f64), KernelError> {
    let slanted = || {
        KernelError::Unsupported(format!(
            "the face the edge ends on is too slanted to the edge for the {what} to be cut \
             off by it"
        ))
    };
    let height = |u: f64, v: f64| (surface.point(DVec2::new(u, v)) - open.origin).dot(open.normal);
    // The `v` at which the surface's line `u` passes through the plane: bracketed
    // between stations from the outside in, then bisected.
    let crossing = |u: f64| -> Option<f64> {
        let n = params.len();
        let order: Vec<usize> = if at_end {
            (0..n).rev().collect()
        } else {
            (0..n).collect()
        };
        if height(u, params[order[0]]) <= 0.0 {
            return None;
        }
        let mut outside = params[order[0]];
        for &k in &order[1..] {
            let h = height(u, params[k]);
            if h < 0.0 {
                let mut inside = params[k];
                for _ in 0..64 {
                    let mid = 0.5 * (inside + outside);
                    if height(u, mid) < 0.0 {
                        inside = mid;
                    } else {
                        outside = mid;
                    }
                }
                return Some(0.5 * (inside + outside));
            }
            outside = params[k];
        }
        None
    };
    let mut count = 8;
    loop {
        let mut vs = Vec::with_capacity(count + 1);
        for k in 0..=count {
            vs.push(crossing(k as f64 / count as f64).ok_or_else(slanted)?);
        }
        let points: Vec<DVec3> = (0..=count)
            .map(|k| surface.point(DVec2::new(k as f64 / count as f64, vs[k])))
            .collect();
        let curve = NurbsCurve::interpolate(&points).map_err(|_| slanted())?;
        // Between the points the curve must stay on the surface and in the plane.
        let mut worst: f64 = 0.0;
        for k in 0..count {
            let mid = 0.5 * (points[k] + points[k + 1]);
            let p = curve.point(curve.param(mid));
            let near = DVec2::new((k as f64 + 0.5) / count as f64, 0.5 * (vs[k] + vs[k + 1]));
            let on = surface.point(surface.param_from(p, near)).distance(p);
            worst = worst.max(on).max((p - open.origin).dot(open.normal).abs());
        }
        if worst <= CURVE_FIT || count >= 256 {
            if worst > 20.0 * LINEAR {
                return Err(slanted());
            }
            return Ok((curve, vs[0], vs[count]));
        }
        count *= 2;
    }
}

/// Refuses a blend that reaches another edge of the faces it is on.
fn swallowed(
    solid: &Solid,
    run: &Run,
    planned: &Planned,
    selected: &[EdgeId],
    trims: &[Trim],
    what: &str,
) -> Result<(), KernelError> {
    for piece in &planned.pieces {
        let re = &run.edges[piece.slot];
        for (on_a, coedge) in [(true, re.ca), (false, re.cb)] {
            let face = solid.coedge_face(coedge);
            for &l in &solid.face(face).loops {
                for c in solid.loop_coedges(l) {
                    let id = solid.coedge(c).edge;
                    if run.edges.iter().any(|r| r.edge == id) {
                        continue;
                    }
                    let e = solid.edge(id);
                    // The part of the edge that is left once it is cut back.
                    let (mut t0, mut t1) = (e.t0, e.t1);
                    let (mut f0, mut f1) = (0.0, 1.0);
                    for t in trims.iter().filter(|t| t.edge == id) {
                        if t.at_start {
                            (t0, f0) = (t.t, 0.03);
                        } else {
                            (t1, f1) = (t.t, 0.97);
                        }
                    }
                    // Another blended edge of this face needs room for its own blend.
                    let room = if selected.contains(&id) { 2.0 } else { 1.0 };
                    for k in 0..=8 {
                        let f = f0 + (f1 - f0) * f64::from(k) / 8.0;
                        let q = e.curve.point(t0 + (t1 - t0) * f);
                        if in_strip(&piece.stations, q, on_a, room) {
                            return Err(KernelError::InvalidInput(format!(
                                "the {what} is too big: it would reach another edge of a face \
                                 next to the one it is on. Try a smaller one"
                            )));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Whether `q`, a point of one of a blend's faces, is in the strip the blend takes
/// from that face (widened `room` times).
fn in_strip(stations: &[Station], q: DVec3, on_a: bool, room: f64) -> bool {
    let Some((k, st)) = stations.iter().enumerate().min_by(|a, b| {
        a.1.e
            .distance_squared(q)
            .total_cmp(&b.1.e.distance_squared(q))
    }) else {
        return false;
    };
    let along = (q - st.e).dot(st.tan);
    // Past the first or last station it is beyond this piece.
    if (k == 0 && along < -1e-9) || (k + 1 == stations.len() && along > 1e-9) {
        return false;
    }
    let across = ((q - st.e).length_squared() - along * along)
        .max(0.0)
        .sqrt();
    let reach = if on_a { st.pa } else { st.pb }.distance(st.e);
    across < 0.98 * room * reach
}

/// The solid being rebuilt: the old one's vertices, edges and loops, edited freely and
/// then written out as a new solid.
struct Draft {
    points: Vec<DVec3>,
    edges: Vec<Option<DraftEdge>>,
    faces: Vec<DraftFace>,
}

struct DraftEdge {
    curve: Curve3,
    start: usize,
    end: usize,
    t0: f64,
    t1: f64,
}

struct DraftFace {
    surface: Surface,
    reversed: bool,
    shell: usize,
    loops: Vec<Vec<(usize, bool)>>,
    label: BlendFace,
}

impl Draft {
    fn new(solid: &Solid) -> Self {
        Self {
            points: solid.vertices.iter().map(|v| v.point).collect(),
            edges: solid
                .edges
                .iter()
                .map(|e| {
                    Some(DraftEdge {
                        curve: e.curve.clone(),
                        start: e.start.index(),
                        end: e.end.index(),
                        t0: e.t0,
                        t1: e.t1,
                    })
                })
                .collect(),
            faces: solid
                .face_ids()
                .map(|id| {
                    let f = solid.face(id);
                    DraftFace {
                        surface: f.surface.clone(),
                        reversed: f.reversed,
                        shell: f.shell.index(),
                        loops: f
                            .loops
                            .iter()
                            .map(|&l| {
                                solid
                                    .loop_coedges(l)
                                    .into_iter()
                                    .map(|c| {
                                        let c = solid.coedge(c);
                                        (c.edge.index(), c.reversed)
                                    })
                                    .collect()
                            })
                            .collect(),
                        label: BlendFace::Original(id),
                    }
                })
                .collect(),
        }
    }

    fn point(&mut self, p: DVec3) -> usize {
        self.points.push(p);
        self.points.len() - 1
    }

    fn edge(&mut self, curve: Curve3, start: usize, end: usize, t0: f64, t1: f64) -> usize {
        self.edges.push(Some(DraftEdge {
            curve,
            start,
            end,
            t0,
            t1,
        }));
        self.edges.len() - 1
    }

    /// Replaces the use of `edge` in a loop of `face` by `uses`.
    fn replace(&mut self, face: FaceId, edge: EdgeId, uses: &[(usize, bool)]) {
        for l in &mut self.faces[face.index()].loops {
            if let Some(at) = l.iter().position(|(e, _)| *e == edge.index()) {
                l.splice(at..=at, uses.iter().copied());
                return;
            }
        }
    }

    /// Puts `new` after the use `after` in a loop of `face`.
    fn insert_after(&mut self, face: FaceId, after: (EdgeId, bool), new: (usize, bool)) {
        for l in &mut self.faces[face.index()].loops {
            if let Some(at) = l
                .iter()
                .position(|&(e, r)| e == after.0.index() && r == after.1)
            {
                l.insert(at + 1, new);
                return;
            }
        }
    }

    /// Cuts an edge back to end (or start) at vertex `v`.
    fn trim(&mut self, trim: &Trim, v: usize) {
        if let Some(e) = &mut self.edges[trim.edge.index()] {
            if trim.at_start {
                (e.start, e.t0) = (v, trim.t);
            } else {
                (e.end, e.t1) = (v, trim.t);
            }
        }
    }

    /// Puts a planned run's faces in.
    fn apply(&mut self, solid: &Solid, run: &Run, planned: Planned, shape: Blend) {
        // Junctions: two vertices and the section between them.
        let mut joints: Vec<(usize, usize, usize)> = Vec::with_capacity(planned.junctions.len());
        for j in &planned.junctions {
            let (va, vb) = (self.point(j.pa), self.point(j.pb));
            let across = match shape {
                Blend::Chamfer { .. } => {
                    let curve = Curve3::line_through(j.pa, j.pb).expect("a section has a width");
                    self.edge(curve, va, vb, 0.0, j.pa.distance(j.pb))
                }
                Blend::Fillet { .. } => {
                    let (a, b) = (j.pa - j.c, j.pb - j.c);
                    let frame = Frame::from_origin_z_x(j.c, a.cross(b), a)
                        .expect("a fillet's section spans an angle");
                    let circle = Curve3::Circle(Circle3 {
                        frame,
                        radius: a.length(),
                    });
                    self.edge(circle, va, vb, 0.0, a.angle_between(b))
                }
            };
            joints.push((va, vb, across));
            for (rail, v) in j.rails.iter().zip([va, vb]) {
                if let Some(rail) = rail {
                    self.trim(rail, v);
                }
            }
        }
        // Per edge of the run: the contact curves that replace it on its two faces.
        let mut on_a: Vec<Vec<(usize, bool)>> = vec![Vec::new(); run.edges.len()];
        let mut on_b: Vec<Vec<(usize, bool)>> = vec![Vec::new(); run.edges.len()];
        for piece in planned.pieces {
            let re = &run.edges[piece.slot];
            // Each end: its vertices on the two sides, the edge across, and the
            // surface's `v` there on each side.
            let mut ends = Vec::with_capacity(2);
            for (end, at_end) in [(&piece.start, false), (&piece.end, true)] {
                ends.push(match end {
                    PieceEnd::Junction(j) => {
                        let (va, vb, across) = joints[*j];
                        let (lo, hi) = piece.surface.domain();
                        let v = if at_end { hi.y } else { lo.y };
                        (va, vb, across, v, v)
                    }
                    PieceEnd::Cap {
                        open,
                        curve,
                        va,
                        vb,
                    } => {
                        let (lo, hi) = curve.domain();
                        let (pa, pb) = (curve.point(lo), curve.point(hi));
                        let (qa, qb) = (self.point(pa), self.point(pb));
                        let across = self.edge(
                            Curve3::Nurbs(Arc::new(NurbsCurve::clone(curve))),
                            qa,
                            qb,
                            lo,
                            hi,
                        );
                        for (edge, v, point) in [(open.edge_a, qa, pa), (open.edge_b, qb, pb)] {
                            let e = solid.edge(edge);
                            let trim = Trim {
                                edge,
                                at_start: e.start == open.vertex,
                                t: param_on(&e.curve, e.t0, e.t1, point),
                            };
                            self.trim(&trim, v);
                        }
                        // The cap's loop goes from the first face's side to the second's
                        // at the run's end, and the other way at its start.
                        self.insert_after(open.cap, open.arriving, (across, !at_end));
                        (qa, qb, across, *va, *vb)
                    }
                });
            }
            let (sa, sb, start_across, sva, svb) = ends[0];
            let (ea, eb, end_across, eva, evb) = ends[1];
            let side_a = Curve3::Nurbs(Arc::new(piece.surface.iso_curve(false, 0.0)));
            let side_b = Curve3::Nurbs(Arc::new(piece.surface.iso_curve(false, 1.0)));
            let curve_a = self.edge(side_a, sa, ea, sva, eva);
            let curve_b = self.edge(side_b, sb, eb, svb, evb);
            on_a[piece.slot].push((curve_a, false));
            on_b[piece.slot].push((curve_b, true));
            let shell = solid.face(solid.coedge_face(re.ca)).shell.index();
            self.faces.push(DraftFace {
                surface: Surface::Nurbs(piece.surface),
                reversed: false,
                shell,
                // Up the second face's side, back across the end, down the first
                // face's side, across the start.
                loops: vec![vec![
                    (curve_b, false),
                    (end_across, true),
                    (curve_a, true),
                    (start_across, false),
                ]],
                label: BlendFace::Blend(re.index),
            });
        }
        for (slot, re) in run.edges.iter().enumerate() {
            self.replace(solid.coedge_face(re.ca), re.edge, &on_a[slot]);
            on_b[slot].reverse();
            self.replace(solid.coedge_face(re.cb), re.edge, &on_b[slot]);
            self.edges[re.edge.index()] = None;
        }
    }

    /// Writes the draft out as a solid, with what each face is.
    fn build(self) -> Labelled {
        let mut solid = Solid::new();
        let mut labels = Vec::with_capacity(self.faces.len());
        let shells = self.faces.iter().map(|f| f.shell + 1).max().unwrap_or(0);
        for _ in 0..shells {
            solid.add_shell();
        }
        let mut vertex: Vec<Option<VertexId>> = vec![None; self.points.len()];
        let mut edge: Vec<Option<EdgeId>> = vec![None; self.edges.len()];
        for (i, e) in self.edges.iter().enumerate() {
            let Some(e) = e else { continue };
            let mut v = |k: usize, solid: &mut Solid| {
                *vertex[k].get_or_insert_with(|| solid.add_vertex(self.points[k]))
            };
            let (start, end) = (v(e.start, &mut solid), v(e.end, &mut solid));
            edge[i] = Some(solid.add_edge(e.curve.clone(), start, end, e.t0, e.t1));
        }
        for f in &self.faces {
            let id = solid.add_face(
                crate::topo::ShellId(f.shell as u32),
                f.surface.clone(),
                f.reversed,
            );
            for l in &f.loops {
                let uses: Vec<(EdgeId, bool)> = l
                    .iter()
                    .filter_map(|&(e, r)| edge[e].map(|e| (e, r)))
                    .collect();
                if !uses.is_empty() {
                    solid.add_loop(id, &uses);
                }
            }
            labels.push(vec![f.label]);
        }
        Labelled { solid, labels }
    }
}

#[cfg(test)]
mod tests;
