//! Resolved 2D curve geometry: evaluation, closest points and intersections.
//!
//! [`Curve`] is the geometry of a sketch line, circle or arc with coordinates filled in
//! (the sketch itself stores curves as references to point entities). Everything here is
//! plain `f64` math with no knowledge of ids or constraints, used by the editing
//! operations, region detection, hit testing and rendering.
//!
//! **Parameters.** Every curve has a parameter `t`:
//! - line: `a + t (b - a)`, with the segment at `0..=1` (the infinite line extends beyond),
//! - arc: `start_angle + t * sweep`, with the arc at `0..=1` (the rest of the circle beyond),
//! - circle: angle / 2π measured counter-clockwise from +X, in `0..1`.

use std::f64::consts::TAU;

use peet_math::{DVec2, tolerance};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    Line {
        a: DVec2,
        b: DVec2,
    },
    Circle {
        center: DVec2,
        radius: f64,
    },
    /// Counter-clockwise arc. `sweep` is in `(0, 2π]`.
    Arc {
        center: DVec2,
        radius: f64,
        start_angle: f64,
        sweep: f64,
    },
}

/// A point where two curves meet, with the parameter on each.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Intersection {
    pub point: DVec2,
    pub t_a: f64,
    pub t_b: f64,
}

/// Normalizes an angle to `[0, 2π)`.
pub fn normalize_angle(a: f64) -> f64 {
    let r = a.rem_euclid(TAU);
    if r >= TAU { 0.0 } else { r }
}

impl Curve {
    /// Counter-clockwise arc from `start` to `end` around `center` (radius from `start`).
    /// Coincident start and end give a full circle's sweep.
    pub fn arc_from_points(center: DVec2, start: DVec2, end: DVec2) -> Self {
        let radius = start.distance(center);
        let a0 = (start - center).to_angle();
        let a1 = (end - center).to_angle();
        let mut sweep = normalize_angle(a1 - a0);
        if sweep * radius <= tolerance::LINEAR {
            sweep = TAU;
        }
        Self::Arc {
            center,
            radius,
            start_angle: a0,
            sweep,
        }
    }

    /// Point at parameter `t` (see the module docs). Works outside `0..=1` too.
    pub fn point_at(&self, t: f64) -> DVec2 {
        match *self {
            Self::Line { a, b } => a + (b - a) * t,
            Self::Circle { center, radius } => center + DVec2::from_angle(t * TAU) * radius,
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep,
            } => center + DVec2::from_angle(start_angle + t * sweep) * radius,
        }
    }

    /// Unit tangent at `t`, in the direction of increasing `t`.
    pub fn tangent_at(&self, t: f64) -> DVec2 {
        match *self {
            Self::Line { a, b } => (b - a).normalize_or(DVec2::X),
            Self::Circle { .. } => DVec2::from_angle(t * TAU).perp(),
            Self::Arc {
                start_angle, sweep, ..
            } => DVec2::from_angle(start_angle + t * sweep).perp(),
        }
    }

    /// Start point (lines and arcs). A circle "starts" at angle 0.
    pub fn start(&self) -> DVec2 {
        self.point_at(0.0)
    }

    /// End point (lines and arcs). A circle ends where it starts.
    pub fn end(&self) -> DVec2 {
        match self {
            Self::Circle { .. } => self.point_at(0.0),
            _ => self.point_at(1.0),
        }
    }

    pub fn is_closed(&self) -> bool {
        matches!(self, Self::Circle { .. })
    }

    pub fn center(&self) -> Option<DVec2> {
        match *self {
            Self::Line { .. } => None,
            Self::Circle { center, .. } | Self::Arc { center, .. } => Some(center),
        }
    }

    pub fn radius(&self) -> Option<f64> {
        match *self {
            Self::Line { .. } => None,
            Self::Circle { radius, .. } | Self::Arc { radius, .. } => Some(radius),
        }
    }

    /// Unit direction of a line (`None` for circular curves or a zero length line).
    pub fn line_direction(&self) -> Option<DVec2> {
        match *self {
            Self::Line { a, b } => (b - a).try_normalize(),
            _ => None,
        }
    }

    pub fn length(&self) -> f64 {
        match *self {
            Self::Line { a, b } => a.distance(b),
            Self::Circle { radius, .. } => TAU * radius,
            Self::Arc { radius, sweep, .. } => radius * sweep,
        }
    }

    /// Perpendicular distance from `p` to the infinite line through a line segment. For
    /// circular curves: distance to the full circle.
    pub fn distance_to_line(&self, p: DVec2) -> f64 {
        match *self {
            Self::Line { a, b } => match (b - a).try_normalize() {
                Some(d) => d.perp_dot(p - a).abs(),
                None => p.distance(a),
            },
            Self::Circle { center, radius } | Self::Arc { center, radius, .. } => {
                (p.distance(center) - radius).abs()
            }
        }
    }

    /// Parameter of the point on the *unbounded* curve (infinite line, full circle) closest
    /// to `p`. For arcs the result is in `[0, 2π / sweep)`.
    pub fn project(&self, p: DVec2) -> f64 {
        match *self {
            Self::Line { a, b } => {
                let d = b - a;
                let len2 = d.length_squared();
                if len2 <= 0.0 {
                    0.0
                } else {
                    (p - a).dot(d) / len2
                }
            }
            Self::Circle { center, .. } => normalize_angle((p - center).to_angle()) / TAU,
            Self::Arc {
                center,
                start_angle,
                sweep,
                ..
            } => normalize_angle((p - center).to_angle() - start_angle) / sweep,
        }
    }

    /// Whether `t` lies on the bounded curve (within the linear tolerance at the ends).
    pub fn contains_param(&self, t: f64) -> bool {
        match self {
            Self::Circle { .. } => true,
            _ => {
                let slack = tolerance::LINEAR / self.length().max(tolerance::LINEAR);
                (-slack..=1.0 + slack).contains(&t)
            }
        }
    }

    /// Closest point on the *bounded* curve to `p`, as `(t, point)`.
    pub fn closest_point(&self, p: DVec2) -> (f64, DVec2) {
        match *self {
            Self::Line { .. } => {
                let t = self.project(p).clamp(0.0, 1.0);
                (t, self.point_at(t))
            }
            Self::Circle { .. } => {
                let t = self.project(p);
                (t, self.point_at(t))
            }
            Self::Arc { sweep, .. } => {
                let t = self.project(p);
                if t <= 1.0 {
                    return (t, self.point_at(t));
                }
                // Outside the arc: the nearer endpoint (compare angular gaps past each end).
                let past_end = (t - 1.0) * sweep;
                let before_start = TAU - t * sweep;
                let t = if past_end < before_start { 1.0 } else { 0.0 };
                (t, self.point_at(t))
            }
        }
    }

    /// Distance from `p` to the bounded curve.
    pub fn distance(&self, p: DVec2) -> f64 {
        self.closest_point(p).1.distance(p)
    }

    /// Intersections of the *bounded* curves (segments and arcs as drawn). Tangent contacts
    /// are reported once. Overlapping collinear/co-circular curves report no points.
    pub fn intersect(&self, other: &Curve) -> Vec<Intersection> {
        self.intersect_unbounded(other)
            .into_iter()
            .filter(|i| self.contains_param(i.t_a) && other.contains_param(i.t_b))
            .map(|mut i| {
                i.t_a = clamp_param(self, i.t_a);
                i.t_b = clamp_param(other, i.t_b);
                i
            })
            .collect()
    }

    /// Intersections of the *unbounded* curves (infinite lines, full circles). Parameters
    /// may be outside `0..=1` (see [`Curve::project`] for arcs).
    pub fn intersect_unbounded(&self, other: &Curve) -> Vec<Intersection> {
        let points = match (self.carrier(), other.carrier()) {
            (Carrier::Line(p, d), Carrier::Line(q, e)) => line_line(p, d, q, e),
            (Carrier::Line(p, d), Carrier::Circle(c, r))
            | (Carrier::Circle(c, r), Carrier::Line(p, d)) => line_circle(p, d, c, r),
            (Carrier::Circle(c1, r1), Carrier::Circle(c2, r2)) => circle_circle(c1, r1, c2, r2),
        };
        points
            .into_iter()
            .map(|point| Intersection {
                point,
                t_a: self.project(point),
                t_b: other.project(point),
            })
            .collect()
    }

    /// Splits the curve at the given parameters (inside `0..1`), returning the pieces in
    /// order. A circle split at `n >= 2` parameters gives `n` arcs; at one, one arc.
    pub fn split(&self, params: &[f64]) -> Vec<Curve> {
        let mut ts: Vec<f64> = params.iter().copied().filter(|t| t.is_finite()).collect();
        ts.sort_by(f64::total_cmp);
        ts.dedup_by(|a, b| (*a - *b).abs() * self.length() <= tolerance::LINEAR);
        match *self {
            Self::Circle { center, radius } => {
                if ts.is_empty() {
                    return vec![*self];
                }
                let n = ts.len();
                (0..n)
                    .map(|i| {
                        let a0 = ts[i] * TAU;
                        let a1 = if i + 1 < n {
                            ts[i + 1] * TAU
                        } else {
                            ts[0] * TAU + TAU
                        };
                        Self::Arc {
                            center,
                            radius,
                            start_angle: a0,
                            sweep: (a1 - a0).clamp(0.0, TAU),
                        }
                    })
                    .filter(|c| c.length() > tolerance::LINEAR)
                    .collect()
            }
            _ => {
                let inner: Vec<f64> = ts
                    .into_iter()
                    .filter(|t| {
                        let len = self.length();
                        *t * len > tolerance::LINEAR && (1.0 - *t) * len > tolerance::LINEAR
                    })
                    .collect();
                let mut bounds = vec![0.0];
                bounds.extend(inner);
                bounds.push(1.0);
                bounds
                    .windows(2)
                    .map(|w| self.sub_curve(w[0], w[1]))
                    .collect()
            }
        }
    }

    /// The part of a line or arc between parameters `t0 < t1`. For a circle, the arc from
    /// `t0` to `t1` counter-clockwise.
    pub fn sub_curve(&self, t0: f64, t1: f64) -> Curve {
        match *self {
            Self::Line { .. } => Self::Line {
                a: self.point_at(t0),
                b: self.point_at(t1),
            },
            Self::Circle { center, radius } => Self::Arc {
                center,
                radius,
                start_angle: t0 * TAU,
                sweep: normalize_angle((t1 - t0) * TAU).max(f64::MIN_POSITIVE),
            },
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep,
            } => Self::Arc {
                center,
                radius,
                start_angle: start_angle + t0 * sweep,
                sweep: (t1 - t0) * sweep,
            },
        }
    }

    /// Axis-aligned bounds `(min, max)` of the bounded curve.
    pub fn bounds(&self) -> (DVec2, DVec2) {
        match *self {
            Self::Line { a, b } => (a.min(b), a.max(b)),
            Self::Circle { center, radius } => (center - radius, center + radius),
            Self::Arc {
                center,
                radius,
                start_angle,
                sweep,
            } => {
                let mut min = self.start().min(self.end());
                let mut max = self.start().max(self.end());
                // Include each axis extreme the arc passes through.
                for k in 0..4 {
                    let angle = f64::from(k) * TAU / 4.0;
                    if normalize_angle(angle - start_angle) <= sweep {
                        let p = center + DVec2::from_angle(angle) * radius;
                        min = min.min(p);
                        max = max.max(p);
                    }
                }
                (min, max)
            }
        }
    }

    /// Points along the curve such that the chord error stays below `tolerance`, including
    /// both ends (a circle repeats its first point at the end).
    pub fn tessellate(&self, tolerance: f64) -> Vec<DVec2> {
        match *self {
            Self::Line { a, b } => vec![a, b],
            Self::Circle { radius, .. } | Self::Arc { radius, .. } => {
                let sweep = match *self {
                    Self::Arc { sweep, .. } => sweep,
                    _ => TAU,
                };
                // Chord error of a segment spanning angle θ is r (1 - cos(θ/2)).
                let tol = tolerance.max(1e-9).min(radius);
                let max_step = 2.0 * (1.0 - tol / radius.max(1e-12)).clamp(-1.0, 1.0).acos();
                let n = ((sweep / max_step.max(1e-3)).ceil() as usize).clamp(4, 1024);
                (0..=n)
                    .map(|i| self.point_at(i as f64 / n as f64))
                    .collect()
            }
        }
    }

    fn carrier(&self) -> Carrier {
        match *self {
            Self::Line { a, b } => Carrier::Line(a, b - a),
            Self::Circle { center, radius } | Self::Arc { center, radius, .. } => {
                Carrier::Circle(center, radius)
            }
        }
    }
}

fn clamp_param(c: &Curve, t: f64) -> f64 {
    match c {
        Curve::Circle { .. } => t,
        _ => t.clamp(0.0, 1.0),
    }
}

enum Carrier {
    /// Point and (non-normalized) direction.
    Line(DVec2, DVec2),
    Circle(DVec2, f64),
}

fn line_line(p: DVec2, d: DVec2, q: DVec2, e: DVec2) -> Vec<DVec2> {
    let denom = d.perp_dot(e);
    let scale = d.length() * e.length();
    if scale <= 0.0 || denom.abs() <= tolerance::ANGULAR * scale {
        return Vec::new();
    }
    let t = (q - p).perp_dot(e) / denom;
    vec![p + d * t]
}

fn line_circle(p: DVec2, d: DVec2, c: DVec2, r: f64) -> Vec<DVec2> {
    let Some(u) = d.try_normalize() else {
        return Vec::new();
    };
    // Foot of the perpendicular from the centre.
    let foot = p + u * (c - p).dot(u);
    let h = foot.distance(c);
    if h > r + tolerance::LINEAR {
        Vec::new()
    } else if (h - r).abs() <= tolerance::LINEAR {
        vec![foot]
    } else {
        let s = (r * r - h * h).max(0.0).sqrt();
        vec![foot - u * s, foot + u * s]
    }
}

fn circle_circle(c1: DVec2, r1: f64, c2: DVec2, r2: f64) -> Vec<DVec2> {
    let d = c1.distance(c2);
    if d <= tolerance::LINEAR {
        return Vec::new(); // concentric: none, or infinitely many
    }
    if d > r1 + r2 + tolerance::LINEAR || d < (r1 - r2).abs() - tolerance::LINEAR {
        return Vec::new();
    }
    let u = (c2 - c1) / d;
    let a = (d * d + r1 * r1 - r2 * r2) / (2.0 * d);
    let base = c1 + u * a;
    let h2 = r1 * r1 - a * a;
    if h2 <= (tolerance::LINEAR * (r1 + r2)).max(0.0) {
        // Tangent (externally or internally).
        return vec![base];
    }
    let h = h2.sqrt();
    vec![base + u.perp() * h, base - u.perp() * h]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    fn close(a: DVec2, b: DVec2) -> bool {
        a.abs_diff_eq(b, 1e-9)
    }

    #[test]
    fn arc_parameters() {
        let arc = Curve::arc_from_points(DVec2::ZERO, DVec2::X, DVec2::Y);
        let Curve::Arc { sweep, radius, .. } = arc else {
            panic!()
        };
        assert!((sweep - FRAC_PI_2).abs() < 1e-12);
        assert!((radius - 1.0).abs() < 1e-12);
        assert!(close(arc.point_at(0.5), DVec2::new(1.0, 1.0).normalize()));
        assert!((arc.project(DVec2::new(-1.0, 0.0)) - 2.0).abs() < 1e-12);
        assert!(!arc.contains_param(2.0));
        // Closest point on the arc from outside its span: nearest endpoint.
        assert_eq!(arc.closest_point(DVec2::new(0.1, -5.0)).0, 0.0);
        assert_eq!(arc.closest_point(DVec2::new(-5.0, 0.1)).0, 1.0);
        let (min, max) = arc.bounds();
        assert!(close(min, DVec2::ZERO) && close(max, DVec2::ONE));
    }

    #[test]
    fn line_intersections() {
        let a = Curve::Line {
            a: DVec2::new(-1.0, 0.0),
            b: DVec2::new(1.0, 0.0),
        };
        let b = Curve::Line {
            a: DVec2::new(0.0, -1.0),
            b: DVec2::new(0.0, 1.0),
        };
        let hits = a.intersect(&b);
        assert_eq!(hits.len(), 1);
        assert!(close(hits[0].point, DVec2::ZERO));
        assert!((hits[0].t_a - 0.5).abs() < 1e-12);
        let short = Curve::Line {
            a: DVec2::new(0.0, 1.0),
            b: DVec2::new(0.0, 2.0),
        };
        assert!(a.intersect(&short).is_empty());
        assert_eq!(a.intersect_unbounded(&short).len(), 1);
        assert!(a.intersect(&a).is_empty(), "parallel lines");
    }

    #[test]
    fn circle_intersections() {
        let c = Curve::Circle {
            center: DVec2::ZERO,
            radius: 1.0,
        };
        let l = Curve::Line {
            a: DVec2::new(-2.0, 0.0),
            b: DVec2::new(2.0, 0.0),
        };
        let hits = l.intersect(&c);
        assert_eq!(hits.len(), 2);
        let tangent = Curve::Line {
            a: DVec2::new(-2.0, 1.0),
            b: DVec2::new(2.0, 1.0),
        };
        assert_eq!(tangent.intersect(&c).len(), 1);
        let c2 = Curve::Circle {
            center: DVec2::new(1.0, 0.0),
            radius: 1.0,
        };
        let hits = c.intersect(&c2);
        assert_eq!(hits.len(), 2);
        for h in hits {
            assert!((h.point.x - 0.5).abs() < 1e-12);
        }
        let touching = Curve::Circle {
            center: DVec2::new(2.0, 0.0),
            radius: 1.0,
        };
        assert_eq!(c.intersect(&touching).len(), 1);
        // Upper half arc meets the x axis only at its ends.
        let arc = Curve::Arc {
            center: DVec2::ZERO,
            radius: 1.0,
            start_angle: 0.0,
            sweep: PI,
        };
        let below = Curve::Line {
            a: DVec2::new(-2.0, -0.5),
            b: DVec2::new(2.0, -0.5),
        };
        assert!(arc.intersect(&below).is_empty());
        assert_eq!(arc.intersect(&l).len(), 2);
    }

    #[test]
    fn splitting() {
        let l = Curve::Line {
            a: DVec2::ZERO,
            b: DVec2::new(4.0, 0.0),
        };
        let parts = l.split(&[0.75, 0.25, 0.0, 1.0]);
        assert_eq!(parts.len(), 3);
        assert!(close(parts[1].start(), DVec2::new(1.0, 0.0)));
        let c = Curve::Circle {
            center: DVec2::ZERO,
            radius: 1.0,
        };
        let arcs = c.split(&[0.0, 0.5]);
        assert_eq!(arcs.len(), 2);
        let total: f64 = arcs.iter().map(Curve::length).sum();
        assert!((total - TAU).abs() < 1e-9);
    }

    #[test]
    fn tessellation_tolerance() {
        let c = Curve::Circle {
            center: DVec2::ZERO,
            radius: 100.0,
        };
        let pts = c.tessellate(0.01);
        for w in pts.windows(2) {
            let mid = (w[0] + w[1]) * 0.5;
            assert!(100.0 - mid.length() <= 0.0101);
        }
        assert!(close(pts[0], *pts.last().unwrap()));
    }
}
