//! Clipping a curve to faces: the parts of the curve that lie in (or on the boundary of)
//! every given face.

use std::f64::consts::{PI, TAU};

use peet_math::tolerance::LINEAR;
use peet_math::{DVec2, DVec3};

use super::domain::{BoundaryEdge, FaceGeom, Where};
use super::roots::{Implicit, Roots, solve};
use super::util::{bounded_distance, boxes_overlap, curve_bounds, grow, param_from, param_tol};
use crate::geom::{Curve3, Surface};

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

    let mut breaks: Vec<Break> = Vec::new();
    for (i, f) in faces.iter().enumerate() {
        let others: Vec<&FaceGeom> = faces
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != i)
            .map(|(_, g)| *g)
            .collect();
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
                        && let Some(t) = place(refine(curve, t, e, &others))
                    {
                        let fitted = matches!(e.curve, Curve3::Nurbs(_));
                        breaks.push(Break {
                            t,
                            rank: if fitted { 0 } else { 1 },
                            fitted: if fitted { vec![e] } else { Vec::new() },
                        });
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
                breaks.push(Break {
                    t,
                    rank: 2,
                    fitted: Vec::new(),
                });
            }
        }
    }
    if !periodic {
        for t in [lo, hi] {
            breaks.push(Break {
                t,
                rank: 1,
                fitted: Vec::new(),
            });
        }
    }
    breaks.sort_by(|a, b| a.t.total_cmp(&b.t));
    // Merge breaks that are the same point. Where the curve runs along a fitted edge
    // (within tolerance of it all the way between two breaks), those are one point too:
    // the fit's error moved where the curve seems to meet it, or made it seem to cross
    // where the two only touch. The most exact of them stays.
    let mut kept: Vec<Break> = Vec::with_capacity(breaks.len());
    for b in breaks {
        kept.push(b);
        // A merged break can reach back further along a fitted edge it brought along.
        while let [.., a, b] = &kept[..]
            && (same_point(curve, a.t, b.t) || along_fitted(curve, a, b))
        {
            let b = kept.pop().expect("two breaks");
            kept.last_mut().expect("two breaks").absorb(b);
        }
    }
    // Round a closed curve, the last break and the first can be one point too.
    if periodic && kept.len() > 1 {
        let last = &kept[kept.len() - 1];
        let before = Break {
            t: last.t - TAU,
            ..last.clone()
        };
        if same_point(curve, kept[0].t, last.t) || along_fitted(curve, &before, &kept[0]) {
            kept.pop();
            kept[0].absorb(before);
        }
    }
    let cuts: Vec<f64> = kept.iter().map(|b| b.t).collect();

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

/// A parameter where a clipped curve is cut.
#[derive(Clone)]
struct Break<'a> {
    t: f64,
    /// How exact it is: a vertex 2, a crossing with an exact edge (or an end of the span)
    /// 1, a crossing with a fitted edge 0.
    rank: u8,
    /// The fitted edges it is a crossing with.
    fitted: Vec<&'a BoundaryEdge>,
}

impl<'a> Break<'a> {
    /// Takes in another break at the same point, keeping the more exact parameter.
    fn absorb(&mut self, other: Break<'a>) {
        if other.rank > self.rank {
            (self.t, self.rank) = (other.t, other.rank);
        }
        self.fitted.extend(other.fitted);
    }
}

/// Whether `curve` stays within tolerance of a fitted edge either of two breaks is on,
/// all the way from one to the other.
fn along_fitted(curve: &Curve3, a: &Break, b: &Break) -> bool {
    const SAMPLES: usize = 8;
    a.fitted.iter().chain(&b.fitted).any(|e| {
        (0..=SAMPLES).all(|i| {
            let p = curve.point(a.t + (b.t - a.t) * i as f64 / SAMPLES as f64);
            bounded_distance(&e.curve, e.t0, e.t1, p).0 <= LINEAR
        })
    })
}

/// Where a traced curve meets a boundary edge, found again from exact geometry.
///
/// A traced curve (a freeform fit of where two surfaces cross) is only within a fraction
/// of `LINEAR` of the true crossing, and its direction is only as good as that over the
/// length of a piece. Where it crosses an edge steeply that hardly matters, but where it
/// touches the edge (a cylinder meeting another just where that one's seam runs) the
/// point of contact slides along both by that error over the curvature: microns, enough
/// to leave a sliver edge beside the exact vertex a later operation puts there. The
/// point is on the edge and on the other faces' surfaces, and those are exact: the point
/// of the edge nearest `curve.point(t)` that is on all of them is taken instead.
fn refine(curve: &Curve3, t: f64, e: &BoundaryEdge, others: &[&FaceGeom]) -> f64 {
    let exact = |c: &Curve3| !matches!(c, Curve3::Nurbs(_));
    if exact(curve)
        || !exact(&e.curve)
        || others.is_empty()
        || others
            .iter()
            .any(|f| matches!(f.surface, Surface::Nurbs(_)))
    {
        return t;
    }
    let off = |s: f64| {
        let q = e.curve.point(s);
        others
            .iter()
            .map(|f| f.surface.signed_distance(q).abs())
            .sum::<f64>()
    };
    // A search a little wider than the error, along the edge.
    let p = curve.point(t);
    let (_, s) = bounded_distance(&e.curve, e.t0, e.t1, p);
    let reach = REFINE_REACH * param_tol(&e.curve);
    let (lo, hi) = ((s - reach).max(e.t0), (s + reach).min(e.t1));
    // An edge lying in the other surface has no one point to find. (Near a point where
    // it touches the surface it stays within tolerance for a while, so look at all of it.)
    if [0.0, 0.25, 0.5, 0.75, 1.0]
        .iter()
        .all(|f| off(e.t0 + (e.t1 - e.t0) * f) <= LINEAR)
    {
        return t;
    }
    const GOLD: f64 = 0.618_033_988_749_894_8;
    let (mut x0, mut x1) = (lo, hi);
    for _ in 0..100 {
        let (m0, m1) = (x1 - GOLD * (x1 - x0), x0 + GOLD * (x1 - x0));
        if off(m0) < off(m1) {
            x1 = m1;
        } else {
            x0 = m0;
        }
    }
    let s = 0.5 * (x0 + x1);
    let q = e.curve.point(s);
    if off(s) > 0.01 * LINEAR || curve.distance(q) > LINEAR {
        return t;
    }
    curve.param(q)
}

/// How far (in multiples of `LINEAR`) [`refine`] looks along an edge.
const REFINE_REACH: f64 = 100.0;

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
