//! Freeform geometry: NURBS curves and surfaces.
//!
//! A NURBS curve is a piecewise rational polynomial given by a degree, a knot vector and
//! weighted control points; a surface is the same in two directions. They hold what the
//! analytic kinds can't: the sides of a loft, a spline through points, another CAD
//! system's freeform faces. The algorithms are the standard ones (Piegl and Tiller, *The
//! NURBS Book*): span search, basis functions and their derivatives, knot insertion,
//! global interpolation, skinning.
//!
//! **Conventions.**
//! - Knot vectors are clamped (the first and last knot repeated `degree + 1` times), so a
//!   curve starts at its first control point and ends at its last. The parameter runs
//!   over `[first knot, last knot]`; edges and faces use parts of that range.
//! - Curves and surfaces are *not* periodic: a closed shape is made of pieces that meet
//!   at ordinary edges, so no seam is needed and every face is a simple region of its
//!   surface's parameter rectangle.
//! - A surface's control points are stored row by row in `u`: point `(i, j)` is at
//!   `i * count_v + j`. Its natural normal is `∂S/∂u × ∂S/∂v`.
//! - `weights: None` means every weight is 1 (a plain B-spline).
//!
//! Closest-point queries ([`NurbsCurve::param`], [`NurbsSurface::param`]) start Newton's
//! method from the nearest of a set of samples, made once per curve or surface and kept
//! with it.

use std::sync::OnceLock;

use peet_math::{Aabb, DVec2, DVec3, DVec4, Frame};
use serde::{Deserialize, Serialize};

/// What is worked out once per curve or surface and kept with it: samples for starting
/// closest-point searches, and how much it bends and stretches. Not part of the
/// geometry's value: it compares equal to anything, is not saved, and a copy starts
/// without it.
#[derive(Default)]
struct Cache<T>(T);

impl<T: Default> Clone for Cache<T> {
    fn clone(&self) -> Self {
        Self(T::default())
    }
}

#[derive(Default)]
struct CurveCache {
    samples: OnceLock<Vec<(f64, DVec3)>>,
    bend: OnceLock<f64>,
}

#[derive(Default)]
struct SurfaceCache {
    samples: OnceLock<Vec<(DVec2, DVec3)>>,
    bend: OnceLock<DVec2>,
    stretch: OnceLock<DVec2>,
}

/// The `count` entries of `scored` with the smallest scores (in no particular order).
fn nearest<T: Copy>(scored: impl Iterator<Item = (f64, T)>, count: usize) -> Vec<(f64, T)> {
    let mut best: Vec<(f64, T)> = Vec::with_capacity(count + 1);
    for item in scored {
        if best.len() < count {
            best.push(item);
        } else if let Some(worst) = best
            .iter_mut()
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .filter(|w| item.0 < w.0)
        {
            *worst = item;
        }
    }
    best
}

impl<T> PartialEq for Cache<T> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl<T> std::fmt::Debug for Cache<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cache")
    }
}

/// Why a curve or surface can't be made from the data given.
#[derive(Clone, Debug, PartialEq)]
pub struct NurbsError(pub String);

impl std::fmt::Display for NurbsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NurbsError {}

/// Checks a knot vector for `count` control points of `degree`.
fn check_knots(degree: usize, knots: &[f64], count: usize, what: &str) -> Result<(), NurbsError> {
    let err = |m: String| Err(NurbsError(m));
    if degree == 0 || degree > 15 {
        return err(format!("{what}: the degree must be between 1 and 15"));
    }
    if count <= degree {
        return err(format!(
            "{what}: a curve of degree {degree} needs at least {} control points",
            degree + 1
        ));
    }
    if knots.len() != count + degree + 1 {
        return err(format!(
            "{what}: {count} control points of degree {degree} need {} knots, not {}",
            count + degree + 1,
            knots.len()
        ));
    }
    if knots.iter().any(|k| !k.is_finite()) || knots.windows(2).any(|w| w[1] < w[0]) {
        return err(format!("{what}: the knots must be finite and not decrease"));
    }
    let clamped = knots[..=degree].iter().all(|&k| k == knots[0])
        && knots[count..].iter().all(|&k| k == knots[count]);
    if !clamped {
        return err(format!(
            "{what}: the knot vector must be clamped (its end knots repeated {} times)",
            degree + 1
        ));
    }
    if knots[count] - knots[0] <= 0.0 {
        return err(format!("{what}: the knots span no range"));
    }
    // An interior knot repeated more than `degree` times would break the curve apart.
    let mut run = 1;
    for i in degree + 2..count {
        run = if knots[i] == knots[i - 1] { run + 1 } else { 1 };
        if run > degree {
            return err(format!("{what}: an interior knot is repeated too often"));
        }
    }
    Ok(())
}

/// The knot span containing `t`: the index `i` with `knots[i] <= t < knots[i + 1]`
/// (the last span for `t` at the end).
fn find_span(degree: usize, knots: &[f64], count: usize, t: f64) -> usize {
    if t >= knots[count] {
        // The last span with a length.
        let mut i = count - 1;
        while i > degree && knots[i] == knots[count] {
            i -= 1;
        }
        return i;
    }
    if t <= knots[degree] {
        let mut i = degree;
        while i + 1 < count && knots[i + 1] == knots[degree] {
            i += 1;
        }
        return i;
    }
    let (mut lo, mut hi) = (degree, count);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if t < knots[mid] {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    lo
}

/// The most derivatives the evaluators compute.
const MAX_DERIVATIVES: usize = 2;

/// The non-zero basis functions at `t` in span `span` and their derivatives up to
/// `MAX_DERIVATIVES`: `out[k][j]` is the `k`-th derivative of basis `span − degree + j`.
fn basis(degree: usize, knots: &[f64], span: usize, t: f64) -> [Vec<f64>; MAX_DERIVATIVES + 1] {
    let p = degree;
    let mut ndu = vec![vec![0.0; p + 1]; p + 1];
    let mut left = vec![0.0; p + 1];
    let mut right = vec![0.0; p + 1];
    ndu[0][0] = 1.0;
    for j in 1..=p {
        left[j] = t - knots[span + 1 - j];
        right[j] = knots[span + j] - t;
        let mut saved = 0.0;
        for r in 0..j {
            ndu[j][r] = right[r + 1] + left[j - r];
            let temp = if ndu[j][r] != 0.0 {
                ndu[r][j - 1] / ndu[j][r]
            } else {
                0.0
            };
            ndu[r][j] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        ndu[j][j] = saved;
    }
    let mut out = [vec![0.0; p + 1], vec![0.0; p + 1], vec![0.0; p + 1]];
    for j in 0..=p {
        out[0][j] = ndu[j][p];
    }
    let mut a = vec![vec![0.0; p + 1]; 2];
    for r in 0..=p {
        let (mut s1, mut s2) = (0, 1);
        a[0][0] = 1.0;
        for k in 1..=MAX_DERIVATIVES.min(p) {
            let mut d = 0.0;
            let rk = r as isize - k as isize;
            let pk = p - k;
            if r >= k {
                a[s2][0] = if ndu[pk + 1][rk as usize] != 0.0 {
                    a[s1][0] / ndu[pk + 1][rk as usize]
                } else {
                    0.0
                };
                d = a[s2][0] * ndu[rk as usize][pk];
            }
            let j1 = if rk >= -1 { 1 } else { (-rk) as usize };
            let j2 = if r as isize - 1 <= pk as isize {
                k - 1
            } else {
                p - r
            };
            for j in j1..=j2 {
                let idx = (rk + j as isize) as usize;
                a[s2][j] = if ndu[pk + 1][idx] != 0.0 {
                    (a[s1][j] - a[s1][j - 1]) / ndu[pk + 1][idx]
                } else {
                    0.0
                };
                d += a[s2][j] * ndu[idx][pk];
            }
            if r <= pk {
                a[s2][k] = if ndu[pk + 1][r] != 0.0 {
                    -a[s1][k - 1] / ndu[pk + 1][r]
                } else {
                    0.0
                };
                d += a[s2][k] * ndu[r][pk];
            }
            out[k][r] = d;
            std::mem::swap(&mut s1, &mut s2);
        }
    }
    // The factors p! / (p − k)!.
    let mut factor = p as f64;
    for (k, row) in out.iter_mut().enumerate().skip(1) {
        if k > p {
            break;
        }
        for v in row.iter_mut() {
            *v *= factor;
        }
        factor *= (p - k) as f64;
    }
    out
}

/// A point and weight as homogeneous coordinates `(w x, w y, w z, w)`.
fn homogeneous(p: DVec3, w: f64) -> DVec4 {
    (p * w).extend(w)
}

/// The point, first and second derivative of a rational curve from the derivatives of
/// its homogeneous form.
fn rational(a: [DVec4; 3]) -> [DVec3; 3] {
    let w = a[0].w;
    let c0 = a[0].truncate() / w;
    let c1 = (a[1].truncate() - c0 * a[1].w) / w;
    let c2 = (a[2].truncate() - c1 * (2.0 * a[1].w) - c0 * a[2].w) / w;
    [c0, c1, c2]
}

/// Inserts `t` once into a knot vector with homogeneous control points (Boehm's
/// algorithm). The curve is unchanged.
fn insert_knot(degree: usize, knots: &mut Vec<f64>, points: &mut Vec<DVec4>, t: f64) {
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

/// Knots this close are the same knot.
const SAME_KNOT: f64 = 1e-12;

/// How often `t` occurs in `knots`.
fn multiplicity(knots: &[f64], t: f64) -> usize {
    knots
        .iter()
        .filter(|k| (**k - t).abs() <= SAME_KNOT)
        .count()
}

/// A NURBS curve.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NurbsCurve {
    degree: usize,
    knots: Vec<f64>,
    points: Vec<DVec3>,
    weights: Option<Vec<f64>>,
    #[serde(skip)]
    samples: Cache<CurveCache>,
}

impl NurbsCurve {
    /// A curve from its definition. The knot vector must be clamped; weights, if given,
    /// positive.
    pub fn new(
        degree: usize,
        knots: Vec<f64>,
        points: Vec<DVec3>,
        weights: Option<Vec<f64>>,
    ) -> Result<Self, NurbsError> {
        check_knots(degree, &knots, points.len(), "curve")?;
        if points.iter().any(|p| !p.is_finite()) {
            return Err(NurbsError(
                "curve: a control point is not finite".to_owned(),
            ));
        }
        if let Some(w) = &weights
            && (w.len() != points.len() || w.iter().any(|w| !(w.is_finite() && *w > 0.0)))
        {
            return Err(NurbsError(
                "curve: there must be one positive weight per control point".to_owned(),
            ));
        }
        // All weights 1 is a plain B-spline.
        let weights = weights.filter(|w| w.iter().any(|w| (*w - 1.0).abs() > 1e-15));
        Ok(Self {
            degree,
            knots,
            points,
            weights,
            samples: Cache::default(),
        })
    }

    /// The straight segment from `a` to `b`, as a curve of `degree` (1 or more) over
    /// `[0, 1]`.
    pub fn line(a: DVec3, b: DVec3, degree: usize) -> Self {
        let degree = degree.max(1);
        let mut knots = vec![0.0; degree + 1];
        knots.extend(std::iter::repeat_n(1.0, degree + 1));
        let points = (0..=degree)
            .map(|i| a.lerp(b, i as f64 / degree as f64))
            .collect();
        Self {
            degree,
            knots,
            points,
            weights: None,
            samples: Cache::default(),
        }
    }

    /// The circular arc in `frame`'s XY plane, of `radius` about its origin, from angle
    /// `start` over `sweep` (radians, up to a full turn either way), as a rational
    /// quadratic over `[0, 1]`. The parameter is close to, but not exactly, proportional
    /// to the angle.
    pub fn arc(frame: &Frame, radius: f64, start: f64, sweep: f64) -> Self {
        // Pieces of at most a quarter turn, each a rational quadratic Bézier.
        let pieces = ((sweep.abs() / std::f64::consts::FRAC_PI_2 - 1e-9).ceil() as usize).max(1);
        let step = sweep / pieces as f64;
        let w = (0.5 * step).cos();
        let at = |angle: f64, r: f64| {
            let (s, c) = angle.sin_cos();
            frame.to_world(DVec3::new(c * r, s * r, 0.0))
        };
        let mut points = vec![at(start, radius)];
        let mut weights = vec![1.0];
        let mut knots = vec![0.0; 3];
        for i in 0..pieces {
            let a0 = start + step * i as f64;
            points.push(at(a0 + 0.5 * step, radius / w));
            weights.push(w);
            points.push(at(a0 + step, radius));
            weights.push(1.0);
            let k = (i + 1) as f64 / pieces as f64;
            knots.extend([k, k]);
        }
        knots.push(1.0);
        Self {
            degree: 2,
            knots,
            points,
            weights: Some(weights),
            samples: Cache::default(),
        }
    }

    /// A smooth curve through `through`, in order: a cubic (or lower, for fewer than
    /// four points) B-spline over `[0, 1]` with chord-length parameters.
    pub fn interpolate(through: &[DVec3]) -> Result<Self, NurbsError> {
        let n = through.len();
        if n < 2 {
            return Err(NurbsError(
                "a curve needs at least two points to pass through".to_owned(),
            ));
        }
        let degree = (n - 1).min(3);
        let (params, knots) = interpolation_knots(through, degree)?;
        let points = solve_interpolation(degree, &knots, &params, through)?;
        Self::new(degree, knots, points, None)
    }

    pub fn degree(&self) -> usize {
        self.degree
    }

    pub fn knots(&self) -> &[f64] {
        &self.knots
    }

    pub fn control_points(&self) -> &[DVec3] {
        &self.points
    }

    /// The weights, if the curve is rational.
    pub fn weights(&self) -> Option<&[f64]> {
        self.weights.as_deref()
    }

    fn weight(&self, i: usize) -> f64 {
        self.weights.as_ref().map_or(1.0, |w| w[i])
    }

    /// The parameter range.
    pub fn domain(&self) -> (f64, f64) {
        (self.knots[0], self.knots[self.knots.len() - 1])
    }

    /// The point and the first two derivatives at `t` (clamped to the domain).
    // The indices are the formula's: basis function `j` of derivative `k`.
    #[allow(clippy::needless_range_loop)]
    pub fn evaluate(&self, t: f64) -> [DVec3; 3] {
        let (lo, hi) = self.domain();
        let t = t.clamp(lo, hi);
        let span = find_span(self.degree, &self.knots, self.points.len(), t);
        let n = basis(self.degree, &self.knots, span, t);
        let mut a = [DVec4::ZERO; 3];
        for j in 0..=self.degree {
            let i = span - self.degree + j;
            let h = homogeneous(self.points[i], self.weight(i));
            for k in 0..3 {
                a[k] += h * n[k][j];
            }
        }
        rational(a)
    }

    pub fn point(&self, t: f64) -> DVec3 {
        self.evaluate(t)[0]
    }

    /// The parameter of the point of the curve closest to `p`.
    pub fn param(&self, p: DVec3) -> f64 {
        let (lo, hi) = self.domain();
        let samples = self.samples.0.samples.get_or_init(|| {
            // A few per span: enough for Newton to start in the right basin.
            let spans = self.knots.windows(2).filter(|w| w[1] > w[0]).count();
            let n = (spans * (2 * self.degree + 2)).clamp(8, 512);
            (0..=n)
                .map(|i| {
                    let t = lo + (hi - lo) * i as f64 / n as f64;
                    (t, self.point(t))
                })
                .collect()
        });
        let order = nearest(samples.iter().map(|(t, q)| (q.distance_squared(p), *t)), 3);
        let mut best = (f64::INFINITY, lo);
        for &(_, start) in &order {
            let mut t = start;
            for _ in 0..32 {
                let [c, d1, d2] = self.evaluate(t);
                let r = c - p;
                let f = r.dot(d1);
                let df = d1.length_squared() + r.dot(d2);
                if df.abs() <= f64::MIN_POSITIVE {
                    break;
                }
                // Gauss–Newton where the full step would head for a maximum.
                let step = if df > 0.0 {
                    f / df
                } else {
                    f / d1.length_squared()
                };
                let next = (t - step).clamp(lo, hi);
                let moved = (next - t).abs();
                t = next;
                if moved <= 1e-15 * (hi - lo).max(1.0) {
                    break;
                }
            }
            let d = self.point(t).distance_squared(p);
            if d < best.0 {
                best = (d, t);
            }
        }
        best.1
    }

    /// The curve placed by `frame`.
    pub fn transformed(&self, frame: &Frame) -> Self {
        Self {
            points: self.points.iter().map(|p| frame.to_world(*p)).collect(),
            samples: Cache::default(),
            ..self.clone()
        }
    }

    /// The curve with every point mapped by `f`, which must be an affine map (a mirror,
    /// a scale): those map control points to control points.
    pub fn mapped(&self, f: impl Fn(DVec3) -> DVec3) -> Self {
        Self {
            points: self.points.iter().map(|p| f(*p)).collect(),
            samples: Cache::default(),
            ..self.clone()
        }
    }

    /// The same curve run the other way: `point(t)` becomes `point(lo + hi − t)`.
    pub fn reversed(&self) -> Self {
        let (lo, hi) = self.domain();
        Self {
            degree: self.degree,
            knots: self.knots.iter().rev().map(|k| lo + hi - k).collect(),
            points: self.points.iter().rev().copied().collect(),
            weights: self
                .weights
                .as_ref()
                .map(|w| w.iter().rev().copied().collect()),
            samples: Cache::default(),
        }
    }

    /// Bounds of the control points, which hold the whole curve.
    pub fn bounds(&self) -> Aabb {
        Aabb::from_points(self.points.iter().copied())
    }

    /// The largest second derivative over the curve, estimated from samples: how fast
    /// it bends per unit of parameter squared.
    pub fn bend(&self) -> f64 {
        *self.samples.0.bend.get_or_init(|| {
            let (lo, hi) = self.domain();
            (0..=32)
                .map(|i| self.evaluate(lo + (hi - lo) * f64::from(i) / 32.0)[2].length())
                .fold(0.0, f64::max)
        })
    }

    fn homogeneous_points(&self) -> Vec<DVec4> {
        (0..self.points.len())
            .map(|i| homogeneous(self.points[i], self.weight(i)))
            .collect()
    }

    fn from_homogeneous(degree: usize, knots: Vec<f64>, points: &[DVec4]) -> Self {
        let weights: Vec<f64> = points.iter().map(|h| h.w).collect();
        let rational = weights.iter().any(|w| (*w - 1.0).abs() > 1e-15);
        Self {
            degree,
            knots,
            points: points.iter().map(|h| h.truncate() / h.w).collect(),
            weights: rational.then_some(weights),
            samples: Cache::default(),
        }
    }

    /// The same curve with its parameter range mapped linearly to `[lo, hi]`.
    pub fn reparametrized(&self, lo: f64, hi: f64) -> Self {
        let (a, b) = self.domain();
        let scale = (hi - lo) / (b - a);
        Self {
            knots: self.knots.iter().map(|k| lo + (k - a) * scale).collect(),
            samples: Cache::default(),
            ..self.clone()
        }
    }

    /// The same curve with every knot of `wanted` present at least as often as there.
    fn refined(&self, wanted: &[f64]) -> Self {
        let mut knots = self.knots.clone();
        let mut points = self.homogeneous_points();
        let mut i = 0;
        while i < wanted.len() {
            let t = wanted[i];
            let need = wanted[i..]
                .iter()
                .take_while(|k| (**k - t).abs() <= SAME_KNOT)
                .count();
            let have = multiplicity(&knots, t);
            // Use the curve's own value for a knot it already has, so they stay equal.
            let at = knots
                .iter()
                .copied()
                .find(|k| (*k - t).abs() <= SAME_KNOT)
                .unwrap_or(t);
            for _ in have..need {
                insert_knot(self.degree, &mut knots, &mut points, at);
            }
            i += need;
        }
        Self::from_homogeneous(self.degree, knots, &points)
    }

    /// The same curve as one of degree `degree` (only raising from 1 is needed here: a
    /// straight piece next to curved ones).
    fn raised(&self, degree: usize) -> Result<Self, NurbsError> {
        if self.degree == degree {
            return Ok(self.clone());
        }
        if self.degree == 1 && self.points.len() == 2 && self.weights.is_none() {
            let (lo, hi) = self.domain();
            return Ok(Self::line(self.points[0], self.points[1], degree).reparametrized(lo, hi));
        }
        Err(NurbsError(format!(
            "curves of degree {} and {degree} can't be matched yet",
            self.degree
        )))
    }

    /// A curve from a knot vector that need not be clamped (a uniform or "periodic
    /// style" one, as other systems write closed curves). The curve is the part over
    /// `[knots[degree], knots[points.len()]]`, where the basis functions add up to one;
    /// knots are inserted at both ends of that range until it is clamped, which leaves
    /// the shape and the parametrisation as they were.
    pub fn from_unclamped(
        degree: usize,
        knots: Vec<f64>,
        points: Vec<DVec3>,
        weights: Option<Vec<f64>>,
    ) -> Result<Self, NurbsError> {
        check_unclamped(degree, &knots, points.len(), "curve")?;
        if is_clamped(degree, &knots, points.len()) {
            return Self::new(degree, knots, points, weights);
        }
        let line = homogeneous_checked(&points, weights.as_deref(), "curve")?;
        let mut strands = Strands {
            degree,
            knots,
            lines: vec![line],
        };
        strands.clamp();
        let (points, weights) = from_homogeneous_points(&strands.lines[0]);
        Self::new(degree, strands.knots, points, Some(weights))
    }

    fn strands(&self) -> Strands {
        Strands {
            degree: self.degree,
            knots: self.knots.clone(),
            lines: vec![self.homogeneous_points()],
        }
    }

    /// The part of the curve between the parameters `a < b` (limited to the domain),
    /// as a curve of its own with the same parametrisation.
    pub fn sub_curve(&self, a: f64, b: f64) -> Result<Self, NurbsError> {
        let s = self.strands().between(a, b)?;
        Ok(Self::from_homogeneous(s.degree, s.knots, &s.lines[0]))
    }

    /// This curve followed by `next`, which must be of the same degree and start where
    /// this one ends. The parameter runs on from this curve's: `next` is shifted to
    /// start at this curve's last knot. The two meet at a knot of full multiplicity.
    pub fn joined(&self, next: &Self) -> Result<Self, NurbsError> {
        let s = self.strands().join(next.strands())?;
        Ok(Self::from_homogeneous(s.degree, s.knots, &s.lines[0]))
    }

    /// Whether the curve ends where it starts, to within `tolerance`.
    pub fn is_closed(&self, tolerance: f64) -> bool {
        let (lo, hi) = self.domain();
        self.point(lo).distance(self.point(hi)) <= tolerance
    }
}

/// Checks a knot vector that need not be clamped, for `count` control points.
fn check_unclamped(
    degree: usize,
    knots: &[f64],
    count: usize,
    what: &str,
) -> Result<(), NurbsError> {
    let err = |m: String| Err(NurbsError(m));
    if degree == 0 || degree > 15 {
        return err(format!("{what}: the degree must be between 1 and 15"));
    }
    if count <= degree {
        return err(format!(
            "{what}: a curve of degree {degree} needs at least {} control points",
            degree + 1
        ));
    }
    if knots.len() != count + degree + 1 {
        return err(format!(
            "{what}: {count} control points of degree {degree} need {} knots, not {}",
            count + degree + 1,
            knots.len()
        ));
    }
    if knots.iter().any(|k| !k.is_finite()) || knots.windows(2).any(|w| w[1] < w[0]) {
        return err(format!("{what}: the knots must be finite and not decrease"));
    }
    if knots[count] - knots[degree] <= 0.0 {
        return err(format!("{what}: the knots span no range"));
    }
    Ok(())
}

/// Whether both ends of a (checked) knot vector are repeated `degree + 1` times.
fn is_clamped(degree: usize, knots: &[f64], count: usize) -> bool {
    knots[..=degree].iter().all(|&k| k == knots[0])
        && knots[count..].iter().all(|&k| k == knots[count])
}

/// Homogeneous control points, once the weights are known to be usable.
fn homogeneous_checked(
    points: &[DVec3],
    weights: Option<&[f64]>,
    what: &str,
) -> Result<Vec<DVec4>, NurbsError> {
    if points.iter().any(|p| !p.is_finite()) {
        return Err(NurbsError(format!("{what}: a control point is not finite")));
    }
    if let Some(w) = weights
        && (w.len() != points.len() || w.iter().any(|w| !(w.is_finite() && *w > 0.0)))
    {
        return Err(NurbsError(format!(
            "{what}: there must be one positive weight per control point"
        )));
    }
    Ok(points
        .iter()
        .enumerate()
        .map(|(i, p)| homogeneous(*p, weights.map_or(1.0, |w| w[i])))
        .collect())
}

fn from_homogeneous_points(points: &[DVec4]) -> (Vec<DVec3>, Vec<f64>) {
    (
        points.iter().map(|h| h.truncate() / h.w).collect(),
        points.iter().map(|h| h.w).collect(),
    )
}

/// Homogeneous control points of several curves that share a degree and a knot vector:
/// one for a curve, a surface's rows or columns for one of its directions. Knot
/// insertion, trimming and joining act on all of them alike.
struct Strands {
    degree: usize,
    knots: Vec<f64>,
    lines: Vec<Vec<DVec4>>,
}

impl Strands {
    fn insert(&mut self, t: f64) {
        let mut after = self.knots.clone();
        for line in &mut self.lines {
            let mut knots = self.knots.clone();
            insert_knot(self.degree, &mut knots, line, t);
            after = knots;
        }
        self.knots = after;
    }

    fn reverse(&mut self) {
        self.knots = self.knots.iter().rev().map(|k| -k).collect();
        for line in &mut self.lines {
            line.reverse();
        }
    }

    /// Clamps the start of the domain: its knot gets multiplicity `degree`, and the
    /// knots and control points before it, which no longer matter, are dropped.
    fn clamp_start(&mut self) {
        let p = self.degree;
        let t = self.knots[p];
        while self.knots.iter().filter(|k| **k == t).count() < p {
            self.insert(t);
        }
        let last = self.knots.iter().rposition(|k| *k == t).unwrap_or(p);
        let drop = last.saturating_sub(p);
        self.knots.drain(..drop);
        self.knots[0] = t;
        for line in &mut self.lines {
            line.drain(..drop);
        }
    }

    fn clamp(&mut self) {
        self.clamp_start();
        self.reverse();
        self.clamp_start();
        self.reverse();
    }

    /// The parts before and after `t`, which must be strictly inside the (clamped)
    /// domain.
    fn split(mut self, t: f64) -> (Self, Self) {
        let p = self.degree;
        while self.knots.iter().filter(|k| **k == t).count() < p {
            self.insert(t);
        }
        let last = self.knots.iter().rposition(|k| *k == t).unwrap_or(p);
        let mut left_knots = self.knots[..=last].to_vec();
        left_knots.push(t);
        let mut right_knots = vec![t];
        right_knots.extend_from_slice(&self.knots[last + 1 - p..]);
        let left = Self {
            degree: p,
            knots: left_knots,
            lines: self.lines.iter().map(|l| l[..=last - p].to_vec()).collect(),
        };
        let right = Self {
            degree: p,
            knots: right_knots,
            lines: self.lines.iter().map(|l| l[last - p..].to_vec()).collect(),
        };
        (left, right)
    }

    /// The part between `a` and `b`.
    fn between(mut self, a: f64, b: f64) -> Result<Self, NurbsError> {
        let (lo, hi) = (self.knots[0], self.knots[self.knots.len() - 1]);
        let same = 1e-12 * (hi - lo);
        // A cut next to a knot is at the knot: no sliver of a span is left.
        let snap = |t: f64| {
            self.knots
                .iter()
                .copied()
                .find(|k| (*k - t).abs() <= same)
                .unwrap_or(t)
        };
        // (`max` and `min` would swallow a NaN.)
        let limit = |t: f64| if t.is_finite() { t.clamp(lo, hi) } else { t };
        let (a, b) = (snap(limit(a)), snap(limit(b)));
        if !(a.is_finite() && b.is_finite() && b - a > same) {
            return Err(NurbsError(
                "the part to keep has no length in the parameter".to_owned(),
            ));
        }
        if a > lo {
            self = self.split(a).1;
        }
        if b < hi {
            self = self.split(b).0;
        }
        Ok(self)
    }

    /// These followed by `next`, shifted to start at the last knot here. `next` is
    /// scaled (which does not change it) so the weights agree where the two meet.
    fn join(mut self, next: Self) -> Result<Self, NurbsError> {
        if self.degree != next.degree || self.lines.len() != next.lines.len() {
            return Err(NurbsError(
                "pieces of different degrees or sizes can't be joined".to_owned(),
            ));
        }
        let p = self.degree;
        let mut scale = 0.0;
        for (mine, theirs) in self.lines.iter().zip(&next.lines) {
            scale += mine[mine.len() - 1].w / theirs[0].w;
        }
        scale /= self.lines.len() as f64;
        if !(scale.is_finite() && scale > 0.0) {
            return Err(NurbsError(
                "the pieces' weights can't be matched".to_owned(),
            ));
        }
        let shift = self.knots[self.knots.len() - 1] - next.knots[0];
        self.knots.pop();
        self.knots
            .extend(next.knots[p + 1..].iter().map(|k| k + shift));
        for (mine, theirs) in self.lines.iter_mut().zip(&next.lines) {
            mine.extend(theirs[1..].iter().map(|h| *h * scale));
        }
        Ok(self)
    }
}

/// Makes curves compatible: the same degree and the same knot vector, over `[0, 1]`.
pub fn compatible(curves: &[NurbsCurve]) -> Result<Vec<NurbsCurve>, NurbsError> {
    let degree = curves.iter().map(NurbsCurve::degree).max().unwrap_or(1);
    let raised: Vec<NurbsCurve> = curves
        .iter()
        .map(|c| c.raised(degree).map(|c| c.reparametrized(0.0, 1.0)))
        .collect::<Result<_, _>>()?;
    // The union of the knot vectors, each knot as often as its most frequent use.
    let mut all: Vec<f64> = Vec::new();
    for c in &raised {
        let mut i = 0;
        while i < c.knots.len() {
            let t = c.knots[i];
            let count = multiplicity(&c.knots, t);
            let have = multiplicity(&all, t);
            all.extend(std::iter::repeat_n(t, count.saturating_sub(have)));
            i += count.max(1);
        }
    }
    all.sort_by(f64::total_cmp);
    Ok(raised.iter().map(|c| c.refined(&all)).collect())
}

/// Chord-length parameters in `[0, 1]` for points to interpolate, and the knot vector
/// that goes with them (knot averaging).
fn interpolation_knots(
    through: &[DVec3],
    degree: usize,
) -> Result<(Vec<f64>, Vec<f64>), NurbsError> {
    let n = through.len();
    let chords: Vec<f64> = through.windows(2).map(|w| w[0].distance(w[1])).collect();
    let total: f64 = chords.iter().sum();
    if chords.iter().any(|c| *c <= 1e-12) || !total.is_finite() {
        return Err(NurbsError(
            "two points to pass through are the same".to_owned(),
        ));
    }
    let mut params = vec![0.0];
    for c in &chords {
        let last = params[params.len() - 1];
        params.push(last + c / total);
    }
    params[n - 1] = 1.0;
    let mut knots = vec![0.0; degree + 1];
    for j in 1..n - degree {
        knots.push(params[j..j + degree].iter().sum::<f64>() / degree as f64);
    }
    knots.extend(std::iter::repeat_n(1.0, degree + 1));
    Ok((params, knots))
}

/// The control points of the B-spline with these knots that passes through `through`
/// at `params`.
fn solve_interpolation<T>(
    degree: usize,
    knots: &[f64],
    params: &[f64],
    through: &[T],
) -> Result<Vec<T>, NurbsError>
where
    T: Copy + std::ops::Mul<f64, Output = T> + std::ops::Sub<Output = T>,
{
    let n = through.len();
    let mut matrix = vec![vec![0.0; n]; n];
    for (row, &t) in params.iter().enumerate() {
        let span = find_span(degree, knots, n, t);
        let b = basis(degree, knots, span, t);
        for j in 0..=degree {
            matrix[row][span - degree + j] = b[0][j];
        }
    }
    // Gaussian elimination with partial pivoting: the system is small and banded.
    let mut rhs: Vec<T> = through.to_vec();
    for col in 0..n {
        let pivot = (col..n)
            .max_by(|&a, &b| matrix[a][col].abs().total_cmp(&matrix[b][col].abs()))
            .unwrap_or(col);
        if matrix[pivot][col].abs() <= 1e-14 {
            return Err(NurbsError(
                "the points can't be interpolated (they are too unevenly spaced)".to_owned(),
            ));
        }
        matrix.swap(col, pivot);
        rhs.swap(col, pivot);
        for row in col + 1..n {
            let f = matrix[row][col] / matrix[col][col];
            if f != 0.0 {
                let (above, below) = matrix.split_at_mut(row);
                for (x, pivot) in below[0][col..n].iter_mut().zip(&above[col][col..n]) {
                    *x -= f * pivot;
                }
                rhs[row] = rhs[row] - rhs[col] * f;
            }
        }
    }
    let mut out = rhs.clone();
    for row in (0..n).rev() {
        let mut v = rhs[row];
        for k in row + 1..n {
            v = v - out[k] * matrix[row][k];
        }
        out[row] = v * (1.0 / matrix[row][row]);
    }
    Ok(out)
}

/// A NURBS surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NurbsSurface {
    degree_u: usize,
    degree_v: usize,
    knots_u: Vec<f64>,
    knots_v: Vec<f64>,
    /// Row by row in `u`: point `(i, j)` is at `i * count_v + j`.
    points: Vec<DVec3>,
    weights: Option<Vec<f64>>,
    #[serde(skip)]
    samples: Cache<SurfaceCache>,
}

impl NurbsSurface {
    /// A surface from its definition: `points` (and `weights`) row by row in `u`.
    pub fn new(
        degree_u: usize,
        degree_v: usize,
        knots_u: Vec<f64>,
        knots_v: Vec<f64>,
        points: Vec<DVec3>,
        weights: Option<Vec<f64>>,
    ) -> Result<Self, NurbsError> {
        let count_u = knots_u.len().saturating_sub(degree_u + 1);
        let count_v = knots_v.len().saturating_sub(degree_v + 1);
        check_knots(degree_u, &knots_u, count_u, "surface (u)")?;
        check_knots(degree_v, &knots_v, count_v, "surface (v)")?;
        if points.len() != count_u * count_v {
            return Err(NurbsError(format!(
                "surface: its knots call for {count_u} × {count_v} control points, not {}",
                points.len()
            )));
        }
        if points.iter().any(|p| !p.is_finite()) {
            return Err(NurbsError(
                "surface: a control point is not finite".to_owned(),
            ));
        }
        if let Some(w) = &weights
            && (w.len() != points.len() || w.iter().any(|w| !(w.is_finite() && *w > 0.0)))
        {
            return Err(NurbsError(
                "surface: there must be one positive weight per control point".to_owned(),
            ));
        }
        let weights = weights.filter(|w| w.iter().any(|w| (*w - 1.0).abs() > 1e-15));
        Ok(Self {
            degree_u,
            degree_v,
            knots_u,
            knots_v,
            points,
            weights,
            samples: Cache::default(),
        })
    }

    /// Where sections sit along `v` if they are spaced by the distance between them:
    /// the average, over `rails` (the points one place on each section moves through),
    /// of each rail's chord lengths, from 0 to 1.
    pub fn section_params(rails: &[Vec<DVec3>]) -> Result<Vec<f64>, NurbsError> {
        let count = rails.first().map_or(0, Vec::len);
        if count < 2 || rails.iter().any(|r| r.len() != count) {
            return Err(NurbsError(
                "a surface needs at least two sections".to_owned(),
            ));
        }
        let mut params = vec![0.0; count];
        let mut used = 0;
        for rail in rails {
            let chords: Vec<f64> = rail.windows(2).map(|w| w[0].distance(w[1])).collect();
            let total: f64 = chords.iter().sum();
            if total <= 1e-12 {
                continue; // the sections share this point
            }
            let mut at = 0.0;
            for (k, c) in chords.iter().enumerate() {
                at += c / total;
                params[k + 1] += at;
            }
            used += 1;
        }
        if used == 0 {
            return Err(NurbsError("the sections are all the same curve".to_owned()));
        }
        for p in &mut params {
            *p /= f64::from(used);
        }
        params[count - 1] = 1.0;
        if params.windows(2).any(|w| w[1] - w[0] <= 1e-12) {
            return Err(NurbsError("two sections are in the same place".to_owned()));
        }
        Ok(params)
    }

    /// The surface through `sections` (curves along `u`, in order along `v`): ruled
    /// between two, a smooth cubic (or quadratic, for three) through more. `v` runs over
    /// `[0, 1]`, by the distance between the sections.
    pub fn skin(sections: &[NurbsCurve]) -> Result<Self, NurbsError> {
        if sections.len() < 2 {
            return Err(NurbsError(
                "a surface needs at least two sections".to_owned(),
            ));
        }
        let made = compatible(sections)?;
        let rails: Vec<Vec<DVec3>> = (0..made[0].points.len())
            .map(|i| made.iter().map(|s| s.points[i]).collect())
            .collect();
        let params = Self::section_params(&rails)?;
        Self::skin_at(sections, &params)
    }

    /// [`NurbsSurface::skin`] with the sections at the given `v` (increasing from 0 to
    /// 1). Surfaces skinned side by side with the same `params` meet exactly along the
    /// curve through their sections' shared ends.
    pub fn skin_at(sections: &[NurbsCurve], params: &[f64]) -> Result<Self, NurbsError> {
        if sections.len() < 2 || params.len() != sections.len() {
            return Err(NurbsError(
                "a surface needs at least two sections, and a place for each".to_owned(),
            ));
        }
        let sections = compatible(sections)?;
        let count_u = sections[0].points.len();
        let count_v = sections.len();
        let degree_v = (count_v - 1).min(3);
        let mut knots_v = vec![0.0; degree_v + 1];
        for j in 1..count_v - degree_v {
            knots_v.push(params[j..j + degree_v].iter().sum::<f64>() / degree_v as f64);
        }
        knots_v.extend(std::iter::repeat_n(1.0, degree_v + 1));
        // Interpolate each column in homogeneous coordinates.
        let mut grid = vec![DVec4::ZERO; count_u * count_v];
        for i in 0..count_u {
            let column: Vec<DVec4> = sections
                .iter()
                .map(|s| homogeneous(s.points[i], s.weight(i)))
                .collect();
            let solved = solve_interpolation(degree_v, &knots_v, params, &column)?;
            for (j, h) in solved.into_iter().enumerate() {
                grid[i * count_v + j] = h;
            }
        }
        if grid.iter().any(|h| h.w.is_nan() || h.w <= 1e-9) {
            return Err(NurbsError(
                "the sections can't be joined smoothly (their weights differ too much)".to_owned(),
            ));
        }
        let weights: Vec<f64> = grid.iter().map(|h| h.w).collect();
        Self::new(
            sections[0].degree,
            degree_v,
            sections[0].knots.clone(),
            knots_v,
            grid.iter().map(|h| h.truncate() / h.w).collect(),
            Some(weights),
        )
    }

    pub fn degrees(&self) -> (usize, usize) {
        (self.degree_u, self.degree_v)
    }

    pub fn knots(&self) -> (&[f64], &[f64]) {
        (&self.knots_u, &self.knots_v)
    }

    /// Control points in `u` and in `v`.
    pub fn counts(&self) -> (usize, usize) {
        (
            self.knots_u.len() - self.degree_u - 1,
            self.knots_v.len() - self.degree_v - 1,
        )
    }

    pub fn control_points(&self) -> &[DVec3] {
        &self.points
    }

    pub fn weights(&self) -> Option<&[f64]> {
        self.weights.as_deref()
    }

    fn weight(&self, i: usize) -> f64 {
        self.weights.as_ref().map_or(1.0, |w| w[i])
    }

    /// The parameter rectangle: its lower and upper corner.
    pub fn domain(&self) -> (DVec2, DVec2) {
        (
            DVec2::new(self.knots_u[0], self.knots_v[0]),
            DVec2::new(
                self.knots_u[self.knots_u.len() - 1],
                self.knots_v[self.knots_v.len() - 1],
            ),
        )
    }

    /// The distinct knots in each direction: where the surface's pieces meet.
    pub fn breaks(&self) -> (Vec<f64>, Vec<f64>) {
        let distinct = |knots: &[f64]| {
            let mut out: Vec<f64> = Vec::new();
            for &k in knots {
                if out.last().is_none_or(|l| k > *l) {
                    out.push(k);
                }
            }
            out
        };
        (distinct(&self.knots_u), distinct(&self.knots_v))
    }

    /// Derivatives of the homogeneous surface: `out[a][b]` is `∂^(a+b) / ∂u^a ∂v^b`.
    fn homogeneous_derivatives(&self, uv: DVec2) -> [[DVec4; 3]; 3] {
        let (lo, hi) = self.domain();
        let uv = uv.clamp(lo, hi);
        let (count_u, count_v) = self.counts();
        let su = find_span(self.degree_u, &self.knots_u, count_u, uv.x);
        let sv = find_span(self.degree_v, &self.knots_v, count_v, uv.y);
        let nu = basis(self.degree_u, &self.knots_u, su, uv.x);
        let nv = basis(self.degree_v, &self.knots_v, sv, uv.y);
        let mut out = [[DVec4::ZERO; 3]; 3];
        // The indices are the formula's: basis functions `i`, `j` of derivatives `a`, `b`.
        #[allow(clippy::needless_range_loop)]
        for i in 0..=self.degree_u {
            let row = (su - self.degree_u + i) * count_v;
            // The row's sums over v, for each v-derivative.
            let mut along = [DVec4::ZERO; 3];
            for j in 0..=self.degree_v {
                let index = row + sv - self.degree_v + j;
                let h = homogeneous(self.points[index], self.weight(index));
                for (b, sum) in along.iter_mut().enumerate() {
                    *sum += h * nv[b][j];
                }
            }
            for a in 0..3 {
                for b in 0..3 - a {
                    out[a][b] += along[b] * nu[a][i];
                }
            }
        }
        out
    }

    /// The point and the derivatives `[S, Su, Sv, Suu, Suv, Svv]` at `uv` (clamped to
    /// the domain).
    pub fn evaluate(&self, uv: DVec2) -> [DVec3; 6] {
        let a = self.homogeneous_derivatives(uv);
        let w = a[0][0].w;
        let s = a[0][0].truncate() / w;
        let su = (a[1][0].truncate() - s * a[1][0].w) / w;
        let sv = (a[0][1].truncate() - s * a[0][1].w) / w;
        let suu = (a[2][0].truncate() - su * (2.0 * a[1][0].w) - s * a[2][0].w) / w;
        let svv = (a[0][2].truncate() - sv * (2.0 * a[0][1].w) - s * a[0][2].w) / w;
        let suv = (a[1][1].truncate() - su * a[0][1].w - sv * a[1][0].w - s * a[1][1].w) / w;
        [s, su, sv, suu, suv, svv]
    }

    pub fn point(&self, uv: DVec2) -> DVec3 {
        let h = self.homogeneous_derivatives(uv)[0][0];
        h.truncate() / h.w
    }

    /// The unit normal `Su × Sv` at `uv`. Where the surface is degenerate (a collapsed
    /// edge) the normal just inside the domain is used.
    pub fn normal(&self, uv: DVec2) -> DVec3 {
        let [_, su, sv, ..] = self.evaluate(uv);
        if let Some(n) = su.cross(sv).try_normalize() {
            return n;
        }
        let (lo, hi) = self.domain();
        let inside = uv + (0.5 * (lo + hi) - uv) * 1e-4;
        let [_, su, sv, ..] = self.evaluate(inside);
        su.cross(sv).normalize_or(DVec3::Z)
    }

    /// How many steps the kept grid of samples takes across the surface, in `u` and `v`.
    fn sample_counts(&self) -> (usize, usize) {
        let (bu, bv) = self.breaks();
        let per = |breaks: &[f64], degree: usize| ((breaks.len() - 1) * (degree + 2)).clamp(6, 96);
        (per(&bu, self.degree_u), per(&bv, self.degree_v))
    }

    /// A grid of points of the surface, made once and kept: each with its parameters,
    /// and the grid's step in each parameter.
    pub fn samples(&self) -> (&[(DVec2, DVec3)], DVec2) {
        let (lo, hi) = self.domain();
        let (nu, nv) = self.sample_counts();
        let samples = self.samples.0.samples.get_or_init(|| {
            let mut out = Vec::with_capacity((nu + 1) * (nv + 1));
            for i in 0..=nu {
                for j in 0..=nv {
                    let uv =
                        lo + (hi - lo) * DVec2::new(i as f64 / nu as f64, j as f64 / nv as f64);
                    out.push((uv, self.point(uv)));
                }
            }
            out
        });
        (samples, (hi - lo) / DVec2::new(nu as f64, nv as f64))
    }

    /// The parameters of the point of the surface closest to `p`.
    pub fn param(&self, p: DVec3) -> DVec2 {
        let (lo, _) = self.domain();
        let (samples, _) = self.samples();
        // Newton from the nearest few samples, nearest first. A start that lands on the
        // point itself can't be beaten: the rest are skipped.
        let mut order = nearest(
            samples.iter().map(|(uv, q)| (q.distance_squared(p), *uv)),
            4,
        );
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut best = (f64::INFINITY, lo);
        for &(_, start) in &order {
            let uv = self.newton_closest(p, start);
            let d = self.point(uv).distance_squared(p);
            if d < best.0 {
                best = (d, uv);
            }
            if d <= 1e-24 {
                break;
            }
        }
        best.1
    }

    /// The parameters of the point closest to `p` that Newton's method finds from
    /// `start`: the same as [`NurbsSurface::param`] when `start` is near enough, at a
    /// fraction of the cost.
    pub fn param_from(&self, p: DVec3, start: DVec2) -> DVec2 {
        self.newton_closest(p, start)
    }

    /// Newton's method for the closest point to `p`, from `start`, kept in the domain.
    fn newton_closest(&self, p: DVec3, start: DVec2) -> DVec2 {
        let (lo, hi) = self.domain();
        let mut uv = start;
        for _ in 0..40 {
            let [s, su, sv, suu, suv, svv] = self.evaluate(uv);
            let r = s - p;
            // Minimise |S − p|²: gradient g, Hessian H.
            let g = DVec2::new(r.dot(su), r.dot(sv));
            let h11 = su.dot(su) + r.dot(suu);
            let h12 = su.dot(sv) + r.dot(suv);
            let h22 = sv.dot(sv) + r.dot(svv);
            let det = h11 * h22 - h12 * h12;
            let step = if det > 1e-18 * (h11.abs() + h22.abs()).powi(2) && h11 > 0.0 {
                DVec2::new(h22 * g.x - h12 * g.y, h11 * g.y - h12 * g.x) / det
            } else {
                // Gauss–Newton: drop the curvature terms.
                let (a, b, c) = (su.dot(su), su.dot(sv), sv.dot(sv));
                let det = a * c - b * b;
                if det.abs() <= f64::MIN_POSITIVE {
                    break;
                }
                DVec2::new(c * g.x - b * g.y, a * g.y - b * g.x) / det
            };
            let next = (uv - step).clamp(lo, hi);
            let moved = next.distance(uv);
            uv = next;
            if moved <= 1e-15 * (hi - lo).length().max(1.0) {
                break;
            }
        }
        uv
    }

    /// The surface placed by `frame`.
    pub fn transformed(&self, frame: &Frame) -> Self {
        self.mapped(|p| frame.to_world(p))
    }

    /// The surface with every point mapped by `f`, which must be an affine map.
    pub fn mapped(&self, f: impl Fn(DVec3) -> DVec3) -> Self {
        Self {
            points: self.points.iter().map(|p| f(*p)).collect(),
            samples: Cache::default(),
            ..self.clone()
        }
    }

    /// Bounds of the control points, which hold the whole surface.
    pub fn bounds(&self) -> Aabb {
        Aabb::from_points(self.points.iter().copied())
    }

    /// The curve along `u` at a fixed `v` (`along_u`), or along `v` at a fixed `u`.
    pub fn iso_curve(&self, along_u: bool, at: f64) -> NurbsCurve {
        let (count_u, count_v) = self.counts();
        let h = |i: usize, j: usize| {
            let index = i * count_v + j;
            homogeneous(self.points[index], self.weight(index))
        };
        let (degree, knots, points) = if along_u {
            let span = find_span(self.degree_v, &self.knots_v, count_v, at);
            let b = basis(self.degree_v, &self.knots_v, span, at);
            let points: Vec<DVec4> = (0..count_u)
                .map(|i| {
                    (0..=self.degree_v)
                        .map(|j| h(i, span - self.degree_v + j) * b[0][j])
                        .fold(DVec4::ZERO, |a, x| a + x)
                })
                .collect();
            (self.degree_u, self.knots_u.clone(), points)
        } else {
            let span = find_span(self.degree_u, &self.knots_u, count_u, at);
            let b = basis(self.degree_u, &self.knots_u, span, at);
            let points: Vec<DVec4> = (0..count_v)
                .map(|j| {
                    (0..=self.degree_u)
                        .map(|i| h(span - self.degree_u + i, j) * b[0][i])
                        .fold(DVec4::ZERO, |a, x| a + x)
                })
                .collect();
            (self.degree_v, self.knots_v.clone(), points)
        };
        NurbsCurve::from_homogeneous(degree, knots, &points)
    }

    /// The largest second derivatives over the surface, estimated from samples: how fast
    /// it bends per unit of `u` squared and of `v` squared.
    pub fn bend(&self) -> DVec2 {
        *self.samples.0.bend.get_or_init(|| {
            let (lo, hi) = self.domain();
            let mut out = DVec2::ZERO;
            for i in 0..=12 {
                for j in 0..=12 {
                    let uv = lo + (hi - lo) * DVec2::new(f64::from(i) / 12.0, f64::from(j) / 12.0);
                    let [_, _, _, suu, suv, svv] = self.evaluate(uv);
                    out = out.max(DVec2::new(
                        suu.length() + suv.length(),
                        svv.length() + suv.length(),
                    ));
                }
            }
            out
        })
    }

    /// The largest first derivatives over the surface, from samples: millimetres per
    /// unit of `u` and of `v`.
    pub fn stretch(&self) -> DVec2 {
        *self.samples.0.stretch.get_or_init(|| {
            let (lo, hi) = self.domain();
            let mut out = DVec2::ZERO;
            for i in 0..=12 {
                for j in 0..=12 {
                    let uv = lo + (hi - lo) * DVec2::new(f64::from(i) / 12.0, f64::from(j) / 12.0);
                    let [_, su, sv, ..] = self.evaluate(uv);
                    out = out.max(DVec2::new(su.length(), sv.length()));
                }
            }
            out
        })
    }

    /// A surface from knot vectors that need not be clamped; see
    /// [`NurbsCurve::from_unclamped`]. `points` (and `weights`) are row by row in `u`.
    pub fn from_unclamped(
        degree_u: usize,
        degree_v: usize,
        knots_u: Vec<f64>,
        knots_v: Vec<f64>,
        points: Vec<DVec3>,
        weights: Option<Vec<f64>>,
    ) -> Result<Self, NurbsError> {
        let count_u = knots_u.len().saturating_sub(degree_u + 1);
        let count_v = knots_v.len().saturating_sub(degree_v + 1);
        check_unclamped(degree_u, &knots_u, count_u, "surface (u)")?;
        check_unclamped(degree_v, &knots_v, count_v, "surface (v)")?;
        if points.len() != count_u * count_v {
            return Err(NurbsError(format!(
                "surface: its knots call for {count_u} × {count_v} control points, not {}",
                points.len()
            )));
        }
        if is_clamped(degree_u, &knots_u, count_u) && is_clamped(degree_v, &knots_v, count_v) {
            return Self::new(degree_u, degree_v, knots_u, knots_v, points, weights);
        }
        let grid = homogeneous_checked(&points, weights.as_deref(), "surface")?;
        // Along u first: one strand per column.
        let mut along_u = Strands {
            degree: degree_u,
            knots: knots_u,
            lines: (0..count_v)
                .map(|j| (0..count_u).map(|i| grid[i * count_v + j]).collect())
                .collect(),
        };
        along_u.clamp();
        // Then along v: one strand per (new) row.
        let rows = along_u.lines[0].len();
        let mut along_v = Strands {
            degree: degree_v,
            knots: knots_v,
            lines: (0..rows)
                .map(|i| along_u.lines.iter().map(|line| line[i]).collect())
                .collect(),
        };
        along_v.clamp();
        let (points, weights) = from_homogeneous_points(&along_v.lines.concat());
        Self::new(
            degree_u,
            degree_v,
            along_u.knots,
            along_v.knots,
            points,
            Some(weights),
        )
    }

    /// The control points as strands along `u` (one per column) or along `v` (one per
    /// row).
    fn strands(&self, along_u: bool) -> Strands {
        let (count_u, count_v) = self.counts();
        let h = |i: usize, j: usize| {
            let index = i * count_v + j;
            homogeneous(self.points[index], self.weight(index))
        };
        if along_u {
            Strands {
                degree: self.degree_u,
                knots: self.knots_u.clone(),
                lines: (0..count_v)
                    .map(|j| (0..count_u).map(|i| h(i, j)).collect())
                    .collect(),
            }
        } else {
            Strands {
                degree: self.degree_v,
                knots: self.knots_v.clone(),
                lines: (0..count_u)
                    .map(|i| (0..count_v).map(|j| h(i, j)).collect())
                    .collect(),
            }
        }
    }

    /// This surface with its `u` (or `v`) direction replaced by `strands`.
    fn with_strands(&self, along_u: bool, strands: Strands) -> Result<Self, NurbsError> {
        let grid: Vec<DVec4> = if along_u {
            let rows = strands.lines.first().map_or(0, Vec::len);
            (0..rows)
                .flat_map(|i| strands.lines.iter().map(move |line| line[i]))
                .collect()
        } else {
            strands.lines.concat()
        };
        let (points, weights) = from_homogeneous_points(&grid);
        let (knots_u, knots_v) = if along_u {
            (strands.knots, self.knots_v.clone())
        } else {
            (self.knots_u.clone(), strands.knots)
        };
        Self::new(
            self.degree_u,
            self.degree_v,
            knots_u,
            knots_v,
            points,
            Some(weights),
        )
    }

    /// The part of the surface between `a < b` in `u` (`along_u`) or in `v`, limited to
    /// the domain: a surface of its own with the same parametrisation.
    pub fn sub_surface(&self, along_u: bool, a: f64, b: f64) -> Result<Self, NurbsError> {
        let strands = self.strands(along_u).between(a, b)?;
        self.with_strands(along_u, strands)
    }

    /// This surface followed by `next` in `u` (`along_u`) or in `v`. `next` must have
    /// the same degrees and the same knots in the other direction, and start where this
    /// one ends; its parameter is shifted to run on from this surface's last knot. The
    /// two meet at a knot of full multiplicity.
    pub fn joined(&self, next: &Self, along_u: bool) -> Result<Self, NurbsError> {
        let same_other = if along_u {
            self.degree_v == next.degree_v && self.knots_v == next.knots_v
        } else {
            self.degree_u == next.degree_u && self.knots_u == next.knots_u
        };
        if !same_other {
            return Err(NurbsError(
                "surfaces with different knots across the join can't be joined".to_owned(),
            ));
        }
        let strands = self.strands(along_u).join(next.strands(along_u))?;
        self.with_strands(along_u, strands)
    }

    /// Whether the surface closes on itself in `u` (`along_u`) or in `v`: its two edges
    /// across that direction are the same curve, to within `tolerance`.
    pub fn is_closed(&self, along_u: bool, tolerance: f64) -> bool {
        const SAMPLES: usize = 16;
        let (lo, hi) = self.domain();
        (0..=SAMPLES).all(|k| {
            let f = k as f64 / SAMPLES as f64;
            let (a, b) = if along_u {
                let v = lo.y + (hi.y - lo.y) * f;
                (DVec2::new(lo.x, v), DVec2::new(hi.x, v))
            } else {
                let u = lo.x + (hi.x - lo.x) * f;
                (DVec2::new(u, lo.y), DVec2::new(u, hi.y))
            };
            self.point(a).distance(self.point(b)) <= tolerance
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI, TAU};

    fn numeric<T, F>(f: F, t: f64) -> T
    where
        F: Fn(f64) -> T,
        T: std::ops::Sub<Output = T> + std::ops::Mul<f64, Output = T>,
    {
        let h = 1e-5;
        (f(t + h) - f(t - h)) * (0.5 / h)
    }

    #[test]
    fn lines_and_arcs_are_exact() {
        let line = NurbsCurve::line(DVec3::ZERO, DVec3::new(3.0, 4.0, 0.0), 2);
        assert!(
            line.point(0.5)
                .abs_diff_eq(DVec3::new(1.5, 2.0, 0.0), 1e-14)
        );
        assert!((line.evaluate(0.3)[1].length() - 5.0).abs() < 1e-12);
        let frame = Frame {
            origin: DVec3::new(1.0, 2.0, 3.0),
            rotation: peet_math::DQuat::from_rotation_x(0.7),
        };
        for (start, sweep) in [
            (0.3, 1.0),
            (-1.0, PI),
            (0.0, TAU),
            (2.0, -4.0),
            (0.5, FRAC_PI_2),
        ] {
            let arc = NurbsCurve::arc(&frame, 2.5, start, sweep);
            let (lo, hi) = arc.domain();
            assert_eq!((lo, hi), (0.0, 1.0));
            for i in 0..=40 {
                let p = arc.point(f64::from(i) / 40.0);
                let local = frame.to_local(p);
                assert!((local.truncate().length() - 2.5).abs() < 1e-12);
                assert!(local.z.abs() < 1e-12);
            }
            let ends = |a: f64| frame.to_world(DVec3::new(a.cos() * 2.5, a.sin() * 2.5, 0.0));
            assert!(arc.point(0.0).abs_diff_eq(ends(start), 1e-12));
            assert!(arc.point(1.0).abs_diff_eq(ends(start + sweep), 1e-12));
        }
    }

    #[test]
    fn derivatives_match_differences() {
        let c = NurbsCurve::new(
            3,
            vec![0.0, 0.0, 0.0, 0.0, 0.4, 0.7, 1.0, 1.0, 1.0, 1.0],
            vec![
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(1.0, 2.0, 0.5),
                DVec3::new(3.0, 2.5, -1.0),
                DVec3::new(4.0, 0.0, 0.0),
                DVec3::new(6.0, 1.0, 2.0),
                DVec3::new(7.0, 3.0, 1.0),
            ],
            Some(vec![1.0, 0.7, 1.3, 2.0, 0.9, 1.0]),
        )
        .unwrap();
        for t in [0.05, 0.3, 0.41, 0.55, 0.93] {
            let [_, d1, d2] = c.evaluate(t);
            assert!(d1.abs_diff_eq(numeric(|t| c.point(t), t), 1e-6), "{t}");
            assert!(
                d2.abs_diff_eq(numeric(|t| c.evaluate(t)[1], t), 1e-5),
                "{t}"
            );
        }
        // The closest point comes back for points of the curve, and off it the offset
        // is square to the curve.
        for t in [0.0, 0.11, 0.5, 0.77, 1.0] {
            assert!((c.param(c.point(t)) - t).abs() < 1e-9, "{t}");
        }
        let [p, d1, _] = c.evaluate(0.6);
        let side = d1.cross(DVec3::Z).normalize() * 0.2;
        assert!((c.param(p + side) - 0.6).abs() < 1e-7);
        // Reversed and reparametrized, it is the same curve.
        let r = c.reversed();
        assert!(r.point(0.25).abs_diff_eq(c.point(0.75), 1e-12));
        let s = c.reparametrized(2.0, 6.0);
        assert!(s.point(4.0).abs_diff_eq(c.point(0.5), 1e-12));
    }

    #[test]
    fn knot_insertion_keeps_the_curve() {
        let frame = Frame::WORLD;
        let arc = NurbsCurve::arc(&frame, 3.0, 0.0, 2.0);
        let line = NurbsCurve::line(DVec3::ZERO, DVec3::X, 1);
        let quarter = NurbsCurve::arc(&frame, 1.0, 0.0, 4.0);
        let made = compatible(&[arc.clone(), line.clone(), quarter.clone()]).unwrap();
        assert!(made.iter().all(|c| c.degree() == 2));
        assert!(made.iter().all(|c| c.knots() == made[0].knots()));
        for i in 0..=20 {
            let t = f64::from(i) / 20.0;
            assert!(made[0].point(t).abs_diff_eq(arc.point(t), 1e-12));
            assert!(made[1].point(t).abs_diff_eq(line.point(t), 1e-12));
            assert!(made[2].point(t).abs_diff_eq(quarter.point(t), 1e-12));
        }
    }

    #[test]
    fn interpolation_passes_through_its_points() {
        let through: Vec<DVec3> = (0..7)
            .map(|i| {
                let a = f64::from(i) * 0.6;
                DVec3::new(a.cos() * 5.0, a.sin() * 5.0, f64::from(i))
            })
            .collect();
        let c = NurbsCurve::interpolate(&through).unwrap();
        assert_eq!(c.degree(), 3);
        for p in &through {
            assert!(c.point(c.param(*p)).distance(*p) < 1e-10);
        }
        assert!(c.point(0.0).abs_diff_eq(through[0], 1e-12));
        assert!(c.point(1.0).abs_diff_eq(through[6], 1e-12));
        assert_eq!(NurbsCurve::interpolate(&through[..2]).unwrap().degree(), 1);
        assert!(NurbsCurve::interpolate(&[DVec3::ZERO, DVec3::ZERO]).is_err());
    }

    #[test]
    fn skinned_surfaces() {
        // A cylinder's quarter as a ruled surface between two arcs.
        let lower = NurbsCurve::arc(&Frame::WORLD, 2.0, 0.0, FRAC_PI_2);
        let top_frame = Frame {
            origin: DVec3::new(0.0, 0.0, 5.0),
            ..Frame::WORLD
        };
        let upper = NurbsCurve::arc(&top_frame, 2.0, 0.0, FRAC_PI_2);
        let s = NurbsSurface::skin(&[lower.clone(), upper]).unwrap();
        assert_eq!(s.degrees(), (2, 1));
        for i in 0..=10 {
            for j in 0..=10 {
                let uv = DVec2::new(f64::from(i) / 10.0, f64::from(j) / 10.0);
                let [p, su, sv, suu, suv, svv] = s.evaluate(uv);
                assert!((p.truncate().length() - 2.0).abs() < 1e-12);
                assert!((p.z - 5.0 * uv.y).abs() < 1e-12);
                // Outward: u runs counter-clockwise, v up.
                assert!(s.normal(uv).dot(p.truncate().extend(0.0)) > 1.99);
                let du = numeric(|u| s.point(DVec2::new(u, uv.y)), uv.x);
                let dv = numeric(|v| s.point(DVec2::new(uv.x, v)), uv.y);
                if (0.01..0.99).contains(&uv.x) && (0.01..0.99).contains(&uv.y) {
                    assert!(su.abs_diff_eq(du, 1e-6) && sv.abs_diff_eq(dv, 1e-6));
                    let duu = numeric(|u| s.evaluate(DVec2::new(u, uv.y))[1], uv.x);
                    let duv = numeric(|v| s.evaluate(DVec2::new(uv.x, v))[1], uv.y);
                    let dvv = numeric(|v| s.evaluate(DVec2::new(uv.x, v))[2], uv.y);
                    assert!(suu.abs_diff_eq(duu, 1e-5));
                    assert!(suv.abs_diff_eq(duv, 1e-5));
                    assert!(svv.abs_diff_eq(dvv, 1e-5));
                }
            }
        }
        // Closest points, on and off the surface.
        let uv = DVec2::new(0.37, 0.62);
        assert!(s.param(s.point(uv)).abs_diff_eq(uv, 1e-9));
        let off = s.point(uv) + s.normal(uv) * 0.5;
        assert!(s.param(off).abs_diff_eq(uv, 1e-8));
        // Iso curves are the sections.
        let bottom = s.iso_curve(true, 0.0);
        for i in 0..=8 {
            let t = f64::from(i) / 8.0;
            assert!(bottom.point(t).abs_diff_eq(lower.point(t), 1e-12));
        }
        let ruling = s.iso_curve(false, 0.5);
        assert!((ruling.point(1.0).z - 5.0).abs() < 1e-12);

        // Through four sections of different sizes: it passes through each.
        let sections: Vec<NurbsCurve> = [(0.0, 3.0), (4.0, 5.0), (9.0, 2.0), (12.0, 4.0)]
            .iter()
            .map(|&(z, r)| {
                let f = Frame {
                    origin: DVec3::new(0.0, 0.0, z),
                    ..Frame::WORLD
                };
                NurbsCurve::arc(&f, r, 0.0, PI)
            })
            .collect();
        let s = NurbsSurface::skin(&sections).unwrap();
        assert_eq!(s.degrees(), (2, 3));
        for section in &sections {
            for i in 0..=6 {
                let p = section.point(f64::from(i) / 6.0);
                assert!(s.point(s.param(p)).distance(p) < 1e-9);
            }
        }
        assert!(NurbsSurface::skin(&sections[..1]).is_err());
    }

    #[test]
    fn bad_definitions_are_refused() {
        let p = vec![DVec3::ZERO, DVec3::X, DVec3::Y];
        assert!(NurbsCurve::new(2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0], p.clone(), None).is_ok());
        assert!(NurbsCurve::new(2, vec![0.0, 0.0, 1.0, 1.0, 1.0], p.clone(), None).is_err());
        assert!(NurbsCurve::new(2, vec![0.0, 0.1, 0.2, 1.0, 1.0, 1.0], p.clone(), None).is_err());
        assert!(NurbsCurve::new(3, vec![0.0; 7], p.clone(), None).is_err());
        assert!(
            NurbsCurve::new(
                2,
                vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
                p.clone(),
                Some(vec![1.0, -1.0, 1.0])
            )
            .is_err()
        );
        assert!(
            NurbsCurve::new(
                2,
                vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
                vec![DVec3::ZERO, DVec3::splat(f64::NAN), DVec3::X],
                None
            )
            .is_err()
        );
        assert!(
            NurbsSurface::new(
                1,
                1,
                vec![0.0, 0.0, 1.0, 1.0],
                vec![0.0, 0.0, 1.0, 1.0],
                p,
                None
            )
            .is_err()
        );
    }

    /// The B-spline basis function `i` of degree `p` by the Cox–de Boor recursion: slow,
    /// but good for any knot vector.
    fn cox_de_boor(knots: &[f64], i: usize, p: usize, t: f64) -> f64 {
        if p == 0 {
            return if knots[i] <= t && t < knots[i + 1] {
                1.0
            } else {
                0.0
            };
        }
        let mut out = 0.0;
        if knots[i + p] > knots[i] {
            out += (t - knots[i]) / (knots[i + p] - knots[i]) * cox_de_boor(knots, i, p - 1, t);
        }
        if knots[i + p + 1] > knots[i + 1] {
            out += (knots[i + p + 1] - t) / (knots[i + p + 1] - knots[i + 1])
                * cox_de_boor(knots, i + 1, p - 1, t);
        }
        out
    }

    fn rational_point(knots: &[f64], p: usize, points: &[DVec3], weights: &[f64], t: f64) -> DVec3 {
        let mut sum = DVec4::ZERO;
        for i in 0..points.len() {
            sum += homogeneous(points[i], weights[i]) * cox_de_boor(knots, i, p, t);
        }
        sum.truncate() / sum.w
    }

    #[test]
    fn unclamped_knots_are_clamped() {
        // A closed cubic written the periodic way: uniform knots, the first three control
        // points repeated at the end.
        let ring: Vec<DVec3> = (0..6)
            .map(|k| {
                let a = f64::from(k) * TAU / 6.0;
                DVec3::new(a.cos() * 4.0, a.sin() * 3.0, f64::from(k % 2))
            })
            .collect();
        let mut points = ring.clone();
        points.extend_from_slice(&ring[..3]);
        let mut weights = vec![1.0, 2.0, 0.5, 1.0, 1.5, 0.8];
        weights.extend_from_within(..3);
        let knots: Vec<f64> = (0..13).map(|k| f64::from(k) - 3.0).collect();
        for w in [None, Some(weights.clone())] {
            let c =
                NurbsCurve::from_unclamped(3, knots.clone(), points.clone(), w.clone()).unwrap();
            assert_eq!(c.domain(), (0.0, 6.0));
            assert!(c.is_closed(1e-12));
            assert_eq!(c.weights().is_some(), w.is_some());
            let w = w.unwrap_or(vec![1.0; 9]);
            for i in 0..60 {
                let t = f64::from(i) * 0.1;
                let expected = rational_point(&knots, 3, &points, &w, t);
                assert!(c.point(t).abs_diff_eq(expected, 1e-12), "{t}");
            }
            // Smooth across the start: the tangent at both ends is the same.
            let (a, b) = (c.evaluate(0.0)[1], c.evaluate(6.0)[1]);
            assert!(a.abs_diff_eq(b, 1e-9), "{a} {b}");
        }
        // Clamped at one end only, with a repeated knot inside.
        let knots = vec![0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 3.0, 4.0, 5.0];
        let c = NurbsCurve::from_unclamped(2, knots.clone(), ring.clone(), None).unwrap();
        assert_eq!(c.domain(), (0.0, 3.0));
        for i in 0..30 {
            let t = f64::from(i) * 0.1;
            let expected = rational_point(&knots, 2, &ring, &[1.0; 6], t);
            assert!(c.point(t).abs_diff_eq(expected, 1e-12), "{t}");
        }
        // A clamped vector is taken as it is.
        let clamped = vec![0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 4.0, 4.0];
        let c = NurbsCurve::from_unclamped(2, clamped.clone(), ring.clone(), None).unwrap();
        assert_eq!(c.knots(), &clamped[..]);
        assert_eq!(c.control_points(), &ring[..]);
        // Bad definitions.
        let unclamped = |degree: usize, knots: Vec<f64>| {
            NurbsCurve::from_unclamped(degree, knots, ring.clone(), None)
        };
        assert!(unclamped(2, vec![0.0; 9]).is_err());
        assert!(unclamped(2, (0..8).map(f64::from).collect()).is_err());
        assert!(unclamped(0, (0..7).map(f64::from).collect()).is_err());
        assert!(unclamped(16, (0..23).map(f64::from).collect()).is_err());
        assert!(unclamped(2, vec![0.0, 1.0, 2.0, 3.0, 2.5, 5.0, 6.0, 7.0, 8.0]).is_err());
        assert!(unclamped(2, vec![0.0, 1.0, 2.0, f64::NAN, 4.0, 5.0, 6.0, 7.0, 8.0]).is_err());
        // A break in the middle of the curve.
        assert!(unclamped(2, vec![0.0, 1.0, 2.0, 3.0, 3.0, 3.0, 4.0, 5.0, 6.0]).is_err());
        assert!(
            NurbsCurve::from_unclamped(
                2,
                (0..9).map(f64::from).collect(),
                ring.clone(),
                Some(vec![1.0, 1.0, 0.0, 1.0, 1.0, 1.0])
            )
            .is_err()
        );
    }

    #[test]
    fn curves_are_cut_and_joined() {
        let c = NurbsCurve::new(
            3,
            vec![0.0, 0.0, 0.0, 0.0, 0.4, 0.7, 1.0, 1.0, 1.0, 1.0],
            vec![
                DVec3::new(0.0, 0.0, 0.0),
                DVec3::new(1.0, 2.0, 0.5),
                DVec3::new(3.0, 2.5, -1.0),
                DVec3::new(4.0, 0.0, 0.0),
                DVec3::new(6.0, 1.0, 2.0),
                DVec3::new(7.0, 3.0, 1.0),
            ],
            Some(vec![1.0, 0.7, 1.3, 2.0, 0.9, 1.0]),
        )
        .unwrap();
        for (a, b) in [
            (0.0, 1.0),
            (0.2, 0.55),
            (0.4, 0.7),
            (0.0, 0.4),
            (0.65, 1.0),
            (-1.0, 0.3),
        ] {
            let part = c.sub_curve(a, b).unwrap();
            let (a, b) = (a.max(0.0), b.min(1.0));
            assert_eq!(part.domain(), (a, b));
            for i in 0..=20 {
                let t = a + (b - a) * f64::from(i) / 20.0;
                assert!(
                    part.point(t).abs_diff_eq(c.point(t), 1e-12),
                    "{a}..{b} at {t}"
                );
                assert!(part.evaluate(t)[1].abs_diff_eq(c.evaluate(t)[1], 1e-9));
            }
        }
        assert!(c.sub_curve(0.5, 0.5).is_err());
        assert!(c.sub_curve(0.7, 0.2).is_err());
        assert!(c.sub_curve(f64::NAN, 0.2).is_err());
        assert!(!c.is_closed(1e-6));
        // Two parts put back together are the curve again.
        let whole = c
            .sub_curve(0.0, 0.3)
            .unwrap()
            .joined(&c.sub_curve(0.3, 1.0).unwrap())
            .unwrap();
        assert_eq!(whole.domain(), (0.0, 1.0));
        for i in 0..=20 {
            let t = f64::from(i) / 20.0;
            assert!(whole.point(t).abs_diff_eq(c.point(t), 1e-12));
        }
        // A full circle started somewhere else: its tail, then its head.
        let circle = NurbsCurve::arc(&Frame::WORLD, 2.0, 0.0, TAU);
        assert!(circle.is_closed(1e-12));
        let turned = circle
            .sub_curve(0.6, 1.0)
            .unwrap()
            .joined(&circle.sub_curve(0.0, 0.6).unwrap())
            .unwrap();
        assert_eq!(turned.domain().0, 0.6);
        assert!((turned.domain().1 - 1.6).abs() < 1e-15);
        for i in 0..=40 {
            let t = 0.6 + f64::from(i) / 40.0;
            let expected = circle.point(if t > 1.0 { t - 1.0 } else { t });
            assert!(turned.point(t).abs_diff_eq(expected, 1e-12), "{t}");
        }
        assert!(circle.joined(&c).is_err(), "different degrees");
    }

    #[test]
    fn surfaces_are_cut_joined_and_clamped() {
        // A tube: circles of different sizes up the Z axis.
        let sections: Vec<NurbsCurve> = [(0.0, 3.0), (4.0, 5.0), (9.0, 2.0), (12.0, 4.0)]
            .iter()
            .map(|&(z, r)| {
                let f = Frame {
                    origin: DVec3::new(0.0, 0.0, z),
                    ..Frame::WORLD
                };
                NurbsCurve::arc(&f, r, 0.0, TAU)
            })
            .collect();
        let s = NurbsSurface::skin(&sections).unwrap();
        assert!(s.is_closed(true, 1e-12) && !s.is_closed(false, 1e-6));
        let same = |a: &NurbsSurface, b: &NurbsSurface, lo: DVec2, hi: DVec2, shift: DVec2| {
            for i in 0..=10 {
                for j in 0..=10 {
                    let uv = lo + (hi - lo) * DVec2::new(f64::from(i), f64::from(j)) / 10.0;
                    let (p, q) = (a.point(uv), b.point(uv + shift));
                    assert!(p.abs_diff_eq(q, 1e-11), "{uv}: {p} {q}");
                }
            }
        };
        let part = s.sub_surface(true, 0.1, 0.6).unwrap();
        assert_eq!(part.domain(), (DVec2::new(0.1, 0.0), DVec2::new(0.6, 1.0)));
        same(
            &part,
            &s,
            DVec2::new(0.1, 0.0),
            DVec2::new(0.6, 1.0),
            DVec2::ZERO,
        );
        assert!(!part.is_closed(true, 1e-6));
        let band = s.sub_surface(false, 0.25, 0.5).unwrap();
        assert_eq!(band.domain(), (DVec2::new(0.0, 0.25), DVec2::new(1.0, 0.5)));
        same(
            &band,
            &s,
            DVec2::new(0.0, 0.25),
            DVec2::new(1.0, 0.5),
            DVec2::ZERO,
        );
        assert!(s.sub_surface(true, 0.5, 0.5).is_err());
        // The tube with its seam moved: from u = 0.7 round to 0.7 again.
        let turned = s
            .sub_surface(true, 0.7, 1.0)
            .unwrap()
            .joined(&s.sub_surface(true, 0.0, 0.7).unwrap(), true)
            .unwrap();
        same(
            &turned,
            &s,
            DVec2::new(0.7, 0.0),
            DVec2::new(1.0, 1.0),
            DVec2::ZERO,
        );
        same(
            &turned,
            &s,
            DVec2::new(1.0, 0.0),
            DVec2::new(1.7, 1.0),
            DVec2::new(-1.0, 0.0),
        );
        let stacked = s
            .sub_surface(false, 0.0, 0.4)
            .unwrap()
            .joined(&s.sub_surface(false, 0.4, 1.0).unwrap(), false)
            .unwrap();
        same(&stacked, &s, DVec2::ZERO, DVec2::ONE, DVec2::ZERO);
        assert!(part.joined(&band, true).is_err());

        // Uniform knots both ways: a bicubic patch of a 6 × 5 net.
        let net: Vec<DVec3> = (0..30)
            .map(|k| {
                let (i, j) = (f64::from(k / 5), f64::from(k % 5));
                DVec3::new(i * 2.0, j * 3.0, (i * 0.9).sin() * (j * 0.7).cos() * 2.0)
            })
            .collect();
        let weights: Vec<f64> = (0..30).map(|k| 1.0 + f64::from(k % 7) * 0.1).collect();
        let knots_u: Vec<f64> = (0..10).map(f64::from).collect();
        let knots_v: Vec<f64> = (0..8).map(|k| f64::from(k) * 0.5).collect();
        let patch = NurbsSurface::from_unclamped(
            3,
            2,
            knots_u.clone(),
            knots_v.clone(),
            net.clone(),
            Some(weights.clone()),
        )
        .unwrap();
        assert_eq!(patch.domain(), (DVec2::new(3.0, 1.0), DVec2::new(6.0, 2.5)));
        for a in 0..=6 {
            for b in 0..=6 {
                // Just inside the far edges, where the test's half-open spans end.
                let u = 3.0 + f64::from(a) * 0.4999;
                let v = 1.0 + f64::from(b) * 0.2499;
                let mut sum = DVec4::ZERO;
                for i in 0..6 {
                    for j in 0..5 {
                        sum += homogeneous(net[i * 5 + j], weights[i * 5 + j])
                            * cox_de_boor(&knots_u, i, 3, u)
                            * cox_de_boor(&knots_v, j, 2, v);
                    }
                }
                let expected = sum.truncate() / sum.w;
                assert!(patch.point(DVec2::new(u, v)).abs_diff_eq(expected, 1e-12));
            }
        }
        assert!(
            NurbsSurface::from_unclamped(
                3,
                2,
                knots_u.clone(),
                knots_v.clone(),
                net[1..].to_vec(),
                None
            )
            .is_err()
        );
        assert!(NurbsSurface::from_unclamped(3, 2, knots_u, vec![0.0; 8], net, None).is_err());
    }
}
