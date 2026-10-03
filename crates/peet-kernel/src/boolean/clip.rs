//! Clipping a curve to faces: the parts of the curve that lie in (or on the boundary of)
//! every given face.

use std::f64::consts::{PI, TAU};

use peet_math::tolerance::LINEAR;
use peet_math::{DVec2, DVec3};

use super::domain::{FaceGeom, Where};
use super::roots::{Implicit, Roots, solve};
use super::util::{bounded_distance, boxes_overlap, curve_bounds, grow, param_from, param_tol};
use crate::geom::Curve3;

/// The part of a curve to clip.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Span {
    /// From one parameter to another (a line's search range, or an edge).
    Interval(f64, f64),
    /// A whole closed curve.
    Periodic,
}

/// Parameter intervals `(t0, t1)`, `t0 < t1`, of `curve` (which lies on every face's
/// surface) inside or on the boundary of all `faces`. The curve is cut wherever it meets a
/// boundary edge or passes through a vertex of a face, so the pieces end exactly at those
/// points, and seams cut closed curves.
pub(crate) fn clip(curve: &Curve3, span: Span, faces: &[&FaceGeom]) -> Vec<(f64, f64)> {
    let tol = param_tol(curve);
    let (lo, hi) = match span {
        Span::Interval(a, b) => (a, b),
        Span::Periodic => (-PI, PI),
    };
    let periodic = matches!(span, Span::Periodic);
    let curve_box = match span {
        Span::Interval(a, b) => grow(&curve_bounds(curve, a, b), LINEAR),
        Span::Periodic => grow(&curve_bounds(curve, -PI, PI), LINEAR),
    };
    // A parameter brought into the span, if it is in it.
    let place = |t: f64| -> Option<f64> {
        if periodic {
            Some(lo + (t - lo).rem_euclid(TAU))
        } else {
            let t = if curve.period().is_some() {
                lo - tol + (t - lo + tol).rem_euclid(TAU)
            } else {
                t
            };
            (t >= lo - tol && t <= hi + tol).then(|| t.clamp(lo, hi))
        }
    };

    let mut breaks: Vec<f64> = Vec::new();
    for f in faces {
        for e in &f.edges {
            if !boxes_overlap(&curve_box, &e.bounds) {
                continue;
            }
            let Some(implicit) = Implicit::of(&e.curve, &f.surface) else {
                continue;
            };
            if let Roots::At(ts) = solve(curve, &implicit, (lo, hi)) {
                for t in ts {
                    let p = curve.point(t);
                    if bounded_distance(&e.curve, e.t0, e.t1, p).0 <= 2.0 * LINEAR
                        && let Some(t) = place(t)
                    {
                        breaks.push(t);
                    }
                }
            }
        }
        // A curve through a pole of the face's surface (a sphere's pole, a cone's apex) is
        // cut there too: poles are always vertices, never inside an edge, so every edge
        // has one image in the surface's parameters.
        let poles = f
            .surface
            .poles()
            .into_iter()
            .flatten()
            .map(|pole| f.surface.point(DVec2::new(0.0, pole.v)));
        for v in f.vertices.iter().copied().chain(poles) {
            if curve_box.contains(v)
                && curve.distance(v) <= LINEAR
                && let Some(t) = place(curve.param(v))
            {
                breaks.push(t);
            }
        }
    }
    if !periodic {
        breaks.push(lo);
        breaks.push(hi);
    }
    breaks.sort_by(f64::total_cmp);
    // Merge breaks that are the same point.
    let mut cuts: Vec<f64> = Vec::with_capacity(breaks.len());
    for t in breaks {
        match cuts.last() {
            Some(&last) if same_point(curve, last, t) => {}
            _ => cuts.push(t),
        }
    }
    if periodic && cuts.len() > 1 && same_point(curve, cuts[0], cuts[cuts.len() - 1]) {
        cuts.pop();
    }

    let mut intervals: Vec<(f64, f64)> = Vec::new();
    if periodic {
        match cuts.len() {
            0 => intervals.push((lo, lo + TAU)),
            n => {
                for i in 0..n {
                    let a = cuts[i];
                    let b = if i + 1 < n {
                        cuts[i + 1]
                    } else {
                        cuts[0] + TAU
                    };
                    intervals.push((a, b));
                }
            }
        }
    } else {
        for w in cuts.windows(2) {
            intervals.push((w[0], w[1]));
        }
    }
    intervals.retain(|&(a, b)| {
        let mid = curve.point(0.5 * (a + b));
        faces.iter().all(|f| f.locate(mid) != Where::Outside)
    });
    intervals
}

fn same_point(curve: &Curve3, a: f64, b: f64) -> bool {
    curve.point(a).distance_squared(curve.point(b)) <= LINEAR * LINEAR
}

/// The parameter range of a line that can matter inside `bounds` (a box), with a margin.
pub(crate) fn line_range(curve: &Curve3, bounds: &peet_math::Aabb) -> Option<(f64, f64)> {
    let Curve3::Line(l) = curve else {
        return None;
    };
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for i in 0..8 {
        let corner = DVec3::new(
            if i & 1 == 0 {
                bounds.min.x
            } else {
                bounds.max.x
            },
            if i & 2 == 0 {
                bounds.min.y
            } else {
                bounds.max.y
            },
            if i & 4 == 0 {
                bounds.min.z
            } else {
                bounds.max.z
            },
        );
        let t = (corner - l.origin).dot(l.dir);
        lo = lo.min(t);
        hi = hi.max(t);
    }
    let margin = 1e-3 * (hi - lo) + 16.0 * LINEAR;
    Some((lo - margin, hi + margin))
}

/// Parameter of `p` on an edge, when `p` lies strictly inside it (not at its ends).
pub(crate) fn interior_param(curve: &Curve3, t0: f64, t1: f64, p: DVec3) -> Option<f64> {
    let t = param_from(curve, p, t0);
    (t > t0 && t < t1 && curve.point(t).distance(p) <= LINEAR).then_some(t)
}
