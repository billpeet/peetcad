//! Helpers that draw common shapes as ordinary, fully related sketch geometry.
//!
//! Each helper adds a set of relations that captures the shape's intent **without
//! redundancy**: every relation removes degrees of freedom that no other relation already
//! removes, so the solver's redundancy analysis stays quiet and the shape keeps exactly the
//! freedoms a user expects to dimension (for example a rectangle keeps position, width and
//! height). The geometry is created already satisfying every relation, so the first solve
//! has nothing to move.

use std::f64::consts::TAU;

use peet_math::DVec2;

use crate::sketch::{ConstraintId, ConstraintKind, EntityId, Sketch};

/// What a shape helper created.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    /// The new curves (including construction geometry), in drawing order.
    pub curves: Vec<EntityId>,
    /// Free-standing points the shape created (for example a centre rectangle's centre
    /// point). Points owned by the curves are not listed.
    pub points: Vec<EntityId>,
    /// The relations tying them together.
    pub constraints: Vec<ConstraintId>,
}

impl Shape {
    fn relate(&mut self, sketch: &mut Sketch, kind: ConstraintKind) {
        // The helpers only ever relate entities they just created, with fitting kinds.
        let id = sketch
            .add_constraint(kind)
            .expect("shape helpers create valid relations");
        self.constraints.push(id);
    }
}

/// End point of a line or arc that a helper just created.
fn end_point(sketch: &Sketch, curve: EntityId) -> EntityId {
    sketch.endpoints(curve).expect("a line or arc").1
}

fn start_point(sketch: &Sketch, curve: EntityId) -> EntityId {
    sketch.endpoints(curve).expect("a line or arc").0
}

/// Adds lines through `corners` as a closed loop (`corners[i]` to `corners[i + 1]`), joined
/// end-to-start by coincident relations.
fn closed_polyline(sketch: &mut Sketch, shape: &mut Shape, corners: &[DVec2]) -> Vec<EntityId> {
    let n = corners.len();
    let lines: Vec<EntityId> = (0..n)
        .map(|i| sketch.add_line(corners[i], corners[(i + 1) % n]))
        .collect();
    for i in 0..n {
        let a = end_point(sketch, lines[i]);
        let b = start_point(sketch, lines[(i + 1) % n]);
        shape.relate(sketch, ConstraintKind::Coincident(a, b));
    }
    shape.curves.extend(&lines);
    lines
}

/// Axis-aligned rectangle from two opposite corners: four lines joined by coincident
/// relations, two horizontal and two vertical.
///
/// The lines run counter-clockwise from the lower-left corner: bottom, right, top, left.
/// Relations: 4 coincident + 2 horizontal + 2 vertical. Four lines have 16 degrees of
/// freedom; the coincident relations remove 8 and the H/V relations 4, leaving the 4 a
/// rectangle should have (position, width, height). Zero width or height gives zero
/// length lines (the UI prevents it) but still well-formed geometry.
pub fn rectangle(sketch: &mut Sketch, a: DVec2, b: DVec2) -> Shape {
    let mut shape = Shape::default();
    add_rectangle(sketch, &mut shape, a, b);
    shape
}

/// Adds the rectangle and returns its lines (bottom, right, top, left).
fn add_rectangle(sketch: &mut Sketch, shape: &mut Shape, a: DVec2, b: DVec2) -> Vec<EntityId> {
    let (lo, hi) = (a.min(b), a.max(b));
    let corners = [lo, DVec2::new(hi.x, lo.y), hi, DVec2::new(lo.x, hi.y)];
    let lines = closed_polyline(sketch, shape, &corners);
    shape.relate(sketch, ConstraintKind::Horizontal(lines[0]));
    shape.relate(sketch, ConstraintKind::Vertical(lines[1]));
    shape.relate(sketch, ConstraintKind::Horizontal(lines[2]));
    shape.relate(sketch, ConstraintKind::Vertical(lines[3]));
    lines
}

/// Axis-aligned rectangle from its centre and a corner. Like [`rectangle`], plus
/// construction diagonals whose shared midpoint is a centre point.
///
/// Relations on top of the rectangle's: each diagonal's endpoints are coincident with two
/// opposite corners (8 equations for the diagonals' 8 coordinates), and the centre point
/// is the midpoint of the **first** diagonal only (2 equations for its 2 coordinates).
/// The diagonals of a parallelogram always bisect each other, so a second midpoint
/// relation on the other diagonal would be implied, i.e. redundant. The result keeps the
/// rectangle's 4 degrees of freedom (centre position, width, height), and the centre point
/// is the natural handle to constrain (for example coincident with the origin).
pub fn center_rectangle(sketch: &mut Sketch, center: DVec2, corner: DVec2) -> Shape {
    let mut shape = Shape::default();
    let opposite = center * 2.0 - corner;
    let lines = add_rectangle(sketch, &mut shape, opposite, corner);
    // Corner k is the start point of line k (bottom-left, bottom-right, top-right, top-left).
    let corner_points: Vec<EntityId> = lines.iter().map(|&l| start_point(sketch, l)).collect();
    let mut diagonals = Vec::new();
    for (i, j) in [(0, 2), (1, 3)] {
        let d = sketch.add_line(
            sketch.point(corner_points[i]),
            sketch.point(corner_points[j]),
        );
        sketch.set_construction(d, true);
        let (s, e) = sketch.endpoints(d).expect("a line");
        shape.relate(sketch, ConstraintKind::Coincident(s, corner_points[i]));
        shape.relate(sketch, ConstraintKind::Coincident(e, corner_points[j]));
        diagonals.push(d);
    }
    shape.curves.extend(&diagonals);
    let first = sketch.curve(diagonals[0]).expect("a line");
    let mid = sketch.add_point((first.start() + first.end()) * 0.5);
    shape.points.push(mid);
    shape.relate(
        sketch,
        ConstraintKind::Midpoint {
            point: mid,
            line: diagonals[0],
        },
    );
    shape
}

/// Straight slot between two arc centres with the given radius (half the slot width): two
/// parallel lines and two semicircular end arcs, all tangent, plus a construction
/// centreline between the arc centres.
///
/// Drawing order: the line on the right of `c1 -> c2`, the arc around `c2`, the line on the
/// left, the arc around `c1` (a counter-clockwise loop; the arcs are counter-clockwise as
/// the model requires), then the centreline from `c1` to `c2`.
///
/// Relations: 4 coincident joints, 2 coincident arc centre / centreline ends, 4 tangent
/// (each line to each arc) and 1 equal radius. Counting: the free curves have 22 degrees
/// of freedom (24 coordinates minus each arc's implicit end-radius equation); the 6
/// coincident relations remove 12, leaving the centreline (4) plus each arc's radius and
/// two end angles (3 + 3). Tangency at a joint fixes each line to a common tangent of the
/// two circles (4 equations) and equal radius removes one more: 5 remain (both centres
/// and the radius), exactly a slot's freedoms. "Lines parallel to the centreline" is then
/// implied (common external tangents of equal circles are parallel to the line of
/// centres), so parallel relations would be redundant and are not added.
pub fn slot(sketch: &mut Sketch, c1: DVec2, c2: DVec2, radius: f64) -> Shape {
    let mut shape = Shape::default();
    let r = radius.abs();
    let dir = (c2 - c1).normalize_or(DVec2::X);
    let left = dir.perp() * r;
    // Counter-clockwise loop: right side forward, around c2, left side back, around c1.
    let right_line = sketch.add_line(c1 - left, c2 - left);
    let arc2 = sketch.add_arc(c2, c2 - left, c2 + left);
    let left_line = sketch.add_line(c2 + left, c1 + left);
    let arc1 = sketch.add_arc(c1, c1 + left, c1 - left);
    let loop_curves = [right_line, arc2, left_line, arc1];
    for i in 0..4 {
        let a = end_point(sketch, loop_curves[i]);
        let b = start_point(sketch, loop_curves[(i + 1) % 4]);
        shape.relate(sketch, ConstraintKind::Coincident(a, b));
    }
    let centreline = sketch.add_line(c1, c2);
    sketch.set_construction(centreline, true);
    let (cs, ce) = sketch.endpoints(centreline).expect("a line");
    let arc1_center = sketch.center(arc1).expect("an arc");
    let arc2_center = sketch.center(arc2).expect("an arc");
    shape.relate(sketch, ConstraintKind::Coincident(cs, arc1_center));
    shape.relate(sketch, ConstraintKind::Coincident(ce, arc2_center));
    for line in [right_line, left_line] {
        for arc in [arc1, arc2] {
            shape.relate(sketch, ConstraintKind::Tangent(line, arc));
        }
    }
    shape.relate(sketch, ConstraintKind::Equal(arc1, arc2));
    shape.curves.extend(loop_curves);
    shape.curves.push(centreline);
    shape
}

/// Regular polygon with `sides` sides, inscribed in a construction circle around `center`,
/// with one vertex at `vertex`. Sides are equal and all vertices lie on the circle.
/// Fewer than 3 sides are treated as 3.
///
/// The sides run counter-clockwise from `vertex`; the construction circle comes last in
/// [`Shape::curves`]. Relations: `n` coincident joints, `n` point-on-circle (the start of
/// each side) and `n - 1` equal (every side equal to the first). Counting: `4n + 3`
/// coordinates, minus `2n` (joints), `n` (vertices on the circle) and `n - 1` (equal
/// chords of one circle force equal central angles) leaves 4: centre, radius and
/// rotation. An `n`-th equal relation would be implied by the others.
pub fn polygon(sketch: &mut Sketch, center: DVec2, vertex: DVec2, sides: usize) -> Shape {
    let n = sides.max(3);
    let mut shape = Shape::default();
    let radius = vertex.distance(center);
    let start_angle = (vertex - center).to_angle();
    let corners: Vec<DVec2> = (0..n)
        .map(|k| center + DVec2::from_angle(start_angle + TAU * k as f64 / n as f64) * radius)
        .collect();
    let lines = closed_polyline(sketch, &mut shape, &corners);
    let circle = sketch.add_circle(center, radius);
    sketch.set_construction(circle, true);
    shape.curves.push(circle);
    for &line in &lines {
        let point = start_point(sketch, line);
        shape.relate(
            sketch,
            ConstraintKind::PointOnCurve {
                point,
                curve: circle,
            },
        );
    }
    for &line in &lines[1..] {
        shape.relate(sketch, ConstraintKind::Equal(lines[0], line));
    }
    shape
}

/// Test helpers shared by the shape and editing-operation tests: evaluates how far the
/// current geometry is from satisfying each constraint.
#[cfg(test)]
pub(crate) mod testing {
    use peet_math::DVec2;

    use crate::curve::Curve;
    use crate::sketch::{ConstraintKind, EntityKind, Geometry, Sketch};

    /// Tolerance for "satisfied" in tests: geometry built with a few float operations.
    pub const TOL: f64 = 1e-9;

    fn line(s: &Sketch, id: crate::EntityId) -> (DVec2, DVec2) {
        match s.curve(id) {
            Some(Curve::Line { a, b }) => (a, b),
            other => panic!("expected a line, got {other:?}"),
        }
    }

    fn circle(s: &Sketch, id: crate::EntityId) -> (DVec2, f64) {
        let c = s.curve(id).expect("a curve");
        (c.center().expect("circular"), c.radius().expect("circular"))
    }

    /// How far `kind` is from being satisfied (0 when satisfied): mm for positional
    /// relations, the sine of the angle error for directional ones.
    pub fn residual(s: &Sketch, kind: &ConstraintKind, value: Option<f64>) -> f64 {
        use ConstraintKind::*;
        let is_line = |id| s.kind(id) == Some(EntityKind::Line);
        match *kind {
            Coincident(a, b) => s.point(a).distance(s.point(b)),
            PointOnCurve { point, curve } => {
                s.curve(curve).unwrap().distance_to_line(s.point(point))
            }
            Horizontal(l) => {
                let (a, b) = line(s, l);
                (b.y - a.y).abs()
            }
            Vertical(l) => {
                let (a, b) = line(s, l);
                (b.x - a.x).abs()
            }
            HorizontalPoints(a, b) => (s.point(a).y - s.point(b).y).abs(),
            VerticalPoints(a, b) => (s.point(a).x - s.point(b).x).abs(),
            Parallel(l1, l2) | Perpendicular(l1, l2) => {
                let d1 = s.curve(l1).unwrap().line_direction().unwrap();
                let d2 = s.curve(l2).unwrap().line_direction().unwrap();
                if matches!(kind, Parallel(..)) {
                    d1.perp_dot(d2).abs()
                } else {
                    d1.dot(d2).abs()
                }
            }
            Tangent(a, b) => {
                if is_line(a) || is_line(b) {
                    let (l, c) = if is_line(a) { (a, b) } else { (b, a) };
                    let (center, r) = circle(s, c);
                    (s.curve(l).unwrap().distance_to_line(center) - r).abs()
                } else {
                    let (c1, r1) = circle(s, a);
                    let (c2, r2) = circle(s, b);
                    let d = c1.distance(c2);
                    (d - (r1 + r2)).abs().min((d - (r1 - r2).abs()).abs())
                }
            }
            Equal(a, b) => {
                let len = |id| {
                    let c = s.curve(id).unwrap();
                    if is_line(id) {
                        c.length()
                    } else {
                        c.radius().unwrap()
                    }
                };
                (len(a) - len(b)).abs()
            }
            Concentric(a, b) => circle(s, a).0.distance(circle(s, b).0),
            Midpoint { point, line: l } => {
                let (a, b) = line(s, l);
                s.point(point).distance((a + b) * 0.5)
            }
            Symmetric { a, b, axis } => {
                let (p, q) = (s.point(a), s.point(b));
                let ax = s.curve(axis).unwrap();
                let d = ax.line_direction().unwrap();
                ax.distance_to_line((p + q) * 0.5).max((q - p).dot(d).abs())
            }
            Fix { point, at } => s.point(point).distance(at),
            _ => {
                let measured = s.measure(kind).expect("a measurable dimension");
                (measured - value.expect("a dimension value")).abs()
            }
        }
    }

    /// Asserts that every driving constraint, and every arc's implicit equal-radius
    /// equation, holds for the current geometry.
    pub fn assert_satisfied(s: &Sketch) {
        for (id, c) in s.constraints() {
            if !c.is_driving() {
                continue;
            }
            let r = residual(s, &c.kind, c.dimension.as_ref().map(|d| d.value));
            assert!(r <= TOL, "constraint {id:?} {:?} has residual {r}", c.kind);
        }
        for (id, e) in s.entities() {
            if let Geometry::Arc { center, start, end } = e.geometry {
                let c = s.point(center);
                let r = (s.point(end).distance(c) - s.point(start).distance(c)).abs();
                assert!(r <= TOL, "arc {id:?} end radius differs by {r}");
            }
        }
    }

    /// Number of live constraints of each label, for relation-count assertions.
    pub fn count(s: &Sketch, label: &str) -> usize {
        s.constraints()
            .filter(|(_, c)| c.kind.label() == label)
            .count()
    }

    pub fn close(a: DVec2, b: DVec2) -> bool {
        a.distance(b) <= TOL
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{assert_satisfied, close, count};
    use super::*;
    use crate::curve::Curve;

    fn corners(s: &Sketch, lines: &[EntityId]) -> Vec<DVec2> {
        lines.iter().map(|&l| s.curve(l).unwrap().start()).collect()
    }

    #[test]
    fn rectangle_is_ccw_and_related() {
        let mut s = Sketch::new();
        let shape = rectangle(&mut s, DVec2::new(5.0, 3.0), DVec2::new(-1.0, 1.0));
        assert_eq!(shape.curves.len(), 4);
        assert_eq!(shape.constraints.len(), 8);
        assert_eq!(count(&s, "Coincident"), 4);
        assert_eq!(count(&s, "Horizontal"), 2);
        assert_eq!(count(&s, "Vertical"), 2);
        let c = corners(&s, &shape.curves);
        assert_eq!(
            c,
            vec![
                DVec2::new(-1.0, 1.0),
                DVec2::new(5.0, 1.0),
                DVec2::new(5.0, 3.0),
                DVec2::new(-1.0, 3.0)
            ]
        );
        // Counter-clockwise: positive signed area.
        let area: f64 = (0..4).map(|i| c[i].perp_dot(c[(i + 1) % 4])).sum::<f64>() / 2.0;
        assert!((area - 12.0).abs() < 1e-12);
        assert_satisfied(&s);
    }

    #[test]
    fn degenerate_rectangle_does_not_panic() {
        let mut s = Sketch::new();
        let shape = rectangle(&mut s, DVec2::ONE, DVec2::new(1.0, 4.0));
        assert_eq!(shape.curves.len(), 4);
        assert_eq!(shape.constraints.len(), 8);
        let shape = center_rectangle(&mut s, DVec2::ONE, DVec2::ONE);
        assert_eq!(shape.curves.len(), 6);
    }

    #[test]
    fn center_rectangle_has_diagonals_and_centre() {
        let mut s = Sketch::new();
        let shape = center_rectangle(&mut s, DVec2::new(1.0, 2.0), DVec2::new(4.0, 4.0));
        assert_eq!(shape.curves.len(), 6);
        assert_eq!(shape.points.len(), 1);
        // 8 rectangle + 4 diagonal-end coincident + 1 midpoint.
        assert_eq!(shape.constraints.len(), 13);
        assert_eq!(count(&s, "Coincident"), 8);
        assert_eq!(count(&s, "Midpoint"), 1);
        let c = corners(&s, &shape.curves[..4]);
        assert!(close(c[0], DVec2::new(-2.0, 0.0)));
        assert!(close(c[2], DVec2::new(4.0, 4.0)));
        for &d in &shape.curves[4..] {
            assert!(s.entity(d).unwrap().construction);
        }
        assert!(close(s.point(shape.points[0]), DVec2::new(1.0, 2.0)));
        assert_satisfied(&s);
    }

    #[test]
    fn slot_geometry_and_relations() {
        let mut s = Sketch::new();
        let (c1, c2) = (DVec2::new(0.0, 0.0), DVec2::new(10.0, 0.0));
        let shape = slot(&mut s, c1, c2, 2.0);
        assert_eq!(shape.curves.len(), 5);
        assert_eq!(count(&s, "Coincident"), 6);
        assert_eq!(count(&s, "Tangent"), 4);
        assert_eq!(count(&s, "Equal"), 1);
        assert_eq!(count(&s, "Parallel"), 0);
        assert_eq!(shape.constraints.len(), 11);
        // Arcs bulge outwards: the c2 arc passes through (12, 0), the c1 arc through (-2, 0).
        let arc2 = s.curve(shape.curves[1]).unwrap();
        assert!(close(arc2.point_at(0.5), DVec2::new(12.0, 0.0)));
        let arc1 = s.curve(shape.curves[3]).unwrap();
        assert!(close(arc1.point_at(0.5), DVec2::new(-2.0, 0.0)));
        let Curve::Arc { sweep, .. } = arc1 else {
            panic!()
        };
        assert!((sweep - std::f64::consts::PI).abs() < 1e-12);
        assert!(close(
            s.curve(shape.curves[0]).unwrap().start(),
            DVec2::new(0.0, -2.0)
        ));
        assert!(s.entity(shape.curves[4]).unwrap().construction);
        assert_satisfied(&s);

        // Diagonal slot.
        let mut s = Sketch::new();
        slot(&mut s, DVec2::new(1.0, 1.0), DVec2::new(4.0, 5.0), 0.5);
        assert_satisfied(&s);
    }

    #[test]
    fn polygon_geometry_and_relations() {
        let mut s = Sketch::new();
        let shape = polygon(&mut s, DVec2::new(1.0, 1.0), DVec2::new(3.0, 1.0), 6);
        assert_eq!(shape.curves.len(), 7);
        assert_eq!(count(&s, "Coincident"), 6);
        assert_eq!(count(&s, "Point on curve"), 6);
        assert_eq!(count(&s, "Equal"), 5);
        assert!(close(
            s.curve(shape.curves[0]).unwrap().start(),
            DVec2::new(3.0, 1.0)
        ));
        for &l in &shape.curves[..6] {
            assert!((s.curve(l).unwrap().length() - 2.0).abs() < 1e-12);
        }
        let circle = shape.curves[6];
        assert!(s.entity(circle).unwrap().construction);
        assert_eq!(s.curve(circle).unwrap().radius(), Some(2.0));
        assert_satisfied(&s);

        let mut s = Sketch::new();
        let shape = polygon(&mut s, DVec2::ZERO, DVec2::new(0.0, 1.0), 1);
        assert_eq!(shape.curves.len(), 4, "clamped to a triangle");
        assert_satisfied(&s);
    }
}
