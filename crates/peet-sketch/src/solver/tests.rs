//! Solver tests: every constraint kind, sign preservation, drags, DOF counts, redundancy
//! and conflict diagnosis, caching, plus property tests on random consistent sketches.
//!
//! Constraint satisfaction is checked with an independent geometric evaluation
//! ([`violation`]), not with the solver's own equations.

use std::f64::consts::PI;

use peet_math::DVec2;
use proptest::prelude::*;

use super::*;
use crate::shapes;
use crate::sketch::{Constraint, ConstraintKind, ConstraintKind as K, EntityKind};

mod fixtures;
mod proptests;

pub(super) use fixtures::*;

/// Satisfaction tolerance for tests (the solver aims for ~1e-10).
pub(super) const TOL: f64 = 1e-8;

fn v(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

pub(super) fn line_ab(s: &Sketch, l: EntityId) -> (DVec2, DVec2) {
    let (a, b) = s.endpoints(l).unwrap();
    (s.point(a), s.point(b))
}

pub(super) fn circ(s: &Sketch, c: EntityId) -> (DVec2, f64) {
    let curve = s.curve(c).unwrap();
    (curve.center().unwrap(), curve.radius().unwrap())
}

/// How far the current geometry is from satisfying a constraint (mm, or radians).
pub(super) fn violation(s: &Sketch, c: &Constraint) -> f64 {
    let val = c.dimension.as_ref().map(|d| d.value);
    if !c.is_driving() {
        // Driven dimensions must match the measurement.
        let m = s.measure(&c.kind).unwrap();
        return (m - val.unwrap()).abs();
    }
    let dir = |l| {
        let (a, b): (DVec2, DVec2) = line_ab(s, l);
        (b - a).normalize()
    };
    let kind = |id| s.kind(id).unwrap();
    match c.kind {
        K::Coincident(a, b) => s.point(a).distance(s.point(b)),
        K::PointOnCurve { point, curve } => {
            s.curve(curve).unwrap().distance_to_line(s.point(point))
        }
        K::Horizontal(l) => {
            let (a, b) = line_ab(s, l);
            (b.y - a.y).abs()
        }
        K::Vertical(l) => {
            let (a, b) = line_ab(s, l);
            (b.x - a.x).abs()
        }
        K::HorizontalPoints(a, b) => (s.point(a).y - s.point(b).y).abs(),
        K::VerticalPoints(a, b) => (s.point(a).x - s.point(b).x).abs(),
        K::Parallel(a, b) => dir(a).perp_dot(dir(b)).abs(),
        K::Perpendicular(a, b) => dir(a).dot(dir(b)).abs(),
        K::Tangent(a, b) => {
            if kind(a) == EntityKind::Line || kind(b) == EntityKind::Line {
                let (l, cc) = if kind(a) == EntityKind::Line {
                    (a, b)
                } else {
                    (b, a)
                };
                let (c, r) = circ(s, cc);
                (s.curve(l).unwrap().distance_to_line(c) - r).abs()
            } else {
                let ((c1, r1), (c2, r2)) = (circ(s, a), circ(s, b));
                let d = c1.distance(c2);
                (d - (r1 + r2)).abs().min((d - (r1 - r2).abs()).abs())
            }
        }
        K::Equal(a, b) => {
            if kind(a) == EntityKind::Line {
                (s.curve(a).unwrap().length() - s.curve(b).unwrap().length()).abs()
            } else {
                (circ(s, a).1 - circ(s, b).1).abs()
            }
        }
        K::Concentric(a, b) => circ(s, a).0.distance(circ(s, b).0),
        K::Midpoint { point, line } => {
            let middle = s.curve(line).unwrap().point_at(0.5);
            s.point(point).distance(middle)
        }
        K::Symmetric { a, b, axis } => {
            let (pa, pb) = (s.point(a), s.point(b));
            let axis_curve = s.curve(axis).unwrap();
            axis_curve.distance_to_line((pa + pb) * 0.5) + dir(axis).dot(pb - pa).abs()
        }
        K::Fix { point, at } => s.point(point).distance(at),
        K::Patterned { seed, copy, first } => {
            // Where each point of the copy should be: the seed's, moved n steps
            let places = |id| match s.kind(id).unwrap() {
                EntityKind::Point => vec![s.point(id)],
                EntityKind::Circle => vec![circ(s, id).0],
                _ => {
                    let (a, b) = line_ab(s, id);
                    vec![a, b]
                }
            };
            let (sp, fp, cp) = (places(seed), places(first), places(copy));
            let step = fp[0] - sp[0];
            let n = ((cp[0] - sp[0]).dot(step) / step.length_squared()).round();
            let size = if kind(seed) == EntityKind::Circle {
                (circ(s, seed).1 - circ(s, copy).1).abs()
            } else {
                0.0
            };
            let off = |o: &[DVec2], n: f64| {
                (0..sp.len())
                    .map(|i| (o[i] - sp[i] - step * n).length())
                    .fold(0.0, f64::max)
            };
            off(&cp, n).max(off(&fp, 1.0)).max(size)
        }
        K::Offset(a, b) if kind(a) == EntityKind::Line => {
            let (start, _) = line_ab(s, a);
            dir(a).perp_dot(dir(b)).abs()
                + (s.curve(b).unwrap().distance_to_line(start) - val.unwrap()).abs()
        }
        K::Offset(a, b) => {
            let ((c1, r1), (c2, r2)) = (circ(s, a), circ(s, b));
            c1.distance(c2) + ((r1 - r2).abs() - val.unwrap()).abs()
        }
        K::Angle(l1, l2) => {
            // atan2 rather than `Sketch::measure` (acos based, only ~1e-8 accurate near
            // 0° and 180°).
            let (d1, d2) = (dir(l1), dir(l2));
            let m = d1.perp_dot(d2).atan2(d1.dot(d2)).abs();
            (m - val.unwrap().to_radians()).abs()
        }
        _ => {
            let m = s.measure(&c.kind).unwrap();
            (m - val.unwrap()).abs()
        }
    }
}

/// Largest violation over all constraints and the implicit arc equations.
pub(super) fn max_violation(s: &Sketch) -> f64 {
    let mut m = 0.0f64;
    for (_, c) in s.constraints() {
        m = m.max(violation(s, c));
    }
    for (_, e) in s.entities() {
        if let Geometry::Arc { center, start, end } = e.geometry {
            let c = s.point(center);
            m = m.max((s.point(start).distance(c) - s.point(end).distance(c)).abs());
        }
    }
    m
}

pub(super) fn assert_solved(s: &mut Sketch) -> SolveReport {
    let report = Solver::new().solve(s);
    assert!(report.converged, "not converged: {report:?}");
    let m = max_violation(s);
    assert!(m < TOL, "violation {m}: {report:?}");
    report
}

macro_rules! add {
    ($s:expr, $k:expr) => {{
        let k = $k;
        $s.add_constraint(k).unwrap()
    }};
}

macro_rules! dim {
    ($s:expr, $k:expr, $v:expr) => {{
        let k = $k;
        $s.add_dimension(k, $v).unwrap()
    }};
}

fn pts(s: &Sketch, l: EntityId) -> (EntityId, EntityId) {
    s.endpoints(l).unwrap()
}

// ---------------------------------------------------------------------------------------
// Every constraint kind from a perturbed start
// ---------------------------------------------------------------------------------------

#[test]
fn empty_sketch_solves() {
    let mut s = Sketch::new();
    let r = assert_solved(&mut s);
    assert_eq!(r.iterations, 0);
    let a = Solver::new().analyze(&s);
    assert_eq!(a.dof, 0);
    assert_eq!(a.entity_status(Sketch::ORIGIN), DofStatus::Fully);
}

#[test]
fn coincident() {
    let mut s = Sketch::new();
    let l1 = s.add_line(v(0.0, 0.0), v(10.0, 0.0));
    let l2 = s.add_line(v(10.5, 0.3), v(20.0, 5.0));
    add!(s, K::Coincident(pts(&s, l1).1, pts(&s, l2).0));
    assert_solved(&mut s);
    // The two free points meet halfway: minimum movement.
    assert!(s.point(pts(&s, l1).1).distance(v(10.25, 0.15)) < 1e-9);
}

#[test]
fn coincident_with_origin_moves_only_the_point() {
    let mut s = Sketch::new();
    let p = s.add_point(v(1.0, 2.0));
    add!(s, K::Coincident(p, Sketch::ORIGIN));
    assert_solved(&mut s);
    assert_eq!(s.point(p), DVec2::ZERO);
    assert_eq!(s.point(Sketch::ORIGIN), DVec2::ZERO);
}

#[test]
fn point_on_line_and_circle_and_arc() {
    let mut s = Sketch::new();
    let l = s.add_line(v(0.0, 0.0), v(10.0, 1.0));
    let c = s.add_circle(v(30.0, 0.0), 5.0);
    let a = s.add_arc(v(60.0, 0.0), v(65.0, 0.0), v(60.0, 5.0));
    let p1 = s.add_point(v(20.0, 7.0)); // beyond the segment: infinite line
    let p2 = s.add_point(v(31.0, 1.0));
    let p3 = s.add_point(v(52.0, -3.0)); // off the arc's span: full circle
    add!(
        s,
        K::PointOnCurve {
            point: p1,
            curve: l
        }
    );
    add!(
        s,
        K::PointOnCurve {
            point: p2,
            curve: c
        }
    );
    add!(
        s,
        K::PointOnCurve {
            point: p3,
            curve: a
        }
    );
    assert_solved(&mut s);
}

#[test]
fn horizontal_vertical() {
    let mut s = Sketch::new();
    let l1 = s.add_line(v(0.0, 0.0), v(10.0, 1.0));
    let l2 = s.add_line(v(0.0, 0.0), v(1.0, 10.0));
    let p = s.add_point(v(3.0, 4.0));
    let q = s.add_point(v(5.0, 6.5));
    add!(s, K::Horizontal(l1));
    add!(s, K::Vertical(l2));
    add!(s, K::HorizontalPoints(p, q));
    add!(s, K::VerticalPoints(pts(&s, l1).1, q));
    assert_solved(&mut s);
    // Minimum movement: the horizontal line's endpoints meet at their mean height.
    let (a, b) = line_ab(&s, l1);
    assert!((a.y - 0.5).abs() < 1e-9 && (b.y - 0.5).abs() < 1e-9 || (a.y - b.y).abs() < 1e-9);
}

#[test]
fn parallel_perpendicular() {
    let mut s = Sketch::new();
    let l1 = s.add_line(v(0.0, 0.0), v(10.0, 1.0));
    let l2 = s.add_line(v(0.0, 5.0), v(10.0, 7.0));
    let l3 = s.add_line(v(0.0, 5.0), v(-10.0, 5.5)); // anti-parallel direction
    let l4 = s.add_line(v(20.0, 0.0), v(21.0, 9.0));
    add!(s, K::Parallel(l1, l2));
    add!(s, K::Parallel(l1, l3));
    add!(s, K::Perpendicular(l1, l4));
    assert_solved(&mut s);
    // l3 stays anti-parallel (no flip).
    assert!(dir(&s, l1).dot(dir(&s, l3)) < 0.0);
}

fn dir(s: &Sketch, l: EntityId) -> DVec2 {
    let (a, b) = line_ab(s, l);
    (b - a).normalize()
}

#[test]
fn tangent_line_circle_keeps_side() {
    let mut s = Sketch::new();
    let l = s.add_line(v(-10.0, 6.0), v(10.0, 6.5));
    let c = s.add_circle(v(0.0, 0.0), 5.0);
    let l2 = s.add_line(v(-10.0, -4.0), v(10.0, -4.5));
    add!(s, K::Tangent(l, c));
    add!(s, K::Tangent(c, l2));
    assert_solved(&mut s);
    let (cc, _) = circ(&s, c);
    assert!(line_ab(&s, l).0.y > cc.y, "line above stays above");
    assert!(line_ab(&s, l2).0.y < cc.y, "line below stays below");
}

#[test]
fn tangent_circles_internal_and_external() {
    let mut s = Sketch::new();
    let a = s.add_circle(v(0.0, 0.0), 5.0);
    let b = s.add_circle(v(8.5, 0.0), 3.0); // nearly external (d = 8.5 vs 8)
    let c = s.add_circle(v(40.0, 0.0), 10.0);
    let d = s.add_circle(v(43.0, 1.0), 6.0); // nearly internal (d ≈ 3.2 vs 4)
    add!(s, K::Tangent(a, b));
    add!(s, K::Tangent(c, d));
    assert_solved(&mut s);
    let ((ca, ra), (cb, rb)) = (circ(&s, a), circ(&s, b));
    assert!((ca.distance(cb) - (ra + rb)).abs() < 1e-9);
    let ((cc, rc), (cd, rd)) = (circ(&s, c), circ(&s, d));
    assert!((cc.distance(cd) - (rc - rd).abs()).abs() < 1e-9);
}

#[test]
fn tangent_arc_line_with_shared_endpoint() {
    let mut s = Sketch::new();
    let l = s.add_line(v(-10.0, 5.0), v(0.0, 5.0));
    let a = s.add_arc(v(0.5, 0.2), v(5.5, 0.0), v(0.0, 5.0));
    let (_, le) = pts(&s, l);
    let (_, ae) = pts(&s, a);
    add!(s, K::Coincident(le, ae));
    add!(s, K::Tangent(l, a));
    assert_solved(&mut s);
}

#[test]
fn equal_and_concentric() {
    let mut s = Sketch::new();
    let l1 = s.add_line(v(0.0, 0.0), v(10.0, 0.0));
    let l2 = s.add_line(v(0.0, 5.0), v(7.0, 6.0));
    let c1 = s.add_circle(v(20.0, 0.0), 3.0);
    let a1 = s.add_arc(v(21.0, 1.0), v(26.0, 1.0), v(21.0, 6.0));
    add!(s, K::Equal(l1, l2));
    add!(s, K::Equal(c1, a1));
    add!(s, K::Concentric(c1, a1));
    assert_solved(&mut s);
}

#[test]
fn midpoint_symmetric_fix() {
    let mut s = Sketch::new();
    let l = s.add_line(v(0.0, 0.0), v(10.0, 2.0));
    let m = s.add_point(v(4.0, 3.0));
    let axis = s.add_line(v(20.0, -5.0), v(21.0, 5.0));
    let a = s.add_point(v(15.0, 1.0));
    let b = s.add_point(v(26.0, 2.5));
    let f = s.add_point(v(1.0, 1.0));
    add!(s, K::Midpoint { point: m, line: l });
    add!(s, K::Symmetric { a, b, axis });
    add!(
        s,
        K::Fix {
            point: f,
            at: v(3.0, -2.0)
        }
    );
    assert_solved(&mut s);
    assert!(s.point(f).distance(v(3.0, -2.0)) < 1e-10);
}

#[test]
fn midpoint_of_an_arc() {
    // A quarter arc counter-clockwise from (10, 0) to (0, 10) round the origin: its middle
    // is at 45°, not at the opposite point the same equations allow.
    let mut s = Sketch::new();
    let a = s.add_arc(v(0.0, 0.0), v(10.0, 0.0), v(0.0, 10.0));
    let m = s.add_point(v(6.0, 8.0));
    add!(s, K::Midpoint { point: m, line: a });
    assert_solved(&mut s);
    let middle = s.curve(a).unwrap().point_at(0.5);
    assert!(s.point(m).distance(middle) < 1e-8);
    // It follows the arc when the arc changes
    dim!(s, K::Radius(a), 20.0);
    assert_solved(&mut s);
    let (c, r) = circ(&s, a);
    assert!((r - 20.0).abs() < 1e-8);
    assert!((s.point(m).distance(c) - 20.0).abs() < 1e-8);
}

#[test]
fn offset_follows_what_it_is_offset_from() {
    // A line and a circle offset from a line and a circle, a little out of place to start with
    let mut s = Sketch::new();
    let source = s.add_line(v(0.0, 0.0), v(40.0, 0.0));
    let line = s.add_line(v(5.0, 9.8), v(35.0, 10.3));
    let hole = s.add_circle(v(60.0, 0.0), 4.0);
    let ring = s.add_circle(v(60.2, 0.1), 6.1);
    dim!(s, K::Offset(line, source), 10.0);
    let gap = dim!(s, K::Offset(ring, hole), 2.0);
    assert_solved(&mut s);
    // The source moves and grows: the offsets follow, on the same side, the same way out
    add!(s, K::Horizontal(source));
    dim!(s, K::Radius(hole), 5.0);
    s.constraint_mut(gap)
        .unwrap()
        .dimension
        .as_mut()
        .unwrap()
        .value = 3.0;
    assert_solved(&mut s);
    let ((a, b), (from, _)) = (line_ab(&s, line), line_ab(&s, source));
    assert!(
        (a.y - from.y - 10.0).abs() < 1e-8 && (b.y - a.y).abs() < 1e-8,
        "{a} {b}"
    );
    let ((c1, r1), (c2, r2)) = (circ(&s, ring), circ(&s, hole));
    assert!(
        c1.distance(c2) < 1e-8 && (r1 - r2 - 3.0).abs() < 1e-8,
        "{r1} {r2}"
    );
}

#[test]
fn linear_pattern_follows_its_seed() {
    // Three circles 10 apart and a line copied twice, a little out of place to start with
    let mut s = Sketch::new();
    let seed = s.add_circle(v(0.0, 0.0), 2.0);
    let first = s.add_circle(v(10.0, 0.1), 2.1);
    let third = s.add_circle(v(19.8, 0.0), 1.9);
    add!(s, K::Patterned { seed, copy: first, first });
    add!(s, K::Patterned { seed, copy: third, first });
    let l0 = s.add_line(v(0.0, 10.0), v(4.0, 12.0));
    let l1 = s.add_line(v(5.0, 10.0), v(9.0, 12.1));
    let l2 = s.add_line(v(10.0, 10.0), v(14.0, 12.0));
    add!(s, K::Patterned { seed: l0, copy: l1, first: l1 });
    add!(s, K::Patterned { seed: l0, copy: l2, first: l1 });
    assert_solved(&mut s);
    // The seed's size and the step drive the copies
    dim!(s, K::Radius(seed), 3.0);
    let (c0, c1) = (circ(&s, seed).0, circ(&s, first).0);
    dim!(s, K::Distance(s.center(seed).unwrap(), s.center(first).unwrap()), 12.0);
    assert_solved(&mut s);
    let (c0n, c1n, c2n) = (circ(&s, seed).0, circ(&s, first).0, circ(&s, third).0);
    assert!((circ(&s, third).1 - 3.0).abs() < 1e-8);
    assert!(c2n.distance(c0n + (c1n - c0n) * 2.0) < 1e-8);
    assert!((c1n.distance(c0n) - 12.0).abs() < 1e-8, "{c0} {c1}");
    let (a0, b0) = line_ab(&s, l0);
    let (a2, b2) = line_ab(&s, l2);
    assert!(((b2 - a2) - (b0 - a0)).length() < 1e-8);
}

#[test]
fn dimensions() {
    let mut s = Sketch::new();
    let l = s.add_line(v(0.0, 0.0), v(10.0, 1.0));
    let p = s.add_point(v(3.0, 4.0));
    let q = s.add_point(v(-5.0, -3.0));
    let c = s.add_circle(v(30.0, 0.0), 5.0);
    let a = s.add_arc(v(60.0, 0.0), v(65.0, 0.0), v(60.0, 5.0));
    let l2 = s.add_line(v(0.0, 20.0), v(10.0, 22.0));
    dim!(s, K::Length(l), 12.0);
    dim!(s, K::Distance(p, q), 7.0);
    dim!(s, K::Distance(p, l), 2.5);
    dim!(s, K::HorizontalDistance(p, q), 4.0);
    dim!(s, K::Radius(c), 7.5);
    dim!(s, K::Diameter(a), 12.0);
    dim!(s, K::Angle(l, l2), 30.0);
    assert_solved(&mut s);
    assert!((circ(&s, a).1 - 6.0).abs() < 1e-9);
}

/// A rectangle whose width and height are both measured from its top right corner, and
/// nothing else: a new width moves the left side, as SolidWorks does, and the corner the
/// dimensions share stays put. The same for a new height and the bottom.
#[test]
fn a_changed_dimension_moves_what_less_holds() {
    let mut s = Sketch::new();
    let shape = shapes::rectangle(&mut s, v(0.0, 0.0), v(30.0, 20.0));
    let [bottom, right, top, _] = [
        shape.curves[0],
        shape.curves[1],
        shape.curves[2],
        shape.curves[3],
    ];
    let (top_right, top_left) = pts(&s, top);
    let (bottom_right, _) = pts(&s, right);
    let bottom_left = pts(&s, bottom).0;
    let width = dim!(s, K::HorizontalDistance(top_left, top_right), 30.0);
    let height = dim!(s, K::VerticalDistance(top_right, bottom_right), 20.0);
    assert_solved(&mut s);
    s.constraint_mut(width)
        .unwrap()
        .dimension
        .as_mut()
        .unwrap()
        .value = 40.0;
    assert_solved(&mut s);
    assert!(
        s.point(top_right).distance(v(30.0, 20.0)) < 1e-9,
        "{:?}",
        s.point(top_right)
    );
    assert!(s.point(bottom_right).distance(v(30.0, 0.0)) < 1e-9);
    assert!(
        s.point(top_left).distance(v(-10.0, 20.0)) < 1e-9,
        "{:?}",
        s.point(top_left)
    );
    s.constraint_mut(height)
        .unwrap()
        .dimension
        .as_mut()
        .unwrap()
        .value = 50.0;
    assert_solved(&mut s);
    assert!(s.point(top_right).distance(v(30.0, 20.0)) < 1e-9);
    assert!(
        s.point(bottom_left).distance(v(-10.0, -30.0)) < 1e-9,
        "{:?}",
        s.point(bottom_left)
    );
}

/// A profile placed by a dimension to the origin moves as a whole when it changes, as
/// SolidWorks moves it: its shape is not stretched to meet the new value.
#[test]
fn a_changed_placement_moves_the_whole_profile() {
    let mut s = Sketch::new();
    let shape = shapes::rectangle(&mut s, v(100.0, 10.0), v(130.0, 30.0));
    let [bottom, right, ..] = [
        shape.curves[0],
        shape.curves[1],
        shape.curves[2],
        shape.curves[3],
    ];
    let corner = pts(&s, bottom).0;
    dim!(s, K::Length(bottom), 30.0);
    dim!(s, K::Length(right), 20.0);
    let place = dim!(s, K::HorizontalDistance(Sketch::ORIGIN, corner), 100.0);
    assert_solved(&mut s);
    s.constraint_mut(place)
        .unwrap()
        .dimension
        .as_mut()
        .unwrap()
        .value = 40.0;
    assert_solved(&mut s);
    let (a, b) = line_ab(&s, bottom);
    assert!(a.distance(v(40.0, 10.0)) < 1e-9, "{a:?}");
    assert!(b.distance(v(70.0, 10.0)) < 1e-9, "{b:?}");
}

#[test]
fn signs_are_kept() {
    let mut s = Sketch::new();
    let l = s.add_line(v(0.0, 0.0), v(10.0, 0.0));
    let below = s.add_point(v(5.0, -3.0));
    let p = s.add_point(v(0.0, 0.0));
    let q = s.add_point(v(-4.0, 10.0));
    let l2 = s.add_line(v(0.0, 0.0), v(10.0, -3.0)); // clockwise from l
    dim!(s, K::Distance(below, l), 5.0);
    dim!(s, K::HorizontalDistance(p, q), 6.0);
    dim!(s, K::VerticalDistance(p, q), 2.0);
    dim!(s, K::Angle(l, l2), 60.0);
    assert_solved(&mut s);
    let (a, b) = line_ab(&s, l);
    assert!(
        (b - a).perp_dot(s.point(below) - a) < 0.0,
        "point stays below"
    );
    assert!(s.point(q).x < s.point(p).x, "q stays left of p");
    assert!(s.point(q).y > s.point(p).y, "q stays above p");
    assert!(
        dir(&s, l).perp_dot(dir(&s, l2)) < 0.0,
        "angle stays clockwise"
    );
}

#[test]
fn large_angle_change_does_not_flip() {
    let mut s = Sketch::new();
    let l1 = s.add_line(v(0.0, 0.0), v(10.0, 0.0));
    let l2 = s.add_line(v(0.0, 0.0), v(10.0, 1.0)); // ~5.7°
    add!(s, K::Horizontal(l1));
    let d = dim!(s, K::Angle(l1, l2), 170.0);
    assert_solved(&mut s);
    assert!((s.measure(&s.constraint(d).unwrap().kind).unwrap() - 170.0).abs() < 1e-7);
}

#[test]
fn driven_dimensions_are_refreshed() {
    let mut s = Sketch::new();
    let l = s.add_line(v(0.0, 0.0), v(10.0, 0.0));
    let d = dim!(s, K::Length(l), 10.0);
    s.constraint_mut(d)
        .unwrap()
        .dimension
        .as_mut()
        .unwrap()
        .driving = false;
    let d2 = dim!(s, K::Length(l), 25.0);
    assert_solved(&mut s);
    let _ = d2;
    assert!((s.constraint(d).unwrap().dimension.as_ref().unwrap().value - 25.0).abs() < 1e-9);
}

#[test]
fn changing_a_dimension_value_resolves() {
    let mut s = Sketch::new();
    let l = s.add_line(v(0.0, 0.0), v(10.0, 0.0));
    add!(s, K::Coincident(pts(&s, l).0, Sketch::ORIGIN));
    add!(s, K::Horizontal(l));
    let d = dim!(s, K::Length(l), 10.0);
    let mut solver = Solver::new();
    assert!(solver.solve(&mut s).converged);
    for value in [20.0, 5.0, 123.456, 0.5] {
        s.constraint_mut(d)
            .unwrap()
            .dimension
            .as_mut()
            .unwrap()
            .value = value;
        assert!(solver.solve(&mut s).converged);
        assert!((line_ab(&s, l).1.x - value).abs() < 1e-9);
    }
}

#[test]
fn solving_twice_is_stable_and_skips_work() {
    let mut s = bracket(true);
    let mut solver = Solver::new();
    assert!(solver.solve(&mut s).converged);
    let before = s.clone();
    let r = solver.solve(&mut s);
    assert!(r.converged);
    assert_eq!(r.iterations, 0, "satisfied clusters are skipped");
    assert_eq!(s, before);
}

#[test]
fn topology_changes_between_calls() {
    let mut s = Sketch::new();
    let mut solver = Solver::new();
    let l = s.add_line(v(0.0, 0.0), v(10.0, 1.0));
    assert!(solver.solve(&mut s).converged);
    add!(s, K::Horizontal(l));
    assert!(solver.solve(&mut s).converged);
    let l2 = s.add_line(v(10.0, 0.5), v(12.0, 9.0));
    add!(s, K::Coincident(pts(&s, l).1, pts(&s, l2).0));
    add!(s, K::Perpendicular(l, l2));
    assert!(solver.solve(&mut s).converged);
    s.remove_entity(l).unwrap();
    assert!(solver.solve(&mut s).converged);
    assert_eq!(solver.analyze(&s).dof, 4);
    assert!(max_violation(&s) < TOL);
}

// ---------------------------------------------------------------------------------------
// Conflicts: the solve fails cleanly
// ---------------------------------------------------------------------------------------

#[test]
fn unreachable_triangle_keeps_geometry_and_reports() {
    let mut s = Sketch::new();
    let a = s.add_line(v(0.0, 0.0), v(3.0, 0.0));
    let b = s.add_line(v(3.0, 0.0), v(3.0, 4.0));
    let c = s.add_line(v(3.0, 4.0), v(0.0, 0.0));
    add!(s, K::Coincident(pts(&s, a).1, pts(&s, b).0));
    add!(s, K::Coincident(pts(&s, b).1, pts(&s, c).0));
    add!(s, K::Coincident(pts(&s, c).1, pts(&s, a).0));
    let da = dim!(s, K::Length(a), 3.0);
    let db = dim!(s, K::Length(b), 4.0);
    let dc = dim!(s, K::Length(c), 5.0);
    let mut solver = Solver::new();
    assert!(solver.solve(&mut s).converged);
    s.constraint_mut(dc)
        .unwrap()
        .dimension
        .as_mut()
        .unwrap()
        .value = 10.0;
    let before = s.clone();
    let r = solver.solve(&mut s);
    assert!(!r.converged);
    assert!(r.unsatisfied.contains(&dc));
    assert_eq!(s, before, "a failed solve leaves the geometry alone");
    let an = solver.analyze(&s);
    let d = an
        .diagnoses
        .iter()
        .find(|d| d.constraint == dc)
        .expect("the changed dimension is diagnosed");
    assert_eq!(d.status, ConstraintStatus::Conflicting);
    assert!(
        d.involved.contains(&da) && d.involved.contains(&db),
        "{d:?}"
    );
}

// ---------------------------------------------------------------------------------------
// Drags
// ---------------------------------------------------------------------------------------

#[test]
fn free_point_follows_cursor_exactly() {
    let mut s = Sketch::new();
    let p = s.add_point(v(1.0, 1.0));
    let other = s.add_line(v(5.0, 5.0), v(6.0, 7.0));
    let before = line_ab(&s, other);
    let mut solver = Solver::new();
    for t in [v(2.0, 3.0), v(-7.5, 0.25), v(100.0, -40.0)] {
        let r = solver.solve_drag(
            &mut s,
            &[Drag::Point {
                point: p,
                target: t,
            }],
        );
        assert!(r.converged);
        assert!(s.point(p).distance(t) < 1e-12);
    }
    assert_eq!(
        line_ab(&s, other),
        before,
        "unconnected geometry doesn't move"
    );
}

#[test]
fn drag_endpoint_of_constrained_line() {
    let mut s = Sketch::new();
    let l = s.add_line(v(0.0, 0.0), v(10.0, 0.0));
    let (a, b) = pts(&s, l);
    add!(s, K::Coincident(a, Sketch::ORIGIN));
    add!(s, K::Horizontal(l));
    let unrelated = s.add_circle(v(50.0, 50.0), 3.0);
    let mut solver = Solver::new();
    let r = solver.solve_drag(
        &mut s,
        &[Drag::Point {
            point: b,
            target: v(17.0, 4.0),
        }],
    );
    assert!(r.converged);
    assert!(max_violation(&s) < TOL);
    // x follows the cursor exactly, y is held by the horizontal constraint.
    assert!(s.point(b).distance(v(17.0, 0.0)) < 1e-9, "{:?}", s.point(b));
    assert_eq!(circ(&s, unrelated), (v(50.0, 50.0), 3.0));
}

#[test]
fn drag_rigid_line_endpoint_moves_the_other_end() {
    let mut s = Sketch::new();
    let l = s.add_line(v(0.0, 0.0), v(10.0, 0.0));
    let (_, b) = pts(&s, l);
    dim!(s, K::Length(l), 10.0);
    let mut solver = Solver::new();
    let mut target = v(10.0, 0.0);
    for _ in 0..20 {
        target += v(0.3, 0.4);
        let r = solver.solve_drag(&mut s, &[Drag::Point { point: b, target }]);
        assert!(r.converged);
        assert!(
            s.point(b).distance(target) < 1e-9,
            "reachable target is reached"
        );
        assert!(max_violation(&s) < TOL);
    }
}

#[test]
fn drag_point_on_fixed_circle_goes_to_closest_point() {
    let mut s = Sketch::new();
    let c = s.add_circle(v(0.0, 0.0), 10.0);
    s.fix(c).unwrap();
    let p = s.add_point(v(10.0, 0.0));
    add!(s, K::PointOnCurve { point: p, curve: c });
    let mut solver = Solver::new();
    let r = solver.solve_drag(
        &mut s,
        &[Drag::Point {
            point: p,
            target: v(0.0, 25.0),
        }],
    );
    assert!(r.converged);
    assert!(s.point(p).distance(v(0.0, 10.0)) < 1e-6, "{:?}", s.point(p));
    let (cc, rc) = circ(&s, c);
    assert!(
        cc.length() < 1e-9 && (rc - 10.0).abs() < 1e-9,
        "fixed circle doesn't move"
    );
}

#[test]
fn drag_fully_constrained_geometry_does_not_move() {
    let mut s = bracket(true);
    let mut solver = Solver::new();
    assert!(solver.solve(&mut s).converged);
    let before = s.clone();
    let p = s
        .entities()
        .find(|(id, e)| e.kind() == EntityKind::Point && *id != Sketch::ORIGIN)
        .unwrap()
        .0;
    let r = solver.solve_drag(
        &mut s,
        &[Drag::Point {
            point: p,
            target: v(500.0, 500.0),
        }],
    );
    assert!(r.converged);
    for (id, e) in before.entities() {
        if let Geometry::Point { pos } = e.geometry {
            assert!(s.point(id).distance(pos) < 1e-7, "point {id:?} moved");
        }
    }
}

#[test]
fn drag_line_translates_it() {
    let mut s = Sketch::new();
    let l = s.add_line(v(0.0, 0.0), v(10.0, 0.0));
    let l2 = s.add_line(v(10.0, 0.0), v(10.0, 10.0));
    add!(s, K::Coincident(pts(&s, l).1, pts(&s, l2).0));
    let mut solver = Solver::new();
    let grab = v(5.0, 0.0);
    for k in 1..=5 {
        let target = grab + v(k as f64, 2.0 * k as f64);
        let r = solver.solve_drag(
            &mut s,
            &[Drag::Curve {
                curve: l,
                grab,
                target,
            }],
        );
        assert!(r.converged);
        let (a, b) = line_ab(&s, l);
        let d = target - grab;
        assert!(a.distance(d) < 1e-9 && b.distance(v(10.0, 0.0) + d) < 1e-9);
    }
    // l2's far end stayed put; its start followed the shared corner.
    assert_eq!(line_ab(&s, l2).1, v(10.0, 10.0));
}

#[test]
fn drag_arc_translates_it() {
    let mut s = Sketch::new();
    let a = s.add_arc(v(0.0, 0.0), v(5.0, 0.0), v(0.0, 5.0));
    let mut solver = Solver::new();
    let grab = v(3.5, 3.5);
    let target = v(13.5, -6.5);
    let r = solver.solve_drag(
        &mut s,
        &[Drag::Curve {
            curve: a,
            grab,
            target,
        }],
    );
    assert!(r.converged);
    assert!(circ(&s, a).0.distance(v(10.0, -10.0)) < 1e-9);
    assert!((circ(&s, a).1 - 5.0).abs() < 1e-9);
}

#[test]
fn drag_circle_sets_radius() {
    let mut s = Sketch::new();
    let c = s.add_circle(v(1.0, 2.0), 3.0);
    let mut solver = Solver::new();
    let r = solver.solve_drag(
        &mut s,
        &[Drag::Curve {
            curve: c,
            grab: v(4.0, 2.0),
            target: v(1.0, 9.0),
        }],
    );
    assert!(r.converged);
    assert_eq!(circ(&s, c).0, v(1.0, 2.0), "centre stays put");
    assert!((circ(&s, c).1 - 7.0).abs() < 1e-9);
}

#[test]
fn drag_with_unsatisfied_other_cluster_solves_it_too() {
    let mut s = Sketch::new();
    let p = s.add_point(v(0.0, 0.0));
    let l = s.add_line(v(10.0, 0.0), v(20.0, 3.0));
    add!(s, K::Horizontal(l));
    let r = Solver::new().solve_drag(
        &mut s,
        &[Drag::Point {
            point: p,
            target: v(1.0, 1.0),
        }],
    );
    assert!(r.converged);
    assert!(max_violation(&s) < TOL);
}

// ---------------------------------------------------------------------------------------
// Degrees of freedom
// ---------------------------------------------------------------------------------------

fn dof_of(s: &Sketch) -> Analysis {
    Solver::new().analyze(s)
}

#[test]
fn dof_counts_of_basic_entities() {
    let mut s = Sketch::new();
    s.add_point(v(1.0, 2.0));
    assert_eq!(dof_of(&s).dof, 2);
    let mut s = Sketch::new();
    s.add_line(v(0.0, 0.0), v(1.0, 2.0));
    assert_eq!(dof_of(&s).dof, 4);
    let mut s = Sketch::new();
    s.add_circle(v(0.0, 0.0), 2.0);
    assert_eq!(dof_of(&s).dof, 3);
    let mut s = Sketch::new();
    s.add_arc(v(0.0, 0.0), v(2.0, 0.0), v(0.0, 2.0));
    assert_eq!(dof_of(&s).dof, 5);
    let mut s = Sketch::new();
    let l = s.add_line(v(0.0, 0.0), v(1.0, 0.0));
    add!(s, K::Horizontal(l));
    let a = dof_of(&s);
    assert_eq!(a.dof, 3);
    assert_eq!(a.entity_status(l), DofStatus::Under);
    assert!(a.diagnoses.is_empty());
}

#[test]
fn fully_dimensioned_rectangle() {
    let (s, lines) = rectangle_fully_defined();
    let a = dof_of(&s);
    assert_eq!(a.dof, 0);
    assert!(a.is_fully_defined(), "{:?}", a.diagnoses);
    for l in lines {
        assert_eq!(a.entity_status(l), DofStatus::Fully);
        let (p, q) = pts(&s, l);
        assert_eq!(a.entity_status(p), DofStatus::Fully);
        assert_eq!(a.entity_status(q), DofStatus::Fully);
    }
    assert_eq!(a.entity_status(Sketch::ORIGIN), DofStatus::Fully);
}

#[test]
fn partially_fixed_entities() {
    // A line with its start on the origin: start is fully defined, the line is not.
    let mut s = Sketch::new();
    let l = s.add_line(v(0.0, 0.0), v(5.0, 1.0));
    let (a, b) = pts(&s, l);
    add!(s, K::Coincident(a, Sketch::ORIGIN));
    let an = dof_of(&s);
    assert_eq!(an.dof, 2);
    assert_eq!(an.entity_status(a), DofStatus::Fully);
    assert_eq!(an.entity_status(b), DofStatus::Under);
    assert_eq!(an.entity_status(l), DofStatus::Under);
    // Fixing the length and angle defines it fully.
    dim!(s, K::Length(l), 5.0);
    add!(s, K::Horizontal(l));
    assert_solved(&mut s);
    let an = dof_of(&s);
    assert_eq!(an.dof, 0);
    assert_eq!(an.entity_status(l), DofStatus::Fully);
}

#[test]
fn circle_with_radius_and_centre() {
    let mut s = Sketch::new();
    let c = s.add_circle(v(3.0, 4.0), 2.0);
    dim!(s, K::Radius(c), 2.0);
    assert_eq!(dof_of(&s).dof, 2);
    add!(s, K::Coincident(s.center(c).unwrap(), Sketch::ORIGIN));
    assert_solved(&mut s);
    let a = dof_of(&s);
    assert_eq!(a.dof, 0);
    assert_eq!(a.entity_status(c), DofStatus::Fully);
}

#[test]
fn fixed_arc_is_fully_defined_without_diagnoses() {
    let mut s = Sketch::new();
    let a = s.add_arc(v(1.0, 1.0), v(6.0, 1.0), v(1.0, 6.0));
    s.fix(a).unwrap();
    assert_solved(&mut s);
    let an = dof_of(&s);
    assert_eq!(an.dof, 0);
    assert!(an.diagnoses.is_empty(), "{:?}", an.diagnoses);
    assert_eq!(an.entity_status(a), DofStatus::Fully);
}

#[test]
fn fixed_line_and_circle() {
    let mut s = Sketch::new();
    let l = s.add_line(v(1.0, 1.0), v(6.0, 1.0));
    let c = s.add_circle(v(1.0, 1.0), 3.0);
    s.fix(l).unwrap();
    s.fix(c).unwrap();
    let an = dof_of(&s);
    assert_eq!(an.dof, 0);
    assert!(an.diagnoses.is_empty());
    assert_eq!(an.entity_status(c), DofStatus::Fully);
}

// ---------------------------------------------------------------------------------------
// Shape helpers and editing operations analyse cleanly
// ---------------------------------------------------------------------------------------

fn assert_clean(s: &mut Sketch, dof: usize) {
    assert_solved(s);
    let a = dof_of(s);
    assert!(
        a.diagnoses.is_empty(),
        "unexpected diagnoses: {:?}",
        a.diagnoses
    );
    assert_eq!(a.dof, dof);
}

#[test]
fn shapes_have_the_expected_freedoms() {
    let mut s = Sketch::new();
    crate::shapes::rectangle(&mut s, v(0.0, 0.0), v(20.0, 10.0));
    assert_clean(&mut s, 4);

    let mut s = Sketch::new();
    crate::shapes::center_rectangle(&mut s, v(5.0, 5.0), v(20.0, 10.0));
    assert_clean(&mut s, 4);

    let mut s = Sketch::new();
    crate::shapes::slot(&mut s, v(0.0, 0.0), v(30.0, 10.0), 4.0);
    assert_clean(&mut s, 5);

    for n in [3, 4, 5, 6, 8, 12] {
        let mut s = Sketch::new();
        crate::shapes::polygon(&mut s, v(1.0, 2.0), v(11.0, 2.0), n);
        assert_clean(&mut s, 4);
    }
}

#[test]
fn filleted_rectangle_corner_is_clean() {
    let mut s = Sketch::new();
    let shape = crate::shapes::rectangle(&mut s, v(0.0, 0.0), v(20.0, 10.0));
    let corner = pts(&s, shape.curves[0]).1;
    crate::ops::fillet(&mut s, corner, 3.0).unwrap();
    assert_solved(&mut s);
    let a = dof_of(&s);
    assert!(a.diagnoses.is_empty(), "{:?}", a.diagnoses);
    // The fillet adds a radius dimension; the rectangle keeps its 4 freedoms.
    assert_eq!(a.dof, 4);
    // Filleting every corner stays clean too.
    let mut s = Sketch::new();
    let shape = crate::shapes::rectangle(&mut s, v(0.0, 0.0), v(20.0, 10.0));
    let corners: Vec<EntityId> = shape.curves.iter().map(|&l| pts(&s, l).0).collect();
    for c in corners {
        crate::ops::fillet(&mut s, c, 2.0).unwrap();
    }
    assert_solved(&mut s);
    let a = dof_of(&s);
    assert!(a.diagnoses.is_empty(), "{:?}", a.diagnoses);
    assert_eq!(a.dof, 4);
}

#[test]
fn bracket_is_fully_defined() {
    let mut s = bracket(true);
    assert_solved(&mut s);
    let a = dof_of(&s);
    assert!(a.diagnoses.is_empty(), "{:?}", a.diagnoses);
    assert_eq!(a.dof, 0);
    for (id, e) in s.entities() {
        assert_eq!(
            a.entity_status(id),
            DofStatus::Fully,
            "{id:?} {:?}",
            e.kind()
        );
    }
}

// ---------------------------------------------------------------------------------------
// Redundancy and conflicts
// ---------------------------------------------------------------------------------------

#[test]
fn coincident_cycle_is_redundant() {
    let mut s = Sketch::new();
    let a = s.add_point(v(0.0, 0.0));
    let b = s.add_point(v(1.0, 0.0));
    let c = s.add_point(v(0.0, 1.0));
    let ab = add!(s, K::Coincident(a, b));
    let bc = add!(s, K::Coincident(b, c));
    let ac = add!(s, K::Coincident(a, c));
    assert_solved(&mut s);
    let an = dof_of(&s);
    assert_eq!(an.dof, 2);
    assert_eq!(an.diagnoses.len(), 1);
    let d = &an.diagnoses[0];
    assert_eq!(d.constraint, ac);
    assert_eq!(d.status, ConstraintStatus::Redundant);
    assert_eq!(d.involved, vec![ab, bc]);
    // Redundant, not conflicting: the sketch is not over-defined (as in SolidWorks).
    assert_eq!(an.entity_status(a), DofStatus::Under);
    assert!(!an.is_over_defined());
}

#[test]
fn identical_horizontals_are_redundant() {
    let mut s = Sketch::new();
    let l = s.add_line(v(0.0, 0.0), v(10.0, 0.0));
    let h1 = add!(s, K::Horizontal(l));
    let h2 = add!(s, K::Horizontal(l));
    assert_solved(&mut s);
    let an = dof_of(&s);
    assert_eq!(an.dof, 3);
    assert_eq!(
        an.diagnoses,
        vec![Diagnosis {
            constraint: h2,
            status: ConstraintStatus::Redundant,
            involved: vec![h1],
        }]
    );
    assert_eq!(an.constraint_status(h1), ConstraintStatus::Ok);
    assert_eq!(an.constraint_status(h2), ConstraintStatus::Redundant);
    assert_eq!(an.entity_status(l), DofStatus::Under);
    assert!(!an.is_over_defined());
}

#[test]
fn a_redundant_relation_leaves_a_fully_defined_sketch_fully_defined() {
    // A fixed square, then an equal relation its horizontals and verticals already
    // imply. SolidWorks accepts it and calls the sketch fully defined; so does PeetCAD.
    let mut s = Sketch::new();
    let shape = crate::shapes::rectangle(&mut s, v(0.0, 0.0), v(20.0, 10.0));
    let [bottom, right, top, left] = [
        shape.curves[0],
        shape.curves[1],
        shape.curves[2],
        shape.curves[3],
    ];
    let corner = pts(&s, bottom).0;
    add!(s, K::Coincident(corner, Sketch::ORIGIN));
    dim!(s, K::Length(bottom), 20.0);
    dim!(s, K::Length(left), 10.0);
    assert_solved(&mut s);
    assert!(dof_of(&s).is_fully_defined());
    let eq = add!(s, K::Equal(bottom, top));
    assert_solved(&mut s);
    let an = dof_of(&s);
    assert_eq!(an.dof, 0);
    assert!(an.is_fully_defined(), "{:?}", an.diagnoses);
    assert_eq!(an.constraint_status(eq), ConstraintStatus::Redundant);
    assert_eq!(an.conflicts(), 0);
    for e in [bottom, right, top, left] {
        assert_eq!(an.entity_status(e), DofStatus::Fully);
    }

    // A length on the top that disagrees with the bottom's conflicts.
    let mut s2 = s.clone();
    let wrong = dim!(s2, K::Length(top), 25.0);
    let mut solver = Solver::new();
    assert!(!solver.solve(&mut s2).converged);
    let an = solver.analyze(&s2);
    assert!(an.is_over_defined());
    assert_eq!(an.constraint_status(wrong), ConstraintStatus::Conflicting);
    assert_eq!(an.entity_status(top), DofStatus::Over);
}

#[test]
fn rectangle_with_extra_parallel_is_redundant() {
    let mut s = Sketch::new();
    let shape = crate::shapes::rectangle(&mut s, v(0.0, 0.0), v(20.0, 10.0));
    let (bottom, top) = (shape.curves[0], shape.curves[2]);
    let par = add!(s, K::Parallel(bottom, top));
    assert_solved(&mut s);
    let an = dof_of(&s);
    assert_eq!(an.dof, 4);
    assert_eq!(an.diagnoses.len(), 1, "{:?}", an.diagnoses);
    let d = &an.diagnoses[0];
    assert_eq!(d.constraint, par);
    assert_eq!(d.status, ConstraintStatus::Redundant);
    // Explained by the horizontal relations on the two lines.
    let horizontals: Vec<ConstraintId> = s
        .constraints()
        .filter(|(_, c)| matches!(c.kind, K::Horizontal(_)))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(d.involved, horizontals);
}

#[test]
fn horizontal_vertical_angle_conflict() {
    let mut s = Sketch::new();
    let l1 = s.add_line(v(0.0, 0.0), v(10.0, 0.5));
    let l2 = s.add_line(v(0.0, 0.0), v(0.5, 10.0));
    add!(s, K::Coincident(pts(&s, l1).0, pts(&s, l2).0));
    let h = add!(s, K::Horizontal(l1));
    let vv = add!(s, K::Vertical(l2));
    let mut solver = Solver::new();
    assert!(solver.solve(&mut s).converged);
    let ang = dim!(s, K::Angle(l1, l2), 45.0);
    let r = solver.solve(&mut s);
    assert!(!r.converged);
    assert_eq!(r.unsatisfied, vec![ang]);
    let an = solver.analyze(&s);
    assert_eq!(
        an.diagnoses,
        vec![Diagnosis {
            constraint: ang,
            status: ConstraintStatus::Conflicting,
            involved: vec![h, vv],
        }]
    );
    assert_eq!(an.entity_status(l1), DofStatus::Over);
    assert_eq!(an.entity_status(l2), DofStatus::Over);
}

#[test]
fn horizontal_and_vertical_on_one_line_with_angle() {
    // H + V on the same line collapse it to a point; an angle to another line then has
    // nothing to measure. The solve must fail cleanly and the analysis name the culprits.
    let mut s = Sketch::new();
    let l = s.add_line(v(0.0, 0.0), v(10.0, 0.0));
    let other = s.add_line(v(0.0, 5.0), v(10.0, 5.0));
    let h = add!(s, K::Horizontal(l));
    let len = dim!(s, K::Length(l), 10.0);
    let vv = add!(s, K::Vertical(l));
    let mut solver = Solver::new();
    let r = solver.solve(&mut s);
    assert!(!r.converged);
    let an = solver.analyze(&s);
    let d = an
        .diagnoses
        .iter()
        .find(|d| d.constraint == vv)
        .expect("V diagnosed");
    assert_eq!(d.status, ConstraintStatus::Conflicting);
    // At the current (horizontal) geometry V's row equals the length's row.
    assert_eq!(d.involved, vec![len], "{d:?}");
    let _ = (other, h);
}

#[test]
fn conflicting_fixes() {
    let mut s = Sketch::new();
    let p = s.add_point(v(0.0, 0.0));
    let f1 = add!(
        s,
        K::Fix {
            point: p,
            at: v(1.0, 1.0)
        }
    );
    let f2 = add!(
        s,
        K::Fix {
            point: p,
            at: v(2.0, 1.0)
        }
    );
    let mut solver = Solver::new();
    assert!(!solver.solve(&mut s).converged);
    let an = solver.analyze(&s);
    assert_eq!(an.diagnoses.len(), 1);
    assert_eq!(an.diagnoses[0].constraint, f2);
    assert_eq!(an.diagnoses[0].status, ConstraintStatus::Conflicting);
    assert_eq!(an.diagnoses[0].involved, vec![f1]);
}

#[test]
fn dimension_between_coincident_points_conflicts() {
    let mut s = Sketch::new();
    let a = s.add_point(v(0.0, 0.0));
    let b = s.add_point(v(0.0, 0.0));
    let c = add!(s, K::Coincident(a, b));
    let d = dim!(s, K::HorizontalDistance(a, b), 5.0);
    let mut solver = Solver::new();
    assert!(!solver.solve(&mut s).converged);
    let an = solver.analyze(&s);
    assert_eq!(
        an.diagnoses,
        vec![Diagnosis {
            constraint: d,
            status: ConstraintStatus::Conflicting,
            involved: vec![c],
        }]
    );
}

#[test]
fn fix_on_point_coincident_with_origin() {
    let mut s = Sketch::new();
    let p = s.add_point(v(0.0, 0.0));
    let c = add!(s, K::Coincident(p, Sketch::ORIGIN));
    let f = add!(
        s,
        K::Fix {
            point: p,
            at: DVec2::ZERO
        }
    );
    assert_solved(&mut s);
    let an = dof_of(&s);
    assert_eq!(
        an.diagnoses,
        vec![Diagnosis {
            constraint: f,
            status: ConstraintStatus::Redundant,
            involved: vec![c],
        }]
    );
}

#[test]
fn over_defined_triangle_by_angles() {
    // Three angle dimensions of a triangle summing to 180° are redundant (the third is
    // implied), and conflicting if they don't sum to 180°.
    let mut s = Sketch::new();
    let a = s.add_line(v(0.0, 0.0), v(10.0, 0.0));
    let b = s.add_line(v(10.0, 0.0), v(5.0, 8.0));
    let c = s.add_line(v(5.0, 8.0), v(0.0, 0.0));
    add!(s, K::Coincident(pts(&s, a).1, pts(&s, b).0));
    add!(s, K::Coincident(pts(&s, b).1, pts(&s, c).0));
    add!(s, K::Coincident(pts(&s, c).1, pts(&s, a).0));
    let a1 = dim!(s, K::Angle(a, b), 120.0);
    let a2 = dim!(s, K::Angle(b, c), 120.0);
    let a3 = dim!(s, K::Angle(c, a), 120.0);
    assert_solved(&mut s);
    let an = dof_of(&s);
    assert_eq!(an.dof, 5 - 1);
    assert_eq!(an.diagnoses.len(), 1, "{:?}", an.diagnoses);
    assert_eq!(an.diagnoses[0].constraint, a3);
    assert_eq!(an.diagnoses[0].status, ConstraintStatus::Redundant);
    assert_eq!(an.diagnoses[0].involved, vec![a1, a2]);
}

#[test]
fn analysis_is_independent_of_solver_state() {
    let s = bracket(true);
    let mut fresh = Solver::new();
    let mut reused = Solver::new();
    let _ = reused.analyze(&Sketch::new());
    let mut s2 = s.clone();
    reused.solve(&mut s2);
    let mut s3 = s.clone();
    fresh.solve(&mut s3);
    assert_eq!(fresh.analyze(&s3), reused.analyze(&s2));
}

#[test]
fn angle_wraps_correctly() {
    assert!((equations::wrap_angle(PI + 0.1) - (-PI + 0.1)).abs() < 1e-12);
}

#[test]
fn distance_to_a_side_of_a_circle() {
    use crate::{Along, ArcSide};
    // A point 130 along X from a hole's far side (SolidWorks' Max), and a ring 3 from a
    // hole inside it, near side to near side (SolidWorks' Min between nested circles)
    let mut s = Sketch::new();
    let p = s.add_point(v(0.0, 0.0));
    add!(s, K::Fix { point: p, at: v(0.0, 0.0) });
    let hole = s.add_circle(v(120.0, 5.0), 2.1);
    dim!(s, K::Radius(hole), 2.1);
    add!(s, K::HorizontalPoints(p, s.center(hole).unwrap()));
    dim!(
        s,
        K::SideDistance { a: p, b: hole, sides: [ArcSide::Center, ArcSide::Far], along: Along::Horizontal },
        130.0
    );
    let ring = s.add_circle(v(0.5, 40.0), 6.0);
    let inner = s.add_circle(v(0.0, 40.0), 3.0);
    dim!(s, K::Radius(ring), 6.0);
    dim!(s, K::Radius(inner), 3.0);
    add!(s, K::HorizontalPoints(s.center(ring).unwrap(), s.center(inner).unwrap()));
    add!(s, K::Fix { point: s.center(inner).unwrap(), at: v(0.0, 40.0) });
    dim!(
        s,
        K::SideDistance { a: ring, b: inner, sides: [ArcSide::Near, ArcSide::Near], along: Along::Straight },
        2.0
    );
    assert_solved(&mut s);
    let (c, _) = circ(&s, hole);
    assert!((c.x + 2.1 - 130.0).abs() < 1e-8, "{c}");
    let (cr, _) = circ(&s, ring);
    // The ring's near side 6 - d out, the hole's 3: 2 apart puts the centres 1 apart
    assert!((cr.x.abs() - 1.0).abs() < 1e-8, "{cr}");
    // Apart, near to near takes both radii off
    let a = s.add_circle(v(0.0, 100.0), 5.0);
    let b = s.add_circle(v(30.0, 100.0), 4.0);
    let k = K::SideDistance { a, b, sides: [ArcSide::Near, ArcSide::Near], along: Along::Straight };
    assert!((s.measure(&k).unwrap() - 21.0).abs() < 1e-12);
    let k = K::SideDistance { a, b, sides: [ArcSide::Far, ArcSide::Near], along: Along::Straight };
    assert!((s.measure(&k).unwrap() - 31.0).abs() < 1e-12);
}
