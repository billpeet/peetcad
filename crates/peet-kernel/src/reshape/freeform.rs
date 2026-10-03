//! The parts of offset, shell and draft that have no closed form: edges between
//! surfaces whose intersection is not a line, a circle or an ellipse, and the draft
//! surface of a freeform face.
//!
//! **Edges.** The new edge is not found by tracing the two new surfaces' whole
//! intersection. The old edge says where to look: at each of a number of places along
//! it, the plane square to the old edge there (moved along with the edge's ends) is cut
//! with the two new surfaces by Newton's method, which gives one point of the new edge.
//! The points must run from the new start vertex to the new end vertex the way the old
//! edge ran. A line or a circle is put back where the old edge was one and the new
//! points still lie on one; otherwise a quintic is fitted through the points
//! ([`NurbsCurve::fitted_within`]), with more of them wherever it leaves either surface
//! by more than [`CURVE_FIT`]. The fit is in separate pieces between the places where
//! the edge crosses a crease of one of its surfaces, since the edge has a kink in its
//! curvature there.
//!
//! **Nearly tangent faces.** Two faces that meet smoothly (the sides of a loft where it
//! runs into a circle) have surfaces that barely cross, and their fitted offsets, each a
//! fraction of the tolerance from its true shape, may not cross at all. A vertex there
//! is the point nearest to both, and the edge any smooth curve between them, to
//! [`TANGENT_FIT`]; where exactly it runs across the two surfaces is not determined, and
//! does not matter, since they are the same surface there to that tolerance.
//!
//! **Draft.** A freeform face is replaced by the ruled surface through its neutral curve
//! (where it crosses the neutral plane): through every point of the curve a straight
//! line, tilted from the direction of pull by the draft angle about the curve's tangent.
//! The rulings' directions are not polynomial, so the surface is fitted
//! ([`NurbsSurface::fitted`]): of degree 1 along the rulings, which is exact.

use std::cell::RefCell;
use std::f64::consts::TAU;
use std::sync::Arc;

use peet_math::tolerance::LINEAR;
use peet_math::{DMat3, DVec2, DVec3, Frame, Plane};

use crate::boolean::ssi::{self, Ssi};
use crate::geom::{Circle3, Curve3, Surface};
use crate::nurbs::fit::OffsetError;
use crate::nurbs::{NurbsCurve, NurbsSurface};
use crate::topo::{Edge, FaceId};
use crate::{KernelError, Solid};

/// A fitted surface follows its shape to within this (mm) where the fit is tested.
pub(super) const SURFACE_FIT: f64 = 0.2 * LINEAR;

/// A fitted edge curve may leave either of its surfaces by this much (mm).
pub(super) const CURVE_FIT: f64 = 0.2 * LINEAR;

/// What a fitted edge curve may leave its surfaces by (mm) where they are nearly tangent
/// to each other (the sine of the angle between them under [`NEARLY_TANGENT`]): there
/// two fitted surfaces, each within [`SURFACE_FIT`] of its shape, need not cross at all.
const TANGENT_FIT: f64 = 4.0 * LINEAR;

/// See [`TANGENT_FIT`].
const NEARLY_TANGENT: f64 = 0.02;

/// A point counts as within a freeform surface's own extent if it is this close to the
/// surface's nearest point (mm), and two fitted surfaces as meeting if they come this
/// close.
pub(super) const ON_FACE: f64 = LINEAR;

/// A surface as Newton's method sees it. For a freeform surface the parameters of the
/// last point looked at are kept, so that the next closest-point search starts there.
pub(super) struct Probe<'a> {
    surface: &'a Surface,
    uv: Option<DVec2>,
}

/// What a probe says about a point.
pub(super) struct Reading {
    /// Signed distance from the surface, along its normal.
    pub distance: f64,
    pub normal: DVec3,
    /// Distance from the nearest point of the surface itself. Greater than the signed
    /// distance beside a freeform surface's edge, where the surface has ended.
    pub gap: f64,
}

impl<'a> Probe<'a> {
    pub fn new(surface: &'a Surface) -> Self {
        Self { surface, uv: None }
    }

    fn from(surface: &'a Surface, uv: Option<DVec2>) -> Self {
        Self { surface, uv }
    }

    pub fn read(&mut self, p: DVec3) -> Reading {
        match self.surface {
            Surface::Nurbs(s) => {
                let uv = match self.uv {
                    Some(near) => s.param_from(p, near),
                    None => s.param(p),
                };
                self.uv = Some(uv);
                let (q, normal) = (s.point(uv), s.normal(uv));
                Reading {
                    distance: (p - q).dot(normal),
                    normal,
                    gap: p.distance(q),
                }
            }
            analytic => {
                let distance = analytic.signed_distance(p);
                Reading {
                    distance,
                    normal: analytic.normal_at(p),
                    gap: distance.abs(),
                }
            }
        }
    }

    /// [`Probe::read`], searching the whole surface again if the point seems to be off
    /// it: the search from the last point may have gone astray.
    fn read_surely(&mut self, p: DVec3) -> Reading {
        let reading = self.read(p);
        if reading.gap <= ON_FACE || self.uv.is_none() {
            return reading;
        }
        self.uv = None;
        self.read(p)
    }
}

/// The point nearest `near` where the surfaces meet (Gauss–Newton on their signed
/// distances; with fewer than three surfaces, or dependent ones, the point stays as
/// close to `near` as the surfaces allow). `slice` adds a plane, by a point and its
/// normal, as one more surface.
pub(super) fn meet(
    probes: &mut [Probe<'_>],
    slice: Option<(DVec3, DVec3)>,
    near: DVec3,
) -> Result<DVec3, KernelError> {
    let mut p = near;
    let size = near.length().max(1.0);
    for _ in 0..64 {
        let mut jtj = DMat3::ZERO;
        let mut jtf = DVec3::ZERO;
        let mut worst = 0.0_f64;
        let mut add = |f: f64, n: DVec3| {
            worst = worst.max(f.abs());
            jtj += DMat3::from_cols(n * n.x, n * n.y, n * n.z);
            jtf += n * f;
        };
        for probe in probes.iter_mut() {
            let r = probe.read(p);
            add(r.distance, r.normal);
        }
        if let Some((origin, normal)) = slice {
            add((p - origin).dot(normal), normal);
        }
        if worst <= 1e-13 * size {
            break;
        }
        // A little damping keeps the step defined where the normals don't span space
        // (a vertex on a single face, or on two): the point then moves only as needed.
        let trace = jtj.x_axis.x + jtj.y_axis.y + jtj.z_axis.z;
        let damped = jtj + DMat3::from_diagonal(DVec3::splat(1e-12 * trace.max(1e-300)));
        if damped.determinant().abs() <= f64::MIN_POSITIVE {
            break;
        }
        let step = damped.inverse() * jtf;
        if !step.is_finite() {
            return Err(KernelError::InvalidResult(
                "a corner could not be found again".to_owned(),
            ));
        }
        p -= step;
        if step.length() <= 1e-15 * size {
            break;
        }
    }
    Ok(p)
}

fn near(p: DVec3) -> String {
    format!("({:.3}, {:.3}, {:.3})", p.x, p.y, p.z)
}

/// What to say when a point is on a freeform surface's tangent plane but past its edge.
pub(super) fn past_the_edge(p: DVec3) -> KernelError {
    KernelError::Unsupported(format!(
        "a freeform face would have to carry on past its own edge to meet its neighbour \
         near {}; use a smaller change, or leave that face as it is",
        near(p)
    ))
}

/// The message for a freeform surface that can't be offset by `distance`.
pub(super) fn offset_error(e: OffsetError, wall: bool) -> KernelError {
    match e {
        OffsetError::Folds { radius } if wall => KernelError::InvalidResult(format!(
            "The wall is thicker than the tightest curve of a freeform face (radius ≈ \
             {radius:.3} mm), so the inside of that face would fold over itself. Use a \
             thinner wall."
        )),
        OffsetError::Folds { radius } => KernelError::InvalidResult(format!(
            "The distance is larger than the tightest curve of a freeform face (radius ≈ \
             {radius:.3} mm), so the moved face would fold over itself. Use a smaller \
             distance."
        )),
        OffsetError::Pinched => KernelError::Unsupported(
            "a freeform face that comes to a point can't be offset: it has no direction \
             to move in there"
                .to_owned(),
        ),
        OffsetError::Rough => KernelError::Unsupported(
            "a freeform face can't be offset accurately: it has a crease, or the \
             distance is too close to its tightest curve. Use a smaller distance"
                .to_owned(),
        ),
    }
}

/// An edge to work out again numerically.
pub(super) struct EdgeJob<'a> {
    /// The edge as it was.
    pub old: &'a Edge,
    /// Its two faces' new surfaces.
    pub surfaces: [&'a Surface; 2],
    /// Where its ends were, and where they are now.
    pub old_ends: [DVec3; 2],
    pub ends: [DVec3; 2],
}

/// A point of the new edge.
#[derive(Clone, Copy)]
struct Sample {
    /// The fraction of the old edge it belongs to.
    s: f64,
    point: DVec3,
    uv: [Option<DVec2>; 2],
    /// How far it is from the further of the two surfaces.
    miss: f64,
    /// The sine of the angle between the two surfaces there: near zero where they are
    /// nearly tangent, and their intersection is poorly defined.
    crossing: f64,
}

impl EdgeJob<'_> {
    fn middle(&self) -> DVec3 {
        self.old.point_at_fraction(0.5)
    }

    fn old_tangent(&self, s: f64) -> DVec3 {
        self.old
            .curve
            .tangent(self.old.t0 + (self.old.t1 - self.old.t0) * s)
    }

    /// What two probes say about `point`, as a sample; an error if the point is not on
    /// both surfaces.
    fn checked(&self, s: f64, point: DVec3, uv: [Option<DVec2>; 2]) -> Result<Sample, KernelError> {
        let mut out = Sample {
            s,
            point,
            uv,
            miss: 0.0,
            crossing: 0.0,
        };
        let mut normals = [DVec3::ZERO; 2];
        for k in 0..2 {
            let mut probe = Probe::from(self.surfaces[k], uv[k]);
            let r = probe.read_surely(point);
            normals[k] = r.normal;
            if r.distance.abs() > ON_FACE {
                return Err(KernelError::InvalidResult(format!(
                    "two faces no longer meet along their edge near {}",
                    near(self.middle())
                )));
            }
            if r.gap > ON_FACE {
                return Err(past_the_edge(point));
            }
            out.uv[k] = probe.uv;
            out.miss = out.miss.max(r.gap);
        }
        out.crossing = normals[0].cross(normals[1]).length();
        Ok(out)
    }

    /// The point of the new edge that belongs to the fraction `s` of the old one: where
    /// the plane square to the old edge there, moved with the edge's ends, meets both
    /// surfaces. `uv` is where to start looking on freeform surfaces.
    fn at(&self, s: f64, uv: [Option<DVec2>; 2]) -> Result<Sample, KernelError> {
        let moved =
            (self.ends[0] - self.old_ends[0]) * (1.0 - s) + (self.ends[1] - self.old_ends[1]) * s;
        let guess = self.old.point_at_fraction(s) + moved;
        let mut probes = [
            Probe::from(self.surfaces[0], uv[0]),
            Probe::from(self.surfaces[1], uv[1]),
        ];
        let point = meet(&mut probes, Some((guess, self.old_tangent(s))), guess)?;
        self.checked(s, point, [probes[0].uv, probes[1].uv])
    }

    /// How far `p` is from the further of the two surfaces.
    fn off(&self, p: DVec3, uv: [Option<DVec2>; 2]) -> f64 {
        (0..2)
            .map(|k| Probe::from(self.surfaces[k], uv[k]).read_surely(p).gap)
            .fold(0.0, f64::max)
    }

    /// Whether `curve` between `t0` and `t1` lies on both surfaces.
    fn lies_on(&self, curve: &Curve3, t0: f64, t1: f64) -> bool {
        const CHECKS: usize = 12;
        (1..CHECKS).all(|i| {
            let p = curve.point(t0 + (t1 - t0) * i as f64 / CHECKS as f64);
            self.off(p, [None, None]) <= CURVE_FIT
        })
    }

    /// The edge as a straight line again, if it was one and its faces still meet in one.
    fn line(&self) -> Option<(Curve3, f64, f64)> {
        if !matches!(self.old.curve, Curve3::Line(_)) {
            return None;
        }
        let [start, end] = self.ends;
        let length = start.distance(end);
        if length <= LINEAR || (end - start).dot(self.old_tangent(0.5)) <= 0.0 {
            return None;
        }
        let line = Curve3::line_through(start, end)?;
        self.lies_on(&line, 0.0, length)
            .then_some((line, 0.0, length))
    }

    /// The edge as an arc of a circle again, if it was one and its faces still meet in
    /// one.
    fn circle(&self) -> Option<(Curve3, f64, f64)> {
        if !matches!(self.old.curve, Curve3::Circle(_)) {
            return None;
        }
        let closed = self.old.is_closed();
        // Three points of it in order: the ends and the middle, or thirds of a full turn.
        let (a, b, c) = if closed {
            (
                self.ends[0],
                self.at(1.0 / 3.0, [None, None]).ok()?.point,
                self.at(2.0 / 3.0, [None, None]).ok()?.point,
            )
        } else {
            (
                self.ends[0],
                self.at(0.5, [None, None]).ok()?.point,
                self.ends[1],
            )
        };
        let (u, v) = (b - a, c - a);
        let n = u.cross(v);
        let n2 = n.length_squared();
        if n2 <= 1e-16 * u.length_squared() * v.length_squared() {
            return None;
        }
        let center =
            a + (v.cross(n) * u.length_squared() + n.cross(u) * v.length_squared()) / (2.0 * n2);
        let radius = center.distance(a);
        // Counter-clockwise about `n` is the way the points run.
        let frame = Frame::from_origin_z_x(center, n, a - center)?;
        let t1 = if closed {
            TAU
        } else {
            let l = frame.to_local(c);
            l.y.atan2(l.x).rem_euclid(TAU)
        };
        if t1 <= 1e-9 {
            return None;
        }
        let circle = Curve3::Circle(Circle3 { frame, radius });
        self.lies_on(&circle, 0.0, t1).then_some((circle, 0.0, t1))
    }

    /// The fractions of the edge at which it crosses a crease of one of its freeform
    /// surfaces (a knot line where the surface's pieces don't join smoothly, such as the
    /// line where an offset surface carries on past the original's edge), with 0 and 1:
    /// the new edge is smooth between these, and only there. Also the smallest sine of
    /// the angle between the two surfaces along the edge.
    fn creases(&self, first: Sample) -> Result<(Vec<f64>, f64), KernelError> {
        // The knot lines of each surface, as (surface, in v?, parameter).
        let mut lines: Vec<(usize, bool, f64)> = Vec::new();
        for (k, surface) in self.surfaces.iter().enumerate() {
            let Surface::Nurbs(s) = surface else { continue };
            let (degrees, (knots_u, knots_v)) = (s.degrees(), s.knots());
            for (in_v, degree, knots) in [(false, degrees.0, knots_u), (true, degrees.1, knots_v)] {
                let (lo, hi) = (knots[0], knots[knots.len() - 1]);
                let mut i = 0;
                while i < knots.len() {
                    let t = knots[i];
                    let times = knots[i..].iter().take_while(|x| **x == t).count();
                    // Smooth to the second derivative at least: not a crease.
                    if t > lo && t < hi && times + 2 > degree {
                        lines.push((k, in_v, t));
                    }
                    i += times;
                }
            }
        }
        let mut out = vec![0.0, 1.0];
        let side = |sample: &Sample, line: &(usize, bool, f64)| -> f64 {
            let uv = sample.uv[line.0].unwrap_or(DVec2::NAN);
            (if line.1 { uv.y } else { uv.x }) - line.2
        };
        // Closer than this in the parameter, the edge is on the line, not across it.
        const ON_LINE: f64 = 1e-9;
        let steps = if lines.is_empty() { 12 } else { 48 };
        let mut before = first;
        let mut crossing = first.crossing;
        for i in 1..=steps {
            let here = self.at(i as f64 / steps as f64, before.uv)?;
            crossing = crossing.min(here.crossing);
            for line in &lines {
                let (a, b) = (side(&before, line), side(&here, line));
                if !(a.abs() > ON_LINE && b.abs() > ON_LINE && (a < 0.0) != (b < 0.0)) {
                    continue;
                }
                let (mut lo, mut hi) = (before, here);
                for _ in 0..30 {
                    let middle = self.at(0.5 * (lo.s + hi.s), lo.uv)?;
                    if (side(&middle, line) < 0.0) == (a < 0.0) {
                        lo = middle;
                    } else {
                        hi = middle;
                    }
                }
                out.push(0.5 * (lo.s + hi.s));
            }
            before = here;
        }
        out.sort_by(f64::total_cmp);
        // Crossings on top of each other, or on an end, are one.
        const SAME: f64 = 1e-5;
        out.dedup_by(|b, a| *b - *a <= SAME);
        if let Some(l) = out.last_mut() {
            *l = 1.0;
        }
        if out.len() > 2 && out[out.len() - 1] - out[out.len() - 2] <= SAME {
            out.remove(out.len() - 2);
        }
        Ok((out, crossing))
    }

    /// The new edge: its curve, and the parameters of its ends.
    pub fn solve(&self) -> Result<(Curve3, f64, f64), KernelError> {
        if let Some(found) = self.line().or_else(|| self.circle()) {
            return Ok(found);
        }
        if self.old.is_closed() {
            return Err(KernelError::Unsupported(format!(
                "the closed edge near {} would become a freeform curve, which has to be in \
                 two pieces",
                near(self.middle())
            )));
        }
        let turned = || {
            KernelError::InvalidResult(format!(
                "the edge near {} would shrink to nothing or turn round",
                near(self.middle())
            ))
        };
        // The curve through the points that belong to each fraction of the old edge,
        // from the new start vertex to the new end vertex exactly. The points are asked
        // for in order, so each search of a freeform surface starts where the last one
        // ended.
        let first = self.checked(0.0, self.ends[0], [None, None])?;
        let end = self.checked(1.0, self.ends[1], [None, None])?;
        // A vertex where two of its faces are tangent is only as well placed as its
        // fitted surfaces agree, so the curve's own ends may miss the vertices by a
        // fraction of the tolerance. That is taken up evenly along the curve.
        let slack = [
            self.ends[0] - self.at(0.0, first.uv)?.point,
            self.ends[1] - self.at(1.0, end.uv)?.point,
        ];
        if slack.iter().any(|d| d.length() > 4.0 * ON_FACE) {
            return Err(KernelError::InvalidResult(format!(
                "two faces no longer meet along their edge near {}",
                near(self.middle())
            )));
        }
        // The samples of the round of fitting in hand, in order along the edge.
        let samples: RefCell<Vec<Sample>> = RefCell::new(vec![first]);
        let failed: RefCell<Option<KernelError>> = RefCell::new(None);
        let shape = |s: f64| -> Option<DVec3> {
            if s <= 0.0 {
                *samples.borrow_mut() = vec![first];
                return Some(self.ends[0]);
            }
            if s >= 1.0 {
                return Some(self.ends[1]);
            }
            let hint = samples.borrow().last().map_or(first.uv, |l| l.uv);
            match self.at(s, hint) {
                Ok(sample) => {
                    samples.borrow_mut().push(sample);
                    Some(sample.point + slack[0] * (1.0 - s) + slack[1] * s)
                }
                Err(e) => {
                    failed.borrow_mut().get_or_insert(e);
                    None
                }
            }
        };
        // How far the fit is off the two surfaces, beyond what its ends and the points
        // it passes through are: where two fitted surfaces are nearly tangent they may
        // not quite meet, and the points lie between them.
        let miss = |s: f64, p: DVec3| -> Option<f64> {
            let samples = samples.borrow();
            let before = samples.partition_point(|x| x.s <= s).max(1) - 1;
            let after = (before + 1).min(samples.len() - 1);
            let allowed = slack[0].length() * (1.0 - s)
                + slack[1].length() * s
                + samples[before].miss.max(samples[after].miss);
            Some((self.off(p, samples[before].uv) - allowed).max(0.0))
        };
        const DEGREE: usize = 5;
        let (breaks, crossing) = self.creases(first)?;
        // Where the faces are nearly tangent their fitted surfaces don't meet in one
        // clean curve: any curve between them, as close to both as they are to each
        // other, is their edge.
        let tolerance = if crossing.min(end.crossing) < NEARLY_TANGENT {
            TANGENT_FIT
        } else {
            CURVE_FIT
        };
        let fitted = NurbsCurve::fitted_within(&shape, &miss, &breaks, DEGREE, tolerance);
        let Ok(curve) = fitted else {
            return Err(failed.into_inner().unwrap_or_else(|| {
                KernelError::Unsupported(format!(
                    "the edge near {} can't be followed accurately: its faces are too \
                     nearly tangent there",
                    near(self.middle())
                ))
            }));
        };
        // It must run on the way the old edge did.
        let checks = 4 * (curve.knots().len() - 2 * DEGREE - 1);
        let forward = (0..=checks).all(|i| {
            let s = i as f64 / checks as f64;
            curve.evaluate(s)[1].dot(self.old_tangent(s)) > 0.0
        });
        if !forward || self.ends[0].distance(self.ends[1]) <= LINEAR {
            return Err(turned());
        }
        Ok((Curve3::Nurbs(Arc::new(curve)), 0.0, 1.0))
    }
}

/// The seam of a freeform face that closes on itself, on the face's new surface: the
/// same edge of the parameter rectangle as before.
pub(super) fn seam(old: &NurbsSurface, new: &NurbsSurface, edge: &Edge) -> Option<Curve3> {
    let (lo, hi) = old.domain();
    let uv = old.param(edge.point_at_fraction(0.5));
    let size = hi - lo;
    let at_u = (uv.x - lo.x).min(hi.x - uv.x) <= 1e-9 * size.x;
    let at_v = (uv.y - lo.y).min(hi.y - uv.y) <= 1e-9 * size.y;
    let curve = if at_u && old.is_closed(true, LINEAR) {
        new.iso_curve(false, lo.x)
    } else if at_v && old.is_closed(false, LINEAR) {
        new.iso_curve(true, lo.y)
    } else {
        return None;
    };
    Some(Curve3::Nurbs(Arc::new(curve)))
}

// ---- Draft ----

/// A curve given by its point and derivative at any parameter.
type Path<'a> = Box<dyn Fn(f64) -> (DVec3, DVec3) + 'a>;

/// The neutral curve of a freeform surface: its parameter range, the knots it may have
/// creases at, and the curve.
struct Neutral<'a> {
    range: (f64, f64),
    breaks: Vec<f64>,
    path: Path<'a>,
}

/// The parameter `w*` at which the surface's curve of constant `v` (`along_u`) or of
/// constant `u` lies in `plane`, if one does.
fn flat_iso(s: &NurbsSurface, plane: &Plane, along_u: bool, flat: f64) -> Option<f64> {
    let (lo, hi) = s.domain();
    let (w_lo, w_hi, t_lo, t_hi) = if along_u {
        (lo.y, hi.y, lo.x, hi.x)
    } else {
        (lo.x, hi.x, lo.y, hi.y)
    };
    let uv = |t: f64, w: f64| {
        if along_u {
            DVec2::new(t, w)
        } else {
            DVec2::new(w, t)
        }
    };
    let height = |t: f64, w: f64| plane.signed_distance(s.extended(uv(t, w))[0]);
    // A surface that is straight across (an extruded wall, a side of a loft between two
    // profiles) carries on exactly past its edges, so the plane may cut it there.
    let (degrees, counts) = (s.degrees(), s.counts());
    let straight = if along_u {
        degrees.1 == 1 && counts.1 == 2
    } else {
        degrees.0 == 1 && counts.0 == 2
    };
    let reach = if straight { 4.0 * (w_hi - w_lo) } else { 0.0 };
    const STEPS: usize = 16;
    let steps = if straight { 9 * STEPS } else { STEPS };
    let middle = 0.5 * (t_lo + t_hi);
    let at = |i: usize| w_lo - reach + (w_hi - w_lo + 2.0 * reach) * i as f64 / steps as f64;
    let values: Vec<f64> = (0..=steps).map(|i| height(middle, at(i))).collect();
    let mut roots: Vec<f64> = Vec::new();
    for i in 0..=steps {
        if values[i].abs() <= flat {
            if roots.last().is_none_or(|r| (at(i) - r).abs() > 1e-12) {
                roots.push(at(i));
            }
            continue;
        }
        if i == steps || values[i + 1].abs() <= flat || (values[i] < 0.0) == (values[i + 1] < 0.0) {
            continue;
        }
        let (mut a, mut b) = (at(i), at(i + 1));
        for _ in 0..80 {
            let m = 0.5 * (a + b);
            if (height(middle, m) < 0.0) == (values[i] < 0.0) {
                a = m;
            } else {
                b = m;
            }
        }
        roots.push(0.5 * (a + b));
    }
    let [w] = roots[..] else {
        return None;
    };
    const ALONG: usize = 32;
    (0..=ALONG)
        .all(|k| height(t_lo + (t_hi - t_lo) * k as f64 / ALONG as f64, w).abs() <= flat)
        .then_some(w)
}

/// Where the surface crosses the neutral plane.
fn neutral_curve<'a>(
    surface: &'a Surface,
    s: &'a NurbsSurface,
    plane: &Plane,
) -> Result<Neutral<'a>, KernelError> {
    let (lo, hi) = s.domain();
    let flat = 1e-9 * s.bounds().size().length().max(1.0);
    let (breaks_u, breaks_v) = s.breaks();
    if let Some(v) = flat_iso(s, plane, true, flat) {
        return Ok(Neutral {
            range: (lo.x, hi.x),
            breaks: breaks_u,
            path: Box::new(move |t| {
                let [p, du, _] = s.extended(DVec2::new(t, v));
                (p, du)
            }),
        });
    }
    if let Some(u) = flat_iso(s, plane, false, flat) {
        return Ok(Neutral {
            range: (lo.y, hi.y),
            breaks: breaks_v,
            path: Box::new(move |t| {
                let [p, _, dv] = s.extended(DVec2::new(u, t));
                (p, dv)
            }),
        });
    }
    // Neither: trace the crossing.
    let middle = s.point(0.5 * (lo + hi));
    let curves = match ssi::intersect(surface, &Surface::Plane(*plane), middle) {
        Ssi::Curves(curves) => curves,
        _ => Vec::new(),
    };
    let [Curve3::Nurbs(curve)] = &curves[..] else {
        return Err(KernelError::InvalidInput(
            "a freeform face to draft doesn't cross the neutral plane in one clean curve \
             along its whole width. Move the neutral plane to where it cuts right across \
             the face, or leave the face out"
                .to_owned(),
        ));
    };
    let curve = curve.clone();
    let range = curve.domain();
    let mut breaks: Vec<f64> = Vec::new();
    for &k in curve.knots() {
        if breaks.last().is_none_or(|l| k > *l) {
            breaks.push(k);
        }
    }
    Ok(Neutral {
        range,
        breaks,
        path: Box::new(move |t| {
            // Straight on past its ends.
            let inside = t.clamp(range.0, range.1);
            let [p, d, _] = curve.evaluate(inside);
            (p + d * (t - inside), d)
        }),
    })
}

/// The surface that replaces the freeform surface of `face` when it is drafted by
/// `angle` about `neutral`: ruled, through the curve where the face crosses the neutral
/// plane, narrowing the body along the plane's normal for a positive angle.
pub(super) fn draft_surface(
    solid: &Solid,
    face: FaceId,
    neutral: &Plane,
    angle: f64,
) -> Result<Surface, KernelError> {
    let f = solid.face(face);
    let Surface::Nurbs(s) = &f.surface else {
        unreachable!("only freeform faces get a ruled draft surface")
    };
    let pull = neutral.normal();
    let Neutral {
        range,
        mut breaks,
        path,
    } = neutral_curve(&f.surface, s, neutral)?;
    // Square to the curve in the neutral plane, and which way of that is out of the body.
    let across = |t: f64| -> Option<(DVec3, DVec3)> {
        let (p, d) = path(t);
        let tangent = (d - pull * d.dot(pull)).try_normalize()?;
        Some((p, tangent.cross(pull)))
    };
    const ALONG: usize = 16;
    let mut side = 0.0;
    let mut length = 0.0;
    let mut before: Option<DVec3> = None;
    for k in 0..=ALONG {
        let t = range.0 + (range.1 - range.0) * k as f64 / ALONG as f64;
        let facing = across(t).map(|(p, b)| {
            length += before.map_or(0.0, |q: DVec3| q.distance(p));
            before = Some(p);
            b.dot(s.normal(s.param(p)))
        });
        match facing {
            Some(x) if x.abs() > 1e-6 && (side == 0.0 || (x > 0.0) == (side > 0.0)) => {
                side = x.signum();
            }
            _ => {
                return Err(KernelError::InvalidInput(
                    "a freeform face to draft lies along the neutral plane somewhere on \
                     its neutral curve, so it has no side to tilt to there. Leave the face \
                     out, or pick another neutral plane"
                        .to_owned(),
                ));
            }
        }
    }
    // `b · side` is the surface's natural normal, more or less; outwards is that, or its
    // opposite for a reversed face.
    let outward = if f.reversed { -side } else { side };
    let tan = angle.tan();

    // How far the face reaches along the pull, from its edges.
    let (mut low, mut high) = (0.0_f64, 0.0_f64);
    for &l in &f.loops {
        for c in solid.loop_coedges(l) {
            let e = solid.edge(solid.coedge(c).edge);
            for k in 0..=8 {
                let h = neutral.signed_distance(e.point_at_fraction(f64::from(k) / 8.0));
                (low, high) = (low.min(h), high.max(h));
            }
        }
    }
    let pad = 0.25 * (high - low).max(1e-3);
    let heights = [low - pad, high + pad];
    // Along the curve, room for the neighbours to move: they tilt too.
    let room = 2.0 * (high - low) * tan.abs() + 0.05 * length;
    let speed = |t: f64| path(t).1.length().max(1e-12);
    let span = range.1 - range.0;
    breaks.insert(0, range.0 - (room / speed(range.0)).min(span));
    breaks.push(range.1 + (room / speed(range.1)).min(span));

    let point = |t: f64, h: f64| -> Option<DVec3> {
        let (p, b) = across(t)?;
        Some(p + (pull - b * (outward * tan)) * h)
    };
    // The surface's natural normal has to stay on the side it was: `∂/∂t × ∂/∂h` is
    // along `b`, so the curve's parameter comes first if `b` is the natural normal's
    // side, and second otherwise.
    let fitted = if side > 0.0 {
        NurbsSurface::fitted(
            &|uv: DVec2| point(uv.x, uv.y),
            &breaks,
            &heights,
            (5, 1),
            SURFACE_FIT,
        )
    } else {
        NurbsSurface::fitted(
            &|uv: DVec2| point(uv.y, uv.x),
            &heights,
            &breaks,
            (1, 5),
            SURFACE_FIT,
        )
    };
    let fitted = fitted.map_err(|_| {
        KernelError::Unsupported(
            "a freeform face to draft has a crease along its neutral curve, so its draft \
             surface would not be smooth"
                .to_owned(),
        )
    })?;
    Ok(Surface::Nurbs(Arc::new(fitted)))
}
