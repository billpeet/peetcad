//! Surfaces fitted to shapes that have no NURBS form of their own: the offset of a
//! freeform surface, a ruled surface whose rulings turn along a curve.
//!
//! **Method.** The shape is given as a function of `(u, v)` over a rectangle cut into
//! spans at given breaks (where the shape is allowed a crease: the knots of the surface it
//! derives from). Each span is smooth, so it is interpolated by a spline of a chosen
//! degree in pieces of equal length, through samples at the places where its control
//! points have most say; neighbouring spans share the samples on the line between them,
//! which makes the whole surface continuous. The fit is then compared with the function
//! halfway between the samples, and spans that are off by more than the tolerance are
//! cut into more pieces, until none is. Curves are fitted the same way
//! ([`NurbsCurve::fitted`]).
//!
//! **Offsets.** [`NurbsSurface::offset`] fits `S(u, v) + d·n(u, v)`. So that an offset
//! face can still reach neighbours that moved away from it, the surface is first carried
//! on past its edges along its tangents ([`NurbsSurface::extended`]); the result has the
//! original's parametrisation over a slightly larger rectangle.

use peet_math::{DVec2, DVec3};

use super::{NurbsCurve, NurbsError, NurbsSurface, solve_interpolation};

/// A span is never cut into more pieces than this: a shape that needs more is not smooth.
const MAX_PIECES: usize = 96;

/// Rounds of refinement before a fit is given up.
const MAX_ROUNDS: usize = 12;

/// Why a surface could not be offset.
#[derive(Clone, Debug, PartialEq)]
pub enum OffsetError {
    /// The surface bends more tightly than the distance, towards the side it is moved
    /// to, so its offset would fold over itself. `radius` is its tightest radius of
    /// curvature on that side (mm).
    Folds { radius: f64 },
    /// The surface has a point without a normal (a collapsed edge).
    Pinched,
    /// No fit to the tolerance was found: the surface has a crease.
    Rough,
}

/// One direction of a fit: where the samples are, and the knots of the spline through
/// them.
struct Axis {
    degree: usize,
    knots: Vec<f64>,
    nodes: Vec<f64>,
    /// The span each interval between two nodes belongs to.
    span_of: Vec<usize>,
}

impl Axis {
    /// `pieces[s]` polynomial pieces in the span from `breaks[s]` to `breaks[s + 1]`.
    fn new(breaks: &[f64], pieces: &[usize], degree: usize) -> Self {
        let mut knots = vec![breaks[0]; degree + 1];
        let mut nodes = vec![breaks[0]];
        let mut span_of = Vec::new();
        for (s, w) in breaks.windows(2).enumerate() {
            let (a, b) = (w[0], w[1]);
            // Pieces of equal length, and the span's own knot vector, clamped at both
            // ends.
            let k = pieces[s];
            let mut own = vec![a; degree + 1];
            own.extend((1..k).map(|j| a + (b - a) * j as f64 / k as f64));
            own.extend(std::iter::repeat_n(b, degree + 1));
            // Samples where the control points have their say (the averages of `degree`
            // knots in a row): as many as there are control points, the ends included,
            // which keeps the interpolation well posed and its pieces equally good.
            let m = k + degree;
            for i in 1..m - 1 {
                nodes.push(own[i + 1..=i + degree].iter().sum::<f64>() / degree as f64);
            }
            nodes.push(b);
            // Full multiplicity where spans meet: the fit may have a crease there.
            let last = s + 2 == breaks.len();
            knots.extend_from_slice(&own[degree + 1..degree + k]);
            knots.extend(std::iter::repeat_n(
                b,
                if last { degree + 1 } else { degree },
            ));
            span_of.extend(std::iter::repeat_n(s, m - 1));
        }
        Self {
            degree,
            knots,
            nodes,
            span_of,
        }
    }

    fn solve(&self, values: &[DVec3]) -> Result<Vec<DVec3>, NurbsError> {
        solve_interpolation(self.degree, &self.knots, &self.nodes, values)
    }

    /// The points halfway between the nodes, each with its span.
    fn middles(&self) -> Vec<(f64, usize)> {
        self.nodes
            .windows(2)
            .zip(&self.span_of)
            .map(|(w, s)| (0.5 * (w[0] + w[1]), *s))
            .collect()
    }
}

/// More pieces for the spans whose error is over the tolerance. `false` if a span would
/// need more than are allowed.
fn refine(pieces: &mut [usize], errors: &[f64], degree: usize, tolerance: f64) -> bool {
    for (k, e) in pieces.iter_mut().zip(errors) {
        if *e <= tolerance {
            continue;
        }
        // The error falls with the piece length to the power degree + 1.
        let factor = (e / tolerance).powf(1.0 / (degree + 1) as f64) * 1.15;
        let wanted = ((*k as f64 * factor).ceil() as usize).max(*k + 1);
        if *k >= MAX_PIECES {
            return false;
        }
        *k = wanted.min(MAX_PIECES);
    }
    true
}

impl NurbsCurve {
    /// The curve of the given degree that follows `shape` to within `tolerance` (mm)
    /// over the parameters from the first to the last of `breaks`. The breaks in
    /// between are where `shape` may have a corner; between them it must be smooth.
    /// `shape` is asked for points in order along the curve, and returns `None` where
    /// it has none, which fails the fit.
    pub fn fitted(
        shape: &dyn Fn(f64) -> Option<DVec3>,
        breaks: &[f64],
        degree: usize,
        tolerance: f64,
    ) -> Result<Self, NurbsError> {
        let miss = |t: f64, p: DVec3| Some(shape(t)?.distance(p));
        Self::fitted_within(shape, &miss, breaks, degree, tolerance)
    }

    /// [`NurbsCurve::fitted`] with the caller's own measure of how far off the fit is:
    /// `miss(t, p)` for the fit's point `p` at the parameter `t`. For a curve that has
    /// to lie on two surfaces, the distance from them is a better measure than the
    /// distance from sampled points of their intersection, which wander where the
    /// surfaces are nearly tangent. In each round `shape` is asked for all its points,
    /// in order, before `miss` is asked anything.
    pub fn fitted_within(
        shape: &dyn Fn(f64) -> Option<DVec3>,
        miss: &dyn Fn(f64, DVec3) -> Option<f64>,
        breaks: &[f64],
        degree: usize,
        tolerance: f64,
    ) -> Result<Self, NurbsError> {
        if breaks.len() < 2 || breaks.windows(2).any(|w| w[1] <= w[0]) || degree == 0 {
            return Err(NurbsError(
                "a curve to fit needs a range of parameters and a degree of at least 1".to_owned(),
            ));
        }
        let undefined = || NurbsError("the shape to fit has a point without a position".to_owned());
        let mut pieces = vec![1; breaks.len() - 1];
        for _ in 0..MAX_ROUNDS {
            let axis = Axis::new(breaks, &pieces, degree);
            let mut values = Vec::with_capacity(axis.nodes.len());
            for &t in &axis.nodes {
                let p = shape(t).ok_or_else(undefined)?;
                if !p.is_finite() {
                    return Err(undefined());
                }
                values.push(p);
            }
            let curve = Self::new(degree, axis.knots.clone(), axis.solve(&values)?, None)?;
            // How far off it is halfway between the samples, by span.
            let mut errors = vec![0.0_f64; pieces.len()];
            for (t, span) in axis.middles() {
                let e = miss(t, curve.point(t)).ok_or_else(undefined)?;
                errors[span] = errors[span].max(e);
            }
            if errors.iter().all(|e| *e <= tolerance) {
                return Ok(curve);
            }
            if !refine(&mut pieces, &errors, degree, tolerance) {
                break;
            }
        }
        Err(NurbsError(
            "the shape can't be followed closely enough by a smooth curve (it has a corner)"
                .to_owned(),
        ))
    }
}

impl NurbsSurface {
    /// The surface of the given degrees (in `u` and in `v`) that follows `shape` to
    /// within `tolerance` (mm), over the rectangle from the first to the last of
    /// `breaks_u` and of `breaks_v`. The breaks in between are where `shape` may have a
    /// crease; between them it must be smooth. `shape` returns `None` where it has no
    /// point, which fails the fit.
    pub fn fitted(
        shape: &dyn Fn(DVec2) -> Option<DVec3>,
        breaks_u: &[f64],
        breaks_v: &[f64],
        degrees: (usize, usize),
        tolerance: f64,
    ) -> Result<Self, NurbsError> {
        let rising = |b: &[f64]| b.len() >= 2 && b.windows(2).all(|w| w[1] > w[0]);
        if !rising(breaks_u) || !rising(breaks_v) || degrees.0 == 0 || degrees.1 == 0 {
            return Err(NurbsError(
                "a surface to fit needs a rectangle of parameters and degrees of at least 1"
                    .to_owned(),
            ));
        }
        let undefined = || NurbsError("the shape to fit has a point without a position".to_owned());
        let mut pieces_u = vec![1; breaks_u.len() - 1];
        let mut pieces_v = vec![1; breaks_v.len() - 1];
        for _ in 0..MAX_ROUNDS {
            let au = Axis::new(breaks_u, &pieces_u, degrees.0);
            let av = Axis::new(breaks_v, &pieces_v, degrees.1);
            let (nu, nv) = (au.nodes.len(), av.nodes.len());
            // Along v at each u, then along u for each row of the results.
            let mut half = Vec::with_capacity(nu * nv);
            let mut row = Vec::with_capacity(nv);
            for &u in &au.nodes {
                row.clear();
                for &v in &av.nodes {
                    let p = shape(DVec2::new(u, v)).ok_or_else(undefined)?;
                    if !p.is_finite() {
                        return Err(undefined());
                    }
                    row.push(p);
                }
                half.extend(av.solve(&row)?);
            }
            let mut points = vec![DVec3::ZERO; nu * nv];
            let mut column = Vec::with_capacity(nu);
            for j in 0..nv {
                column.clear();
                column.extend((0..nu).map(|i| half[i * nv + j]));
                for (i, p) in au.solve(&column)?.into_iter().enumerate() {
                    points[i * nv + j] = p;
                }
            }
            let surface = Self::new(
                degrees.0,
                degrees.1,
                au.knots.clone(),
                av.knots.clone(),
                points,
                None,
            )?;
            // How far off it is between the samples, by span.
            let mut errors_u = vec![0.0_f64; pieces_u.len()];
            let mut errors_v = vec![0.0_f64; pieces_v.len()];
            let off = |uv: DVec2| -> Result<f64, NurbsError> {
                Ok(shape(uv).ok_or_else(undefined)?.distance(surface.point(uv)))
            };
            let (middles_u, middles_v) = (au.middles(), av.middles());
            // Between samples along one direction, on a line of samples of the other:
            // the error there is that direction's alone.
            for &(u, su) in &middles_u {
                for &v in &av.nodes {
                    errors_u[su] = errors_u[su].max(off(DVec2::new(u, v))?);
                }
            }
            for &u in &au.nodes {
                for &(v, sv) in &middles_v {
                    errors_v[sv] = errors_v[sv].max(off(DVec2::new(u, v))?);
                }
            }
            let fine = |errors: &[f64]| errors.iter().all(|e| *e <= tolerance);
            if fine(&errors_u) && fine(&errors_v) {
                // Each direction is fine on the other's lines; between both, the error
                // is the two together, and both are to blame for what is left.
                let mut both = false;
                for &(u, su) in &middles_u {
                    for &(v, sv) in &middles_v {
                        let e = off(DVec2::new(u, v))?;
                        if e > tolerance {
                            errors_u[su] = errors_u[su].max(e);
                            errors_v[sv] = errors_v[sv].max(e);
                            both = true;
                        }
                    }
                }
                if !both {
                    return Ok(surface);
                }
            }
            if !refine(&mut pieces_u, &errors_u, degrees.0, tolerance)
                || !refine(&mut pieces_v, &errors_v, degrees.1, tolerance)
            {
                break;
            }
        }
        Err(NurbsError(
            "the shape can't be followed closely enough by a smooth surface (it has a crease \
             or a pinched corner)"
                .to_owned(),
        ))
    }

    /// The point and the derivatives `[S, Su, Sv]` at `uv`, which may lie outside the
    /// parameter rectangle: past an edge the surface carries on along its tangents there
    /// (straight on, as a ruled strip).
    pub fn extended(&self, uv: DVec2) -> [DVec3; 3] {
        let (lo, hi) = self.domain();
        let inside = uv.clamp(lo, hi);
        let [s, su, sv, _, suv, _] = self.evaluate(inside);
        let d = uv - inside;
        if d == DVec2::ZERO {
            return [s, su, sv];
        }
        [
            s + su * d.x + sv * d.y + suv * (d.x * d.y),
            su + suv * d.y,
            sv + suv * d.x,
        ]
    }

    /// The unit normal of [`NurbsSurface::extended`] at `uv`, or `None` where the
    /// surface is pinched.
    pub fn extended_normal(&self, uv: DVec2) -> Option<DVec3> {
        let [_, su, sv] = self.extended(uv);
        let n = su.cross(sv);
        (n.length() > 1e-9 * su.length() * sv.length()).then(|| n.normalize())
    }

    /// The tightest radius of curvature of the surface where it bends towards its
    /// natural normal (`toward_normal`) or away from it, from samples: an offset to that
    /// side by as much would fold. `None` if it doesn't bend that way at all.
    pub fn tightest_radius(&self, toward_normal: bool) -> Option<f64> {
        const PER_SPAN: usize = 6;
        let (bu, bv) = self.breaks();
        let along = |breaks: &[f64]| -> Vec<f64> {
            let mut out = vec![breaks[0]];
            for w in breaks.windows(2) {
                out.extend(
                    (1..=PER_SPAN).map(|i| w[0] + (w[1] - w[0]) * i as f64 / PER_SPAN as f64),
                );
            }
            out
        };
        let (us, vs) = (along(&bu), along(&bv));
        let mut tightest = 0.0_f64;
        for &u in &us {
            for &v in &vs {
                let [_, su, sv, suu, suv, svv] = self.evaluate(DVec2::new(u, v));
                let cross = su.cross(sv);
                let (e, f, g) = (su.dot(su), su.dot(sv), sv.dot(sv));
                let det = e * g - f * f;
                if det <= 1e-18 * e * g || det <= f64::MIN_POSITIVE {
                    continue;
                }
                let n = cross / cross.length();
                let (l, m, nn) = (suu.dot(n), suv.dot(n), svv.dot(n));
                // Principal curvatures from the mean and Gaussian curvatures; positive
                // where the surface bends towards its normal.
                let mean = (e * nn - 2.0 * f * m + g * l) / (2.0 * det);
                let gauss = (l * nn - m * m) / det;
                let spread = (mean * mean - gauss).max(0.0).sqrt();
                let k = if toward_normal {
                    mean + spread
                } else {
                    -(mean - spread)
                };
                tightest = tightest.max(k);
            }
        }
        (tightest > 1e-12).then(|| 1.0 / tightest)
    }

    /// This surface moved by `distance` along its natural normal (against it for a
    /// negative distance), to within `tolerance` (mm). The result keeps this surface's
    /// parametrisation (the point at `(u, v)` is the offset of this surface's point
    /// there) and reaches about `margin` millimetres past each of its edges, except
    /// across a direction in which the surface closes on itself.
    pub fn offset(&self, distance: f64, margin: f64, tolerance: f64) -> Result<Self, OffsetError> {
        if let Some(radius) = self.tightest_radius(distance > 0.0)
            && distance.abs() >= 0.98 * radius
        {
            return Err(OffsetError::Folds { radius });
        }
        let (lo, hi) = self.domain();
        let (mut breaks_u, mut breaks_v) = self.breaks();
        // How far a margin in millimetres is in each parameter at each edge: by the
        // slowest the surface moves across that edge.
        const ALONG: usize = 16;
        let reach = |along_u: bool, at: f64| -> Option<f64> {
            if margin <= 0.0 || self.is_closed(along_u, tolerance.max(1e-9)) {
                return None;
            }
            let (range, other_lo, other_hi) = if along_u {
                (hi.x - lo.x, lo.y, hi.y)
            } else {
                (hi.y - lo.y, lo.x, hi.x)
            };
            let slowest = (0..=ALONG)
                .map(|k| {
                    let w = other_lo + (other_hi - other_lo) * k as f64 / ALONG as f64;
                    let uv = if along_u {
                        DVec2::new(at, w)
                    } else {
                        DVec2::new(w, at)
                    };
                    let [_, su, sv, ..] = self.evaluate(uv);
                    if along_u { su.length() } else { sv.length() }
                })
                .fold(f64::INFINITY, f64::min);
            Some((margin / slowest.max(1e-12)).min(range))
        };
        if let Some(r) = reach(true, lo.x) {
            breaks_u.insert(0, lo.x - r);
        }
        if let Some(r) = reach(true, hi.x) {
            breaks_u.push(hi.x + r);
        }
        if let Some(r) = reach(false, lo.y) {
            breaks_v.insert(0, lo.y - r);
        }
        if let Some(r) = reach(false, hi.y) {
            breaks_v.push(hi.y + r);
        }
        let shape = |uv: DVec2| -> Option<DVec3> {
            let [p, su, sv] = self.extended(uv);
            let n = su.cross(sv);
            (n.length() > 1e-9 * su.length() * sv.length()).then(|| p + n.normalize() * distance)
        };
        // Is the surface pinched anywhere? (The fit would only say it found no point.)
        let corners = [lo, hi, DVec2::new(lo.x, hi.y), DVec2::new(hi.x, lo.y)];
        if corners.iter().any(|c| self.extended_normal(*c).is_none()) {
            return Err(OffsetError::Pinched);
        }
        const DEGREE: usize = 5;
        let fitted = Self::fitted(&shape, &breaks_u, &breaks_v, (DEGREE, DEGREE), tolerance)
            .map_err(|_| OffsetError::Rough)?;
        // The offset must face the way the surface does everywhere: where it doesn't, it
        // has folded (past the edges, where the curvature was not looked at).
        const CHECKS: usize = 4;
        let (flo, fhi) = fitted.domain();
        let (fu, fv) = fitted.breaks();
        let steps = |breaks: &[f64]| (breaks.len() - 1) * CHECKS;
        let (nu, nv) = (steps(&fu), steps(&fv));
        let mut radius = f64::INFINITY;
        for i in 0..=nu {
            for j in 0..=nv {
                let uv = flo + (fhi - flo) * DVec2::new(i as f64 / nu as f64, j as f64 / nv as f64);
                let Some(n) = self.extended_normal(uv) else {
                    return Err(OffsetError::Pinched);
                };
                let [_, ou, ov, ..] = fitted.evaluate(uv);
                let [_, su, sv] = self.extended(uv);
                // The offset's area element against the surface's: it shrinks to nothing
                // where the offset folds.
                let ratio = ou.cross(ov).dot(n) / su.cross(sv).length().max(f64::MIN_POSITIVE);
                if ratio <= 1e-3 {
                    radius = radius.min(distance.abs());
                }
            }
        }
        if radius.is_finite() {
            return Err(OffsetError::Folds { radius });
        }
        Ok(fitted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peet_math::Frame;
    use std::f64::consts::FRAC_PI_2;

    /// A quarter of a cone between two arcs: radius `r0` at the bottom, `r1` at `height`.
    fn cone(r0: f64, r1: f64, height: f64) -> NurbsSurface {
        let lower = NurbsCurve::arc(&Frame::WORLD, r0, 0.0, FRAC_PI_2);
        let top = Frame {
            origin: DVec3::new(0.0, 0.0, height),
            ..Frame::WORLD
        };
        let upper = NurbsCurve::arc(&top, r1, 0.0, FRAC_PI_2);
        NurbsSurface::skin(&[lower, upper]).unwrap()
    }

    #[test]
    fn offset_of_a_cylinder_and_a_cone() {
        // A cylinder of radius 4 moved inwards by 1 is the cylinder of radius 3.
        let s = cone(4.0, 4.0, 10.0);
        let o = s.offset(-1.0, 2.0, 1e-7).unwrap();
        let (lo, hi) = o.domain();
        assert!(lo.x < 0.0 && lo.y < 0.0 && hi.x > 1.0 && hi.y > 1.0);
        let mut worst = 0.0_f64;
        for i in 0..=60 {
            for j in 0..=60 {
                let uv = lo + (hi - lo) * DVec2::new(f64::from(i) / 60.0, f64::from(j) / 60.0);
                let p = o.point(uv);
                // Past its straight edges it carries on as the planes tangent there.
                if (0.0..=1.0).contains(&uv.x) {
                    worst = worst.max((p.truncate().length() - 3.0).abs());
                }
                // The same parametrisation.
                let [q, ..] = s.extended(uv);
                assert!((p.z - q.z).abs() < 1e-6);
            }
        }
        assert!(worst < 1.5e-7, "{worst}");
        // A cone: radius 10 − 0.4 z, moved outwards by 0.5 square to its rulings.
        let s = cone(10.0, 4.0, 15.0);
        let o = s.offset(0.5, 1.0, 1e-7).unwrap();
        let slant = (1.0_f64 + 0.16).sqrt();
        let mut worst = 0.0_f64;
        for i in 0..=40 {
            for j in 0..=40 {
                let uv = DVec2::new(f64::from(i) / 40.0, f64::from(j) / 40.0);
                let p = o.point(uv);
                let expected = 10.0 - 0.4 * p.z + 0.5 * slant;
                worst = worst.max((p.truncate().length() - expected).abs());
                assert!(o.normal(uv).dot(s.normal(uv)) > 0.999);
            }
        }
        assert!(worst < 1.5e-7, "{worst}");
    }

    #[test]
    fn offsets_that_fold_are_refused() {
        let s = cone(4.0, 4.0, 10.0);
        // Outwards is fine at any distance; inwards only up to the radius.
        assert!(s.offset(20.0, 0.0, 1e-7).is_ok());
        let Err(OffsetError::Folds { radius }) = s.offset(-4.5, 0.0, 1e-7) else {
            panic!("an offset past the axis");
        };
        assert!((radius - 4.0).abs() < 1e-6, "{radius}");
        assert_eq!(s.tightest_radius(true), None);
        assert!((s.tightest_radius(false).unwrap() - 4.0).abs() < 1e-9);
    }

    #[test]
    fn fitted_curves_follow_their_shape() {
        // A helix with a corner in it at t = 2: smooth on both sides of the break.
        let shape = |t: f64| {
            let lift = if t < 2.0 { t } else { 4.0 - t };
            Some(DVec3::new(3.0 * t.cos(), 3.0 * t.sin(), lift))
        };
        let c = NurbsCurve::fitted(&shape, &[0.0, 2.0, 5.0], 5, 1e-8).unwrap();
        assert_eq!((c.degree(), c.domain()), (5, (0.0, 5.0)));
        for i in 0..=500 {
            let t = f64::from(i) / 100.0;
            assert!(c.point(t).distance(shape(t).unwrap()) < 2e-8, "{t}");
        }
        // Without the break the corner can't be followed.
        assert!(NurbsCurve::fitted(&shape, &[0.0, 5.0], 5, 1e-8).is_err());
        // The caller's own measure: only the distance from the cylinder counts.
        let miss = |_: f64, p: DVec3| Some((p.truncate().length() - 3.0).abs());
        let loose = NurbsCurve::fitted_within(&shape, &miss, &[0.0, 2.0, 5.0], 5, 1e-8).unwrap();
        assert!(loose.knots().len() <= c.knots().len());
    }

    #[test]
    fn fits_follow_their_shape() {
        // A ruled surface on a sine wave: degree 1 across is exact, along it is fitted.
        let shape = |uv: DVec2| Some(DVec3::new(uv.x, uv.x.sin() * 3.0, uv.y * (1.0 + uv.x)));
        let s = NurbsSurface::fitted(&shape, &[0.0, 2.0, 5.0], &[-1.0, 1.0], (5, 1), 1e-8).unwrap();
        assert_eq!(s.degrees(), (5, 1));
        assert_eq!(s.counts().1, 2);
        for i in 0..=100 {
            for j in 0..=10 {
                let uv = DVec2::new(5.0 * f64::from(i) / 100.0, -1.0 + 0.2 * f64::from(j));
                assert!(s.point(uv).distance(shape(uv).unwrap()) < 2e-8);
            }
        }
        // A crease that is not at a break can't be followed.
        let creased = |uv: DVec2| Some(DVec3::new(uv.x, (uv.x - 1.0).abs(), uv.y));
        assert!(NurbsSurface::fitted(&creased, &[0.0, 2.0], &[0.0, 1.0], (5, 1), 1e-8).is_err());
        assert!(
            NurbsSurface::fitted(&creased, &[0.0, 1.0, 2.0], &[0.0, 1.0], (5, 1), 1e-8).is_ok()
        );
    }
}
