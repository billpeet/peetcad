//! Glyph outlines as lines and arcs: each Bézier piece becomes biarcs (two arcs meeting
//! tangent to each other and to the curve at its ends) within a tolerance.
//!
//! Sheet metal takes only lines and arcs, so text has no splines. A piece is split where
//! its direction turns through more than a quarter turn or changes the way it turns (a
//! cubic's inflection), then halved until its biarc is within the tolerance everywhere.

use std::f64::consts::{FRAC_PI_2, TAU};

use peet_math::DVec2;

use super::GlyphCurve;

/// A Bézier piece of an outline: two (a line), three (quadratic) or four (cubic) points.
#[derive(Clone, Debug)]
pub(crate) struct Bezier(pub Vec<DVec2>);

impl Bezier {
    fn point(&self, t: f64) -> DVec2 {
        let mut p = self.0.clone();
        for k in (1..p.len()).rev() {
            for i in 0..k {
                p[i] = p[i].lerp(p[i + 1], t);
            }
        }
        p[0]
    }

    fn split(&self, t: f64) -> (Bezier, Bezier) {
        let mut left = Vec::new();
        let mut right = Vec::new();
        let mut p = self.0.clone();
        while !p.is_empty() {
            left.push(p[0]);
            right.push(*p.last().unwrap());
            p = p.windows(2).map(|w| w[0].lerp(w[1], t)).collect();
        }
        right.reverse();
        (Bezier(left), Bezier(right))
    }

    fn start(&self) -> DVec2 {
        self.0[0]
    }

    fn end(&self) -> DVec2 {
        *self.0.last().unwrap()
    }

    /// Unit tangents at the two ends, skipping control points on top of an end.
    fn tangents(&self) -> Option<(DVec2, DVec2)> {
        let n = self.0.len();
        let t0 = (1..n).find_map(|i| {
            let d = self.0[i] - self.0[0];
            (d.length() > 1e-12).then(|| d.normalize())
        })?;
        let t1 = (0..n - 1).rev().find_map(|i| {
            let d = self.0[n - 1] - self.0[i];
            (d.length() > 1e-12).then(|| d.normalize())
        })?;
        Some((t0, t1))
    }

    /// How far the control points are from the chord: zero for a straight piece.
    fn flatness(&self) -> f64 {
        let (a, b) = (self.start(), self.end());
        let d = b - a;
        let len = d.length();
        self.0
            .iter()
            .map(|p| {
                if len < 1e-12 {
                    p.distance(a)
                } else {
                    // Distance to the chord as a segment: a control point beyond an end
                    // makes the piece bulge past it.
                    let t = ((*p - a).dot(d) / (len * len)).clamp(0.0, 1.0);
                    p.distance(a + d * t)
                }
            })
            .fold(0.0, f64::max)
    }

    /// Parameters where a cubic's curvature changes sign, in (0, 1).
    fn inflections(&self) -> Vec<f64> {
        if self.0.len() != 4 {
            return Vec::new();
        }
        let [p0, p1, p2, p3] = [self.0[0], self.0[1], self.0[2], self.0[3]];
        // B'(t) = 3(a t² + b t + c), with a, b, c below; inflections are where
        // B' × B'' = 0, a quadratic in t.
        let a = p3 - p0 + (p1 - p2) * 3.0;
        let b = (p0 - p1 * 2.0 + p2) * 2.0;
        let c = p1 - p0;
        let qa = a.perp_dot(b);
        let qb = 2.0 * a.perp_dot(c);
        let qc = b.perp_dot(c);
        let mut out = Vec::new();
        if qa.abs() < 1e-18 {
            if qb.abs() > 1e-18 {
                out.push(-qc / qb);
            }
        } else {
            let disc = qb * qb - 4.0 * qa * qc;
            if disc >= 0.0 {
                let s = disc.sqrt();
                out.push((-qb - s) / (2.0 * qa));
                out.push((-qb + s) / (2.0 * qa));
            }
        }
        out.retain(|t| *t > 1e-6 && *t < 1.0 - 1e-6);
        out.sort_by(f64::total_cmp);
        out
    }
}

/// The circle through `a` with tangent `t` there that passes through `b`: its centre and
/// signed radius (positive turning left). `None` if it is a straight line.
fn arc_from(a: DVec2, t: DVec2, b: DVec2) -> Option<(DVec2, f64)> {
    let n = t.perp();
    let d = b - a;
    let denom = 2.0 * n.dot(d);
    if denom.abs() < 1e-14 * d.length_squared().max(1e-30) {
        return None;
    }
    let r = d.length_squared() / denom;
    Some((a + n * r, r))
}

/// A circular piece from `a` to `b` around `center`, turning left (`r > 0`) or right.
fn arc_piece(a: DVec2, b: DVec2, center: DVec2, r: f64) -> GlyphCurve {
    // So flat that its chord is as good: a line.
    if a.distance_squared(b) / (8.0 * r.abs()) < 1e-8 {
        return GlyphCurve::Line { a, b };
    }
    let ang = |p: DVec2| (p - center).y.atan2((p - center).x);
    let (from, to) = if r > 0.0 { (a, b) } else { (b, a) };
    let start = ang(from);
    let sweep = (ang(to) - start).rem_euclid(TAU);
    GlyphCurve::Arc {
        center,
        radius: r.abs(),
        start_angle: start,
        sweep,
    }
}

/// Lines and arcs within `tol` of the piece, appended to `out`.
pub(crate) fn fit(piece: &Bezier, tol: f64, out: &mut Vec<GlyphCurve>) {
    if piece.start().distance(piece.end()) < 1e-12 && piece.flatness() < 1e-12 {
        return;
    }
    let ts = piece.inflections();
    if ts.is_empty() {
        fit_simple(piece, tol, out, 0);
        return;
    }
    let mut rest = piece.clone();
    let mut done = 0.0;
    for t in ts {
        let (l, r) = rest.split((t - done) / (1.0 - done));
        fit_simple(&l, tol, out, 0);
        rest = r;
        done = t;
    }
    fit_simple(&rest, tol, out, 0);
}

fn fit_simple(piece: &Bezier, tol: f64, out: &mut Vec<GlyphCurve>, depth: u32) {
    let (a, b) = (piece.start(), piece.end());
    if piece.flatness() <= tol {
        if a.distance(b) > 1e-12 {
            out.push(GlyphCurve::Line { a, b });
        }
        return;
    }
    let halve = |out: &mut Vec<GlyphCurve>| {
        let (l, r) = piece.split(0.5);
        fit_simple(&l, tol, out, depth + 1);
        fit_simple(&r, tol, out, depth + 1);
    };
    if depth > 24 {
        out.push(GlyphCurve::Line { a, b });
        return;
    }
    let Some((t0, t1)) = piece.tangents() else {
        out.push(GlyphCurve::Line { a, b });
        return;
    };
    let turn = t0.perp_dot(t1).atan2(t0.dot(t1));
    if turn.abs() > FRAC_PI_2 {
        return halve(out);
    }
    match biarc(a, t0, b, t1) {
        Some(arcs) if deviation(piece, &arcs) <= tol => out.extend(arcs),
        _ => halve(out),
    }
}

/// Two arcs from `a` (tangent `t0`) to `b` (tangent `t1`) meeting tangent to each other at
/// the incentre of the triangle `a`, `b` and where the tangents cross.
fn biarc(a: DVec2, t0: DVec2, b: DVec2, t1: DVec2) -> Option<Vec<GlyphCurve>> {
    // Where the tangent lines cross: a + t0 s = b - t1 u.
    let d = b - a;
    let den = t0.perp_dot(t1);
    if den.abs() < 1e-12 {
        return None;
    }
    let s = d.perp_dot(t1) / den;
    let u = t0.perp_dot(d) / den;
    if s <= 0.0 || u <= 0.0 {
        return None;
    }
    let v = a + t0 * s;
    let (la, lb, lc) = (v.distance(b), a.distance(b), a.distance(v));
    let j = (a * la + v * lb + b * lc) / (la + lb + lc);
    let mut arcs = Vec::new();
    for (p, t, q) in [(a, t0, j), (b, -t1, j)] {
        match arc_from(p, t, q) {
            Some((c, r)) => {
                // The second arc is found from `b` backwards: it turns the other way.
                let (from, to, r) = if p == a { (p, q, r) } else { (q, p, -r) };
                arcs.push(arc_piece(from, to, c, r));
            }
            None => arcs.push(GlyphCurve::Line {
                a: if p == a { p } else { q },
                b: if p == a { q } else { p },
            }),
        }
    }
    Some(arcs)
}

/// The furthest the piece gets from the arcs, sampled along it.
fn deviation(piece: &Bezier, arcs: &[GlyphCurve]) -> f64 {
    let dist = |p: DVec2| {
        arcs.iter()
            .map(|c| c.distance(p))
            .fold(f64::INFINITY, f64::min)
    };
    (1..32)
        .map(|i| dist(piece.point(i as f64 / 32.0)))
        .fold(0.0, f64::max)
}
