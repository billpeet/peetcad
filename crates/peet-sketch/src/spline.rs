//! Splines: smooth curves through fit points.
//!
//! A sketch spline is defined by the points it passes through. [`Spline::interpolate`]
//! turns them into a cubic B-spline with a clamped knot vector:
//!
//! - **Open**: the natural cubic spline through the points (no curvature at its two ends).
//! - **Closed**: the periodic cubic spline, smooth through the closing point as through
//!   every other. It is still stored clamped, so it starts and ends at the first fit
//!   point with a seam there, the way the kernel keeps its closed curves.
//!
//! Parameters are centripetal (the square root of the distance between fit points), which
//! keeps the curve from overshooting where the points are unevenly spaced, and run from
//! 0 to 1.
//!
//! **Exactness.** The degree, knots and control points are the whole definition: the
//! kernel builds its edge from the same numbers ([`SplinePiece::to_spline`]), so the curve
//! drawn in the sketch and the edge of the solid are one curve, not two approximations.
//!
//! A [`SplinePiece`] is a stretch of a spline between two parameters. Region detection
//! cuts curves where they cross; a piece shares its spline's data instead of copying it.

use std::sync::Arc;

use peet_math::{DVec2, tolerance};

/// Fit points closer than this (mm) are one point.
const DISTINCT: f64 = 1e-9;

/// Knots closer than this are one knot.
const SAME_KNOT: f64 = 1e-12;

/// Samples per knot span when looking for roots and closest points.
const SAMPLES_PER_SPAN: usize = 8;

/// Gauss–Legendre nodes and weights on `[-1, 1]` (five points: exact to degree nine).
const GAUSS: [(f64, f64); 5] = [
    (0.0, 0.568_888_888_888_888_9),
    (-0.538_469_310_105_683_1, 0.478_628_670_499_366_5),
    (0.538_469_310_105_683_1, 0.478_628_670_499_366_5),
    (-0.906_179_845_938_664, 0.236_926_885_056_189_08),
    (0.906_179_845_938_664, 0.236_926_885_056_189_08),
];

/// A B-spline curve in the sketch plane: degree 3 through fit points (degree 1 where the
/// points leave no room for a cubic). Not rational.
#[derive(Clone, Debug, PartialEq)]
pub struct Spline {
    degree: usize,
    knots: Vec<f64>,
    points: Vec<DVec2>,
    closed: bool,
    /// The parameter at which the curve passes each fit point it was made from.
    fit_params: Vec<f64>,
}

/// The knot span containing `u`: the index `i` with `knots[i] <= u < knots[i + 1]`, kept
/// to the spans that have a length (the last one for `u` at the end).
fn find_span(degree: usize, knots: &[f64], count: usize, u: f64) -> usize {
    if u >= knots[count] {
        let mut i = count - 1;
        while i > degree && knots[i] == knots[count] {
            i -= 1;
        }
        return i;
    }
    if u <= knots[degree] {
        let mut i = degree;
        while i + 1 < count && knots[i + 1] == knots[degree] {
            i += 1;
        }
        return i;
    }
    let (mut lo, mut hi) = (degree, count);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if u < knots[mid] {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    lo
}

/// The `order`-th derivative at `u` (in knot span `span`) of the B-spline of `degree` over
/// `knots` whose control point `i` is `ctrl(i)`. De Boor's algorithm; a derivative is the
/// B-spline of one degree less through the scaled differences of the control points.
fn eval(
    degree: usize,
    knots: &[f64],
    span: usize,
    u: f64,
    ctrl: &dyn Fn(usize) -> DVec2,
    order: usize,
) -> DVec2 {
    if order > 0 {
        if degree == 0 {
            return DVec2::ZERO;
        }
        let p = degree as f64;
        let difference = |i: usize| {
            let width = knots[i + degree + 1] - knots[i + 1];
            if width > 0.0 {
                (ctrl(i + 1) - ctrl(i)) * (p / width)
            } else {
                DVec2::ZERO
            }
        };
        return eval(
            degree - 1,
            &knots[1..knots.len() - 1],
            span - 1,
            u,
            &difference,
            order - 1,
        );
    }
    let mut d = [DVec2::ZERO; 4];
    debug_assert!(degree <= 3);
    for (j, slot) in d.iter_mut().enumerate().take(degree + 1) {
        *slot = ctrl(span - degree + j);
    }
    for r in 1..=degree {
        for j in (r..=degree).rev() {
            let i = span - degree + j;
            let width = knots[i + degree + 1 - r] - knots[i];
            let a = if width > 0.0 {
                (u - knots[i]) / width
            } else {
                0.0
            };
            d[j] = d[j - 1] * (1.0 - a) + d[j] * a;
        }
    }
    d[degree]
}

/// The `order`-th derivative at `u` of basis function `j`.
fn basis(degree: usize, knots: &[f64], span: usize, u: f64, j: usize, order: usize) -> f64 {
    let unit = |i: usize| if i == j { DVec2::X } else { DVec2::ZERO };
    eval(degree, knots, span, u, &unit, order).x
}

/// Solves a tridiagonal system: row `i` is `sub[i] x[i−1] + diag[i] x[i] + sup[i] x[i+1] =
/// rhs[i]`. `None` if a pivot vanishes.
fn solve_tridiagonal(sub: &[f64], diag: &[f64], sup: &[f64], rhs: &[DVec2]) -> Option<Vec<DVec2>> {
    let n = diag.len();
    let mut c = vec![0.0; n];
    let mut x = vec![DVec2::ZERO; n];
    let mut pivot = diag[0];
    if pivot.abs() < 1e-300 {
        return None;
    }
    x[0] = rhs[0] / pivot;
    for i in 1..n {
        c[i - 1] = sup[i - 1] / pivot;
        pivot = diag[i] - sub[i] * c[i - 1];
        if pivot.abs() < 1e-300 || !pivot.is_finite() {
            return None;
        }
        x[i] = (rhs[i] - x[i - 1] * sub[i]) / pivot;
    }
    for i in (0..n - 1).rev() {
        let next = x[i + 1];
        x[i] -= next * c[i];
    }
    Some(x)
}

/// Solves a cyclic tridiagonal system (`sub[0]` multiplies the last unknown, `sup[n−1]`
/// the first) by the Sherman–Morrison formula. Needs at least three unknowns.
fn solve_cyclic(sub: &[f64], diag: &[f64], sup: &[f64], rhs: &[DVec2]) -> Option<Vec<DVec2>> {
    let n = diag.len();
    let (alpha, beta) = (sub[0], sup[n - 1]);
    let gamma = -diag[0];
    let mut d = diag.to_vec();
    d[0] -= gamma;
    d[n - 1] -= alpha * beta / gamma;
    let x = solve_tridiagonal(sub, &d, sup, rhs)?;
    let mut u = vec![DVec2::ZERO; n];
    u[0] = DVec2::new(gamma, 0.0);
    u[n - 1] = DVec2::new(beta, 0.0);
    let z = solve_tridiagonal(sub, &d, sup, &u)?;
    let denominator = 1.0 + z[0].x + alpha * z[n - 1].x / gamma;
    if denominator.abs() < 1e-300 {
        return None;
    }
    let factor = (x[0] + x[n - 1] * (alpha / gamma)) / denominator;
    Some(x.iter().zip(&z).map(|(x, z)| *x - factor * z.x).collect())
}

/// Inserts `t` once into a knot vector (Boehm's algorithm). The curve is unchanged.
fn insert_knot(degree: usize, knots: &mut Vec<f64>, points: &mut Vec<DVec2>, t: f64) {
    let n = points.len();
    let span = find_span(degree, knots, n, t);
    let mut out = Vec::with_capacity(n + 1);
    for i in 0..=n {
        out.push(if i + degree <= span {
            points[i]
        } else if i > span {
            points[i - 1]
        } else {
            let a = (t - knots[i]) / (knots[i + degree] - knots[i]);
            points[i - 1] * (1.0 - a) + points[i] * a
        });
    }
    knots.insert(span + 1, t);
    *points = out;
}

/// The part of a B-spline between `a < b` (inside its domain) as a clamped B-spline of
/// its own, with the same parametrisation. The knot vector given need not be clamped.
fn extract(
    degree: usize,
    mut knots: Vec<f64>,
    mut points: Vec<DVec2>,
    a: f64,
    b: f64,
) -> (Vec<f64>, Vec<DVec2>) {
    // A cut at a knot uses the knot's own value, so the two stay equal.
    let snap = |knots: &[f64], t: f64| {
        knots
            .iter()
            .copied()
            .find(|k| (*k - t).abs() <= SAME_KNOT)
            .unwrap_or(t)
    };
    let multiplicity = |knots: &[f64], t: f64| knots.iter().filter(|k| **k == t).count();
    let a = snap(&knots, a);
    let b = snap(&knots, b);
    for t in [a, b] {
        for _ in multiplicity(&knots, t)..degree {
            insert_knot(degree, &mut knots, &mut points, t);
        }
    }
    let last_a = knots.iter().rposition(|k| *k == a).expect("inserted");
    let first_b = knots.iter().position(|k| *k == b).expect("inserted");
    let (from, to) = (last_a + 1 - degree, first_b + degree - 1);
    let mut out_knots = Vec::with_capacity(to - from + 3);
    out_knots.push(a);
    out_knots.extend_from_slice(&knots[from..=to]);
    out_knots.push(b);
    let count = out_knots.len() - degree - 1;
    let out_points = points[from - 1..from - 1 + count].to_vec();
    (out_knots, out_points)
}

impl Spline {
    /// The spline through `through`, in order. `closed` brings it back to the first point,
    /// smooth there too; it needs three different points, and is ignored with fewer.
    ///
    /// This never fails: points that coincide are passed once, and with fewer than two
    /// different points the result is a curve of no length at the first point.
    pub fn interpolate(through: &[DVec2], closed: bool) -> Self {
        // The different points, and which of them each fit point is.
        let mut distinct: Vec<DVec2> = Vec::with_capacity(through.len());
        let mut index = Vec::with_capacity(through.len());
        for p in through {
            if p.is_finite() && distinct.last().is_none_or(|q| q.distance(*p) > DISTINCT) {
                distinct.push(*p);
            }
            index.push(distinct.len().saturating_sub(1));
        }
        // A closed curve returns to its first point by itself.
        if closed
            && distinct.len() > 1
            && distinct[0].distance(distinct[distinct.len() - 1]) <= DISTINCT
        {
            distinct.pop();
        }
        let closed = closed && distinct.len() >= 3;
        if distinct.len() < 2 {
            let at = distinct.first().copied().unwrap_or(DVec2::ZERO);
            return Self {
                degree: 1,
                knots: vec![0.0, 0.0, 1.0, 1.0],
                points: vec![at, at],
                closed: false,
                fit_params: vec![0.0; through.len()],
            };
        }

        // Centripetal parameters, from 0 to 1 (a closed curve's include the way back).
        let n = distinct.len();
        let chords = if closed { n } else { n - 1 };
        let mut params = Vec::with_capacity(chords + 1);
        params.push(0.0);
        for k in 0..chords {
            let step = distinct[k].distance(distinct[(k + 1) % n]).sqrt();
            params.push(params[k] + step);
        }
        let total = params[chords];
        for u in &mut params {
            *u /= total;
        }
        params[chords] = 1.0;
        // Fit points merged into the closing point are at the end of a closed curve.
        let fit_params = index
            .iter()
            .map(|&k| match (k < n, closed) {
                (true, _) => params[k],
                (false, true) => 1.0,
                (false, false) => 0.0,
            })
            .collect();

        let made = if closed {
            periodic_cubic(&distinct, &params)
        } else {
            natural_cubic(&distinct, &params)
        };
        let scale = distinct
            .iter()
            .fold(1.0_f64, |m, p| m.max(p.abs().max_element()));
        let (degree, knots, points) = made
            .filter(|(knots, points)| {
                // The curve must pass through its points; otherwise the system was too
                // badly conditioned to trust.
                points.iter().all(|p| p.is_finite())
                    && distinct.iter().zip(&params).all(|(q, u)| {
                        let span = find_span(3, knots, points.len(), *u);
                        eval(3, knots, span, *u, &|i| points[i], 0).distance(*q) <= 1e-9 * scale
                    })
            })
            .map(|(knots, points)| (3, knots, points))
            .unwrap_or_else(|| polyline(&distinct, &params, closed));
        Self {
            degree,
            knots,
            points,
            closed,
            fit_params,
        }
    }

    /// The degree: 3, or 1 for a curve too degenerate for a cubic.
    pub fn degree(&self) -> usize {
        self.degree
    }

    /// The clamped knot vector.
    pub fn knots(&self) -> &[f64] {
        &self.knots
    }

    /// The control points (which the curve does not pass through, except the end ones).
    pub fn control_points(&self) -> &[DVec2] {
        &self.points
    }

    /// Whether the curve was made closed: it ends where it starts, smoothly.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// The parameter at which the curve passes each of the fit points it was made from,
    /// in their order. A closed curve passes its first point at both ends of its domain;
    /// this gives the start.
    pub fn fit_params(&self) -> &[f64] {
        &self.fit_params
    }

    /// The parameter range.
    pub fn domain(&self) -> (f64, f64) {
        (self.knots[0], self.knots[self.knots.len() - 1])
    }

    /// The point and the first two derivatives at `u` (clamped to the domain).
    pub fn evaluate(&self, u: f64) -> [DVec2; 3] {
        let (lo, hi) = self.domain();
        let u = u.clamp(lo, hi);
        let span = find_span(self.degree, &self.knots, self.points.len(), u);
        let ctrl = |i: usize| self.points[i];
        [0, 1, 2].map(|order| eval(self.degree, &self.knots, span, u, &ctrl, order))
    }

    /// The point at `u` (clamped to the domain).
    pub fn point(&self, u: f64) -> DVec2 {
        self.evaluate(u)[0]
    }

    /// The part between the parameters `a < b` (limited to the domain) as a spline of its
    /// own: the same points at the same parameters, with a clamped knot vector from `a`
    /// to `b`.
    pub fn between(&self, a: f64, b: f64) -> Self {
        let (lo, hi) = self.domain();
        let (a, b) = (a.clamp(lo, hi), b.clamp(lo, hi));
        if b - a <= SAME_KNOT {
            let at = self.point(a);
            return Self {
                degree: 1,
                knots: vec![0.0, 0.0, 1.0, 1.0],
                points: vec![at, at],
                closed: false,
                fit_params: Vec::new(),
            };
        }
        let whole = a - lo <= SAME_KNOT && hi - b <= SAME_KNOT;
        if whole {
            return self.clone();
        }
        let (knots, points) = extract(self.degree, self.knots.clone(), self.points.clone(), a, b);
        let (a, b) = (knots[0], knots[knots.len() - 1]);
        Self {
            degree: self.degree,
            knots,
            points,
            closed: false,
            fit_params: self
                .fit_params
                .iter()
                .copied()
                .filter(|u| (a..=b).contains(u))
                .collect(),
        }
    }

    /// The curve with every point mapped by `f`, which must be an affine map (a move, a
    /// turn, a mirror, a scale): those map control points to control points.
    pub fn mapped(&self, f: impl Fn(DVec2) -> DVec2) -> Self {
        Self {
            points: self.points.iter().map(|p| f(*p)).collect(),
            ..self.clone()
        }
    }

    /// The same curve run the other way: `point(u)` becomes `point(lo + hi − u)`.
    pub fn reversed(&self) -> Self {
        let (lo, hi) = self.domain();
        Self {
            degree: self.degree,
            knots: self.knots.iter().rev().map(|k| lo + hi - k).collect(),
            points: self.points.iter().rev().copied().collect(),
            closed: self.closed,
            fit_params: self.fit_params.iter().rev().map(|u| lo + hi - u).collect(),
        }
    }

    /// The knots between `a` and `b` where the polynomial pieces meet, with `a` and `b`.
    fn breaks(&self, a: f64, b: f64) -> Vec<f64> {
        let mut out = vec![a];
        for &k in &self.knots {
            if k > a + SAME_KNOT && k < b - SAME_KNOT && k > out[out.len() - 1] {
                out.push(k);
            }
        }
        out.push(b);
        out
    }

    /// `∫ f(u) du` from `a` to `b`, by Gauss–Legendre quadrature on each quarter of each
    /// polynomial piece.
    fn integrate(&self, a: f64, b: f64, f: impl Fn(f64) -> f64) -> f64 {
        let mut total = 0.0;
        for w in self.breaks(a, b).windows(2) {
            let step = 0.25 * (w[1] - w[0]);
            for k in 0..4 {
                let (lo, hi) = (w[0] + step * f64::from(k), w[0] + step * f64::from(k + 1));
                let (centre, half) = (0.5 * (lo + hi), 0.5 * (hi - lo));
                total += GAUSS
                    .iter()
                    .map(|(x, weight)| weight * f(centre + half * x))
                    .sum::<f64>()
                    * half;
            }
        }
        total
    }

    /// The largest second derivative between `a` and `b`, which lie in one polynomial
    /// piece. For a cubic it is linear there, so it is largest at an end.
    fn bend(&self, a: f64, b: f64) -> f64 {
        if self.degree < 2 {
            return 0.0;
        }
        let span = find_span(self.degree, &self.knots, self.points.len(), 0.5 * (a + b));
        let ctrl = |i: usize| self.points[i];
        let at = |u: f64| eval(self.degree, &self.knots, span, u, &ctrl, 2).length();
        at(a).max(at(b))
    }
}

/// The natural cubic spline through `q` at `params`: knots and control points.
fn natural_cubic(q: &[DVec2], params: &[f64]) -> Option<(Vec<f64>, Vec<DVec2>)> {
    let n = q.len() - 1;
    let count = n + 3;
    let mut knots = vec![params[0]; 4];
    knots.extend_from_slice(&params[1..n]);
    knots.extend(std::iter::repeat_n(params[n], 4));
    // Ordered so that row `r` involves control points `r − 1 ..= r + 1`: the first point,
    // no curvature at the start, the points between, no curvature at the end, the last.
    let mut sub = vec![0.0; count];
    let mut diag = vec![0.0; count];
    let mut sup = vec![0.0; count];
    let mut rhs = vec![DVec2::ZERO; count];
    diag[0] = 1.0;
    rhs[0] = q[0];
    diag[count - 1] = 1.0;
    rhs[count - 1] = q[n];
    let mut row = |r: usize, u: f64, order: usize, value: DVec2| {
        let span = find_span(3, &knots, count, u);
        sub[r] = basis(3, &knots, span, u, r - 1, order);
        diag[r] = basis(3, &knots, span, u, r, order);
        sup[r] = basis(3, &knots, span, u, r + 1, order);
        rhs[r] = value;
    };
    row(1, params[0], 2, DVec2::ZERO);
    for k in 1..n {
        row(k + 1, params[k], 0, q[k]);
    }
    row(n + 1, params[n], 2, DVec2::ZERO);
    let points = solve_tridiagonal(&sub, &diag, &sup, &rhs)?;
    Some((knots, points))
}

/// The periodic cubic spline through `q` (and back to `q[0]`) at `params` (one more than
/// points: the last is where the curve is back at the start), clamped at the seam.
fn periodic_cubic(q: &[DVec2], params: &[f64]) -> Option<(Vec<f64>, Vec<DVec2>)> {
    let n = q.len();
    let period = params[n] - params[0];
    // The periodic knot vector, three knots beyond each end: knot `j` is at `j + 3`.
    let knot = |j: isize| {
        let turns = j.div_euclid(n as isize);
        params[j.rem_euclid(n as isize) as usize] + period * turns as f64
    };
    let knots: Vec<f64> = (-3..=n as isize + 3).map(knot).collect();
    let count = n + 3;
    // At fit point `k` three basis functions are non-zero: those of control points
    // `k − 3 ..= k − 1`, counted round. With unknown `i` the control point `i − 2`, row
    // `k` involves unknowns `k − 1 ..= k + 1`.
    let mut sub = vec![0.0; n];
    let mut diag = vec![0.0; n];
    let mut sup = vec![0.0; n];
    for k in 0..n {
        let u = params[k];
        let span = k + 3;
        sub[k] = basis(3, &knots, span, u, k, 0);
        diag[k] = basis(3, &knots, span, u, k + 1, 0);
        sup[k] = basis(3, &knots, span, u, k + 2, 0);
    }
    let unknowns = solve_cyclic(&sub, &diag, &sup, q)?;
    // Control point `j` (for `j` from −3) is unknown `j + 2`, counted round.
    let points: Vec<DVec2> = (0..count).map(|i| unknowns[(i + n - 1) % n]).collect();
    Some(extract(3, knots, points, params[0], params[n]))
}

/// The straight pieces from point to point: what a spline falls back to.
fn polyline(q: &[DVec2], params: &[f64], closed: bool) -> (usize, Vec<f64>, Vec<DVec2>) {
    let mut points = q.to_vec();
    if closed {
        points.push(q[0]);
    }
    let last = points.len() - 1;
    let mut knots = vec![params[0]];
    knots.extend_from_slice(&params[..=last]);
    knots.push(params[last]);
    (1, knots, points)
}

/// A stretch of a [`Spline`] between two of its parameters. The curve parameter `t` of
/// [`crate::Curve`] runs from 0 at the start of the stretch to 1 at its end.
#[derive(Clone, Debug)]
pub struct SplinePiece {
    spline: Arc<Spline>,
    from: f64,
    to: f64,
}

impl PartialEq for SplinePiece {
    fn eq(&self, other: &Self) -> bool {
        self.from == other.from
            && self.to == other.to
            && (Arc::ptr_eq(&self.spline, &other.spline) || self.spline == other.spline)
    }
}

impl SplinePiece {
    /// The whole of `spline`.
    pub fn whole(spline: Spline) -> Self {
        let (from, to) = spline.domain();
        Self {
            spline: Arc::new(spline),
            from,
            to,
        }
    }

    /// The spline this is a stretch of.
    pub fn spline(&self) -> &Spline {
        &self.spline
    }

    /// The stretch, in the spline's own parameter.
    pub fn range(&self) -> (f64, f64) {
        (self.from, self.to)
    }

    /// Whether this is the whole spline.
    pub fn is_whole(&self) -> bool {
        (self.from, self.to) == self.spline.domain()
    }

    /// Whether this is the whole of a closed spline.
    pub fn is_closed(&self) -> bool {
        self.spline.closed && self.is_whole()
    }

    /// The stretch with every point mapped by `f`, which must be an affine map.
    pub fn mapped(&self, f: impl Fn(DVec2) -> DVec2) -> Self {
        Self {
            spline: Arc::new(self.spline.mapped(f)),
            from: self.from,
            to: self.to,
        }
    }

    /// The stretch as a spline of its own: exactly this curve, with a clamped knot vector
    /// over [`SplinePiece::range`]. This is what the kernel builds an edge from.
    pub fn to_spline(&self) -> Spline {
        self.spline.between(self.from, self.to)
    }

    /// The spline's parameter at the curve parameter `t`.
    pub fn param(&self, t: f64) -> f64 {
        self.from + t * (self.to - self.from)
    }

    /// The curve parameter at the spline's parameter `u`.
    pub fn local(&self, u: f64) -> f64 {
        let width = self.to - self.from;
        if width > 0.0 {
            (u - self.from) / width
        } else {
            0.0
        }
    }

    /// The stretch between the curve parameters `t0 < t1` (limited to this stretch).
    pub fn sub_piece(&self, t0: f64, t1: f64) -> Self {
        let (t0, t1) = (t0.clamp(0.0, 1.0), t1.clamp(0.0, 1.0));
        Self {
            spline: Arc::clone(&self.spline),
            from: self.param(t0.min(t1)),
            to: self.param(t0.max(t1)),
        }
    }

    /// The point at curve parameter `t`. Beyond the ends the curve carries straight on.
    pub fn point_at(&self, t: f64) -> DVec2 {
        let width = self.to - self.from;
        if t < 0.0 {
            let [p, d, _] = self.spline.evaluate(self.from);
            p + d * (t * width)
        } else if t > 1.0 {
            let [p, d, _] = self.spline.evaluate(self.to);
            p + d * ((t - 1.0) * width)
        } else {
            self.spline.point(self.param(t))
        }
    }

    /// The unit tangent at curve parameter `t`.
    pub fn tangent_at(&self, t: f64) -> DVec2 {
        let t = t.clamp(0.0, 1.0);
        let d = self.spline.evaluate(self.param(t))[1];
        d.try_normalize().unwrap_or_else(|| {
            // No speed here (a doubled point): the direction a little further on.
            let step = if t < 0.5 { 1e-6 } else { -1e-6 };
            let ahead = self.spline.point(self.param(t + step)) - self.spline.point(self.param(t));
            (ahead * step.signum()).normalize_or(DVec2::X)
        })
    }

    /// The signed curvature at curve parameter `t` (positive turning left).
    pub fn curvature_at(&self, t: f64) -> f64 {
        let [_, d1, d2] = self.spline.evaluate(self.param(t.clamp(0.0, 1.0)));
        let speed = d1.length();
        if speed > 1e-12 {
            d1.perp_dot(d2) / (speed * speed * speed)
        } else {
            0.0
        }
    }

    /// The length of the stretch.
    pub fn length(&self) -> f64 {
        self.spline
            .integrate(self.from, self.to, |u| self.spline.evaluate(u)[1].length())
    }

    /// `½ ∫ (x dy − y dx)` along the stretch: its share of the area of a loop it is part of.
    pub fn area_term(&self) -> f64 {
        0.5 * self.spline.integrate(self.from, self.to, |u| {
            let [p, d, _] = self.spline.evaluate(u);
            p.perp_dot(d)
        })
    }

    /// Spline parameters to look between: a few in every polynomial piece.
    fn samples(&self) -> Vec<f64> {
        let breaks = self.spline.breaks(self.from, self.to);
        let mut out = Vec::with_capacity((breaks.len() - 1) * SAMPLES_PER_SPAN + 1);
        for w in breaks.windows(2) {
            for i in 0..SAMPLES_PER_SPAN {
                out.push(w[0] + (w[1] - w[0]) * i as f64 / SAMPLES_PER_SPAN as f64);
            }
        }
        out.push(self.to);
        out
    }

    /// The closest point of the stretch to `p`, as `(t, point)`.
    pub fn closest_point(&self, p: DVec2) -> (f64, DVec2) {
        let samples = self.samples();
        // Newton from the best few samples: enough to start in the right basin.
        let mut order: Vec<(f64, f64)> = samples
            .iter()
            .map(|&u| (self.spline.point(u).distance_squared(p), u))
            .collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut best = (f64::INFINITY, self.from);
        for &(_, start) in order.iter().take(3) {
            let mut u = start;
            for _ in 0..32 {
                let [c, d1, d2] = self.spline.evaluate(u);
                let r = c - p;
                let f = r.dot(d1);
                let df = d1.length_squared() + r.dot(d2);
                let step = if df > 0.0 {
                    f / df
                } else if d1.length_squared() > 0.0 {
                    f / d1.length_squared()
                } else {
                    break;
                };
                let next = (u - step).clamp(self.from, self.to);
                let moved = (next - u).abs();
                u = next;
                if moved <= 1e-15 {
                    break;
                }
            }
            let d = self.spline.point(u).distance_squared(p);
            if d < best.0 {
                best = (d, u);
            }
        }
        (self.local(best.1), self.spline.point(best.1))
    }

    /// The spline parameters in the stretch where `f` is zero. `f` is a smooth function of
    /// the parameter (a signed distance of the curve's point from something); `tolerance`
    /// is how close to zero counts where `f` touches zero without crossing it.
    fn roots(&self, f: impl Fn(f64) -> f64, tolerance: f64) -> Vec<f64> {
        let xs = self.samples();
        let fs: Vec<f64> = xs.iter().map(|&x| f(x)).collect();
        let bisect = |mut a: f64, mut fa: f64, mut b: f64| {
            for _ in 0..64 {
                let m = 0.5 * (a + b);
                let fm = f(m);
                if fm == 0.0 || (b - a) <= 1e-15 {
                    return m;
                }
                if (fm > 0.0) == (fa > 0.0) {
                    (a, fa) = (m, fm);
                } else {
                    b = m;
                }
            }
            0.5 * (a + b)
        };
        let mut out: Vec<f64> = Vec::new();
        let last = xs.len() - 1;
        for i in 0..=last {
            if fs[i] == 0.0 {
                out.push(xs[i]);
                continue;
            }
            if i < last && fs[i + 1] != 0.0 && (fs[i] > 0.0) != (fs[i + 1] > 0.0) {
                out.push(bisect(xs[i], fs[i], xs[i + 1]));
            }
            // A dip towards zero between samples: touching, or crossing twice.
            let sign = fs[i].signum();
            let left = if i > 0 {
                fs[i - 1] * sign
            } else {
                f64::INFINITY
            };
            let right = if i < last {
                fs[i + 1] * sign
            } else {
                f64::INFINITY
            };
            let here = fs[i] * sign;
            if left < 0.0 || right < 0.0 || here > left || here > right {
                continue;
            }
            let (lo, hi) = (xs[i.saturating_sub(1)], xs[(i + 1).min(last)]);
            let (x, value) = minimum(lo, hi, |x| f(x) * sign);
            if value < 0.0 {
                out.push(bisect(lo, fs[i.saturating_sub(1)], x));
                out.push(bisect(x, value * sign, hi));
            } else if value <= tolerance {
                out.push(x);
            }
        }
        out.sort_by(f64::total_cmp);
        out.dedup_by(|b, a| self.spline.point(*a).distance(self.spline.point(*b)) <= tolerance);
        out
    }

    /// Where the stretch meets the infinite line through `origin` along `dir`: curve
    /// parameters and points.
    pub(crate) fn meet_line(&self, origin: DVec2, dir: DVec2) -> Vec<(f64, DVec2)> {
        let Some(dir) = dir.try_normalize() else {
            return Vec::new();
        };
        let f = |u: f64| dir.perp_dot(self.spline.point(u) - origin);
        self.hits(self.roots(f, tolerance::LINEAR))
    }

    /// Where the stretch meets the full circle about `center`.
    pub(crate) fn meet_circle(&self, center: DVec2, radius: f64) -> Vec<(f64, DVec2)> {
        let f = |u: f64| self.spline.point(u).distance(center) - radius;
        self.hits(self.roots(f, tolerance::LINEAR))
    }

    fn hits(&self, roots: Vec<f64>) -> Vec<(f64, DVec2)> {
        roots
            .into_iter()
            .map(|u| (self.local(u), self.spline.point(u)))
            .collect()
    }

    /// The stretch as straight segments for finding crossings: parameters and points.
    fn chords(&self) -> Vec<(f64, DVec2)> {
        self.samples()
            .into_iter()
            .map(|u| (u, self.spline.point(u)))
            .collect()
    }

    /// Where the stretch crosses `other`: the curve parameter on each, and the point.
    /// Places where the two only touch are not found, except at their ends (which region
    /// detection looks at separately).
    pub(crate) fn meet_spline(&self, other: &Self) -> Vec<(f64, f64, DVec2)> {
        let (a, b) = (self.chords(), other.chords());
        let mut out: Vec<(f64, f64, DVec2)> = Vec::new();
        for i in 0..a.len() - 1 {
            for j in 0..b.len() - 1 {
                let Some((s, t)) = segments_cross(a[i].1, a[i + 1].1, b[j].1, b[j + 1].1) else {
                    continue;
                };
                let u = a[i].0 + s * (a[i + 1].0 - a[i].0);
                let v = b[j].0 + t * (b[j + 1].0 - b[j].0);
                if let Some((u, v, p)) = self.refine_crossing(other, u, v)
                    && !out
                        .iter()
                        .any(|(_, _, q)| q.distance(p) <= tolerance::LINEAR)
                {
                    out.push((self.local(u), other.local(v), p));
                }
            }
        }
        out
    }

    /// Where the stretch crosses itself: the two curve parameters and the point.
    pub(crate) fn meet_self(&self) -> Vec<(f64, f64, DVec2)> {
        let a = self.chords();
        let mut out: Vec<(f64, f64, DVec2)> = Vec::new();
        let width = self.to - self.from;
        for i in 0..a.len() - 1 {
            for j in i + 2..a.len() - 1 {
                let Some((s, t)) = segments_cross(a[i].1, a[i + 1].1, a[j].1, a[j + 1].1) else {
                    continue;
                };
                let u = a[i].0 + s * (a[i + 1].0 - a[i].0);
                let v = a[j].0 + t * (a[j + 1].0 - a[j].0);
                let Some((u, v, p)) = self.refine_crossing(self, u, v) else {
                    continue;
                };
                // Not the same place on the curve, nor a closed curve's seam.
                let apart = (v - u).abs();
                if apart <= 1e-6 * width || apart >= (1.0 - 1e-6) * width {
                    continue;
                }
                if !out
                    .iter()
                    .any(|(_, _, q)| q.distance(p) <= tolerance::LINEAR)
                {
                    out.push((self.local(u), self.local(v), p));
                }
            }
        }
        out
    }

    /// Newton's method on `self(u) = other(v)`, from a crossing of their chords.
    fn refine_crossing(&self, other: &Self, mut u: f64, mut v: f64) -> Option<(f64, f64, DVec2)> {
        for _ in 0..32 {
            let [p, dp, _] = self.spline.evaluate(u);
            let [q, dq, _] = other.spline.evaluate(v);
            let r = q - p;
            let det = dp.perp_dot(dq);
            if det.abs() <= 1e-300 {
                break;
            }
            // p + dp du = q + dq dv
            let du = r.perp_dot(dq) / det;
            let dv = r.perp_dot(dp) / det;
            u = (u + du).clamp(self.from, self.to);
            v = (v + dv).clamp(other.from, other.to);
            if du.abs() <= 1e-15 && dv.abs() <= 1e-15 {
                break;
            }
        }
        let (p, q) = (self.spline.point(u), other.spline.point(v));
        (p.distance(q) <= tolerance::LINEAR).then_some((u, v, (p + q) * 0.5))
    }

    /// Axis-aligned bounds `(min, max)` of the stretch.
    pub fn bounds(&self) -> (DVec2, DVec2) {
        let (a, b) = (self.spline.point(self.from), self.spline.point(self.to));
        let (mut min, mut max) = (a.min(b), a.max(b));
        // The extremes are where the curve runs along an axis.
        for axis in [DVec2::X, DVec2::Y] {
            for u in self.roots(|u| self.spline.evaluate(u)[1].dot(axis), 0.0) {
                let p = self.spline.point(u);
                min = min.min(p);
                max = max.max(p);
            }
        }
        (min, max)
    }

    /// Points along the stretch such that the chord error stays below `tolerance`,
    /// including both ends.
    pub fn tessellate(&self, tolerance: f64) -> Vec<DVec2> {
        let tolerance = tolerance.max(1e-9);
        let mut out = vec![self.spline.point(self.from)];
        for w in self.spline.breaks(self.from, self.to).windows(2) {
            // A chord over a parameter step `h` strays at most `h² |C″| / 8` from the curve.
            let h = w[1] - w[0];
            let bend = self.spline.bend(w[0], w[1]);
            let n = ((bend * h * h / (8.0 * tolerance)).sqrt().ceil() as usize).clamp(1, 512);
            for i in 1..=n {
                out.push(self.spline.point(w[0] + h * i as f64 / n as f64));
            }
        }
        out
    }

    /// The angle the stretch sweeps as seen from `p` (radians, counter-clockwise
    /// positive), exact however close `p` is to the curve. Summed round a loop this gives
    /// the loop's winding number about `p`.
    pub fn angle_swept(&self, p: DVec2) -> f64 {
        self.spline
            .breaks(self.from, self.to)
            .windows(2)
            .map(|w| {
                let (a, b) = (self.spline.point(w[0]), self.spline.point(w[1]));
                self.swept(p, (w[0], a), (w[1], b), 0)
            })
            .sum()
    }

    fn swept(&self, p: DVec2, a: (f64, DVec2), b: (f64, DVec2), depth: usize) -> f64 {
        // The curve between `a` and `b` stays within `stray` of its chord. Seen from
        // outside that band, it sweeps the angle its chord does.
        let h = b.0 - a.0;
        let stray = self.spline.bend(a.0, b.0) * h * h / 8.0;
        let chord = b.1 - a.1;
        let along = if chord.length_squared() > 0.0 {
            ((p - a.1).dot(chord) / chord.length_squared()).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let away = p.distance(a.1 + chord * along);
        if away > stray || depth >= 48 || h <= 1e-15 {
            return (a.1 - p).angle_to(b.1 - p);
        }
        let mid = 0.5 * (a.0 + b.0);
        let m = (mid, self.spline.point(mid));
        self.swept(p, a, m, depth + 1) + self.swept(p, m, b, depth + 1)
    }
}

/// Where segments `a0 a1` and `b0 b1` cross: the fraction along each.
fn segments_cross(a0: DVec2, a1: DVec2, b0: DVec2, b1: DVec2) -> Option<(f64, f64)> {
    let (d, e) = (a1 - a0, b1 - b0);
    let det = d.perp_dot(e);
    if det == 0.0 {
        return None;
    }
    let s = (b0 - a0).perp_dot(e) / det;
    let t = (b0 - a0).perp_dot(d) / det;
    // A little beyond the ends too: the curves bulge past their chords.
    let slack = 0.05;
    ((-slack..=1.0 + slack).contains(&s) && (-slack..=1.0 + slack).contains(&t)).then_some((s, t))
}

/// The minimum of `f` between `lo` and `hi` by golden-section search: where, and its value.
fn minimum(lo: f64, hi: f64, f: impl Fn(f64) -> f64) -> (f64, f64) {
    const RATIO: f64 = 0.618_033_988_749_894_9;
    let (mut a, mut b) = (lo, hi);
    let mut x1 = b - RATIO * (b - a);
    let mut x2 = a + RATIO * (b - a);
    let (mut f1, mut f2) = (f(x1), f(x2));
    for _ in 0..80 {
        if f1 < f2 {
            b = x2;
            (x2, f2) = (x1, f1);
            x1 = b - RATIO * (b - a);
            f1 = f(x1);
        } else {
            a = x1;
            (x1, f1) = (x2, f2);
            x2 = a + RATIO * (b - a);
            f2 = f(x2);
        }
        if b - a <= 1e-14 {
            break;
        }
    }
    if f1 < f2 { (x1, f1) } else { (x2, f2) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave() -> Vec<DVec2> {
        vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 8.0),
            DVec2::new(25.0, -3.0),
            DVec2::new(40.0, 6.0),
            DVec2::new(47.0, 0.0),
        ]
    }

    fn blob() -> Vec<DVec2> {
        vec![
            DVec2::new(10.0, 0.0),
            DVec2::new(4.0, 9.0),
            DVec2::new(-8.0, 5.0),
            DVec2::new(-9.0, -6.0),
            DVec2::new(3.0, -8.0),
        ]
    }

    #[test]
    fn open_spline_passes_its_points_with_no_end_curvature() {
        let q = wave();
        let s = Spline::interpolate(&q, false);
        assert_eq!(s.degree(), 3);
        assert_eq!(s.control_points().len(), q.len() + 2);
        for (p, u) in q.iter().zip(s.fit_params()) {
            assert!(s.point(*u).distance(*p) < 1e-10);
        }
        let (lo, hi) = s.domain();
        assert!(s.evaluate(lo)[2].length() < 1e-7);
        assert!(s.evaluate(hi)[2].length() < 1e-7);
        // Smooth across the knots: the second derivative has no jump.
        for &u in &s.fit_params()[1..q.len() - 1] {
            let (before, after) = (s.evaluate(u - 1e-7)[2], s.evaluate(u + 1e-7)[2]);
            assert!(before.distance(after) < 1e-2 * (1.0 + after.length()));
        }
    }

    #[test]
    fn two_points_make_a_straight_line() {
        let s = Spline::interpolate(&[DVec2::ZERO, DVec2::new(10.0, 5.0)], false);
        for i in 0..=10 {
            let u = f64::from(i) / 10.0;
            assert!(s.point(u).distance(DVec2::new(10.0, 5.0) * u) < 1e-12);
        }
        let piece = SplinePiece::whole(s);
        assert!((piece.length() - 125.0_f64.sqrt()).abs() < 1e-12);
    }

    #[test]
    fn closed_spline_is_smooth_through_its_seam() {
        let q = blob();
        let s = Spline::interpolate(&q, true);
        assert!(s.is_closed());
        for (p, u) in q.iter().zip(s.fit_params()) {
            assert!(s.point(*u).distance(*p) < 1e-10);
        }
        let (lo, hi) = s.domain();
        let (start, end) = (s.evaluate(lo), s.evaluate(hi));
        for k in 0..3 {
            assert!(
                start[k].distance(end[k]) < 1e-7 * (1.0 + start[k].length()),
                "derivative {k} jumps at the seam: {} vs {}",
                start[k],
                end[k]
            );
        }
        // The same curve whichever fit point it starts from.
        let mut turned = q.clone();
        turned.rotate_left(2);
        let t = SplinePiece::whole(Spline::interpolate(&turned, true));
        let piece = SplinePiece::whole(s);
        for i in 0..40 {
            let p = piece.point_at(f64::from(i) / 40.0);
            assert!(t.closest_point(p).1.distance(p) < 1e-8);
        }
        assert!((piece.length() - t.length()).abs() < 1e-8);
    }

    #[test]
    fn degenerate_input_never_panics() {
        let p = DVec2::new(3.0, 4.0);
        for (points, closed) in [
            (vec![], false),
            (vec![p], false),
            (vec![p, p, p], true),
            (vec![p, DVec2::ZERO], true),
            (vec![p, p, DVec2::ZERO, DVec2::ZERO, p], true),
            (vec![DVec2::NAN, p], false),
        ] {
            let s = Spline::interpolate(&points, closed);
            assert_eq!(s.fit_params().len(), points.len());
            let piece = SplinePiece::whole(s);
            assert!(piece.length().is_finite());
            assert!(piece.point_at(0.5).is_finite());
            assert!(piece.tangent_at(0.5).is_finite());
        }
    }

    #[test]
    fn a_stretch_is_the_same_curve() {
        let whole = SplinePiece::whole(Spline::interpolate(&wave(), false));
        let part = whole.sub_piece(0.2, 0.7);
        let own = part.to_spline();
        let (lo, hi) = own.domain();
        assert_eq!((lo, hi), part.range());
        assert_eq!(own.knots()[..4], [lo; 4]);
        for i in 0..=50 {
            let t = f64::from(i) / 50.0;
            let u = part.param(t);
            assert!(own.point(u).distance(whole.spline().point(u)) < 1e-12);
            assert!(part.point_at(t).distance(whole.point_at(0.2 + 0.5 * t)) < 1e-12);
        }
        let back = own.reversed();
        assert!(back.point(lo).distance(own.point(hi)) < 1e-12);
        assert!(back.point(lo + 0.1).distance(own.point(hi - 0.1)) < 1e-12);
        // Lengths add up.
        let total =
            whole.sub_piece(0.0, 0.2).length() + part.length() + whole.sub_piece(0.7, 1.0).length();
        assert!((total - whole.length()).abs() < 1e-6);
    }

    #[test]
    fn closest_point_and_bounds() {
        let piece = SplinePiece::whole(Spline::interpolate(&wave(), false));
        let on = piece.point_at(0.37);
        let normal = piece.tangent_at(0.37).perp();
        let (t, p) = piece.closest_point(on + normal * 0.5);
        assert!((t - 0.37).abs() < 1e-7 && p.distance(on) < 1e-7);
        let (min, max) = piece.bounds();
        for q in piece.tessellate(1e-4) {
            assert!(q.cmpge(min - 1e-9).all() && q.cmple(max + 1e-9).all());
        }
        // The bounds are reached, not just an outer box.
        let top = piece
            .tessellate(1e-5)
            .iter()
            .fold(f64::NEG_INFINITY, |m, q| m.max(q.y));
        assert!((max.y - top).abs() < 1e-3);
    }

    #[test]
    fn tessellation_keeps_to_its_tolerance() {
        let piece = SplinePiece::whole(Spline::interpolate(&blob(), true));
        for tolerance in [0.5, 0.01] {
            let pts = piece.tessellate(tolerance);
            for w in pts.windows(2) {
                let mid = (w[0] + w[1]) * 0.5;
                assert!(piece.closest_point(mid).1.distance(mid) <= tolerance * 1.001);
            }
        }
        assert!(piece.tessellate(0.01).len() > piece.tessellate(0.5).len());
    }

    #[test]
    fn crossings_with_lines_circles_and_splines() {
        let piece = SplinePiece::whole(Spline::interpolate(&wave(), false));
        let hits = piece.meet_line(DVec2::new(0.0, 2.0), DVec2::X);
        assert_eq!(hits.len(), 4, "{hits:?}");
        for (t, p) in &hits {
            assert!((p.y - 2.0).abs() < 1e-9);
            assert!(piece.point_at(*t).distance(*p) < 1e-9);
        }
        let hits = piece.meet_circle(DVec2::new(25.0, 0.0), 12.0);
        assert_eq!(hits.len(), 2);
        for (_, p) in &hits {
            assert!((p.distance(DVec2::new(25.0, 0.0)) - 12.0).abs() < 1e-9);
        }
        // A line that just touches the curve's highest point.
        let (_, max) = piece.bounds();
        let touch = piece.meet_line(DVec2::new(0.0, max.y), DVec2::X);
        assert_eq!(touch.len(), 1, "{touch:?}");
        // The same wave turned upside down crosses it.
        let flipped: Vec<DVec2> = wave().iter().map(|p| DVec2::new(p.x, 3.0 - p.y)).collect();
        let other = SplinePiece::whole(Spline::interpolate(&flipped, false));
        let hits = piece.meet_spline(&other);
        assert_eq!(hits.len(), 4, "{hits:?}");
        for (t, s, p) in hits {
            assert!(piece.point_at(t).distance(p) < 1e-6);
            assert!(other.point_at(s).distance(p) < 1e-6);
            assert!((p.y - 1.5).abs() < 1e-6);
        }
    }

    #[test]
    fn a_loop_in_a_spline_is_found() {
        let looped = [
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 10.0),
            DVec2::new(0.0, 12.0),
            DVec2::new(10.0, 0.0),
        ];
        let piece = SplinePiece::whole(Spline::interpolate(&looped, false));
        let hits = piece.meet_self();
        assert_eq!(hits.len(), 1, "{hits:?}");
        let (a, b, p) = hits[0];
        assert!(piece.point_at(a).distance(p) < 1e-6 && piece.point_at(b).distance(p) < 1e-6);
        assert!(b - a > 0.1);
        let plain = SplinePiece::whole(Spline::interpolate(&blob(), true));
        assert!(plain.meet_self().is_empty());
    }

    #[test]
    fn area_and_winding_of_a_closed_spline() {
        let piece = SplinePiece::whole(Spline::interpolate(&blob(), true));
        let area = piece.area_term();
        // Against a fine polygon.
        let n = 200_000;
        let polygon: f64 = (0..n)
            .map(|i| {
                let at = |i: usize| piece.point_at(i as f64 / n as f64);
                0.5 * at(i).perp_dot(at(i + 1))
            })
            .sum();
        assert!((area - polygon).abs() < 1e-6, "{area} vs {polygon}");
        assert!(area > 0.0, "the points run counter-clockwise");
        let turns = |p: DVec2| piece.angle_swept(p) / std::f64::consts::TAU;
        assert!((turns(DVec2::ZERO) - 1.0).abs() < 1e-9);
        assert!(turns(DVec2::new(30.0, 0.0)).abs() < 1e-9);
        // A hair inside and a hair outside the curve.
        let on = piece.point_at(0.3);
        let normal = piece.tangent_at(0.3).perp();
        assert!((turns(on + normal * 1e-7) - 1.0).abs() < 1e-6);
        assert!(turns(on - normal * 1e-7).abs() < 1e-6);
    }
}
