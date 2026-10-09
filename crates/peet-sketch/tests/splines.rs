//! Splines with the solver and the editing operations.

use peet_math::DVec2;
use peet_sketch::ops::{self, OpError};
use peet_sketch::region::find_regions;
use peet_sketch::{ConstraintKind, Drag, EntityId, EntityKind, Sketch, Solver};

fn v(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

fn wave(s: &mut Sketch) -> EntityId {
    s.add_spline(
        &[v(0.0, 0.0), v(10.0, 8.0), v(25.0, -3.0), v(40.0, 6.0)],
        false,
    )
    .unwrap()
}

fn unsupported(result: Result<impl std::fmt::Debug, OpError>) -> String {
    match result {
        Err(e @ OpError::Unsupported(_)) => e.to_string(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn fit_points_take_relations_and_dimensions() {
    let mut s = Sketch::new();
    let spline = wave(&mut s);
    let points = s.spline_points(spline).unwrap().0.to_vec();
    // Free: two unknowns per point, and the spline adds none of its own.
    let mut solver = Solver::new();
    assert_eq!(solver.analyze(&s).dof, 8);
    s.add_constraint(ConstraintKind::Coincident(points[0], Sketch::ORIGIN))
        .unwrap();
    s.add_constraint(ConstraintKind::HorizontalPoints(points[0], points[3]))
        .unwrap();
    s.add_dimension(ConstraintKind::Distance(points[0], points[3]), 50.0)
        .unwrap();
    s.add_dimension(ConstraintKind::VerticalDistance(points[0], points[1]), 12.0)
        .unwrap();
    let report = solver.solve(&mut s);
    assert!(report.converged);
    assert_eq!(solver.analyze(&s).dof, 3);
    assert!(s.point(points[3]).distance(v(50.0, 0.0)) < 1e-6);
    assert!((s.point(points[1]).y - 12.0).abs() < 1e-6);
    // The curve is where its points now are.
    let curve = s.curve(spline).unwrap();
    for p in &points {
        assert!(curve.distance(s.point(*p)) < 1e-9);
    }
    assert!(curve.end().distance(v(50.0, 0.0)) < 1e-6);
    // Fixing the spline holds every point.
    s.fix(spline).unwrap();
    assert_eq!(solver.analyze(&s).dof, 0);
}

#[test]
fn dragging_a_point_and_the_curve() {
    let mut s = Sketch::new();
    let spline = wave(&mut s);
    let points = s.spline_points(spline).unwrap().0.to_vec();
    let before: Vec<DVec2> = points.iter().map(|p| s.point(*p)).collect();
    let mut solver = Solver::new();
    // One point follows the cursor; the others stay.
    let report = solver.solve_drag(
        &mut s,
        &[Drag::Point {
            point: points[2],
            target: v(25.0, -10.0),
        }],
    );
    assert!(report.converged);
    assert!(s.point(points[2]).distance(v(25.0, -10.0)) < 1e-6);
    assert!(s.point(points[1]).distance(before[1]) < 1e-6);
    // Dragging the curve itself moves the whole spline.
    let grab = s.curve(spline).unwrap().point_at(0.4);
    let report = solver.solve_drag(
        &mut s,
        &[Drag::Curve {
            curve: spline,
            grab,
            target: grab + v(5.0, 7.0),
        }],
    );
    assert!(report.converged);
    assert!(s.point(points[0]).distance(before[0] + v(5.0, 7.0)) < 1e-4);
    assert!(s.point(points[3]).distance(before[3] + v(5.0, 7.0)) < 1e-4);
}

#[test]
fn mirror_copies_a_spline_and_ties_its_points() {
    let mut s = Sketch::new();
    let axis = s.add_line(v(0.0, -20.0), v(0.0, 20.0));
    s.set_construction(axis, true);
    s.fix(axis).unwrap();
    // An arch that starts on the axis.
    let spline = s
        .add_spline(
            &[v(0.0, 10.0), v(8.0, 14.0), v(15.0, 3.0), v(12.0, -6.0)],
            false,
        )
        .unwrap();
    let copies = ops::mirror(&mut s, &[spline], axis).unwrap();
    assert_eq!(copies.len(), 1);
    let copy = copies[0];
    assert_eq!(s.kind(copy), Some(EntityKind::Spline));
    let originals = s.spline_points(spline).unwrap().0.to_vec();
    let mirrored = s.spline_points(copy).unwrap().0.to_vec();
    assert_eq!(mirrored.len(), 4);
    for (a, b) in originals.iter().zip(&mirrored) {
        let (p, q) = (s.point(*a), s.point(*b));
        assert!(q.distance(v(-p.x, p.y)) < 1e-9);
    }
    // Three symmetric pairs, and the point on the axis joined to its copy.
    let symmetric = s
        .constraints()
        .filter(|(_, c)| matches!(c.kind, ConstraintKind::Symmetric { .. }))
        .count();
    assert_eq!(symmetric, 3);
    assert!(s.constraints().any(|(_, c)| matches!(
        c.kind,
        ConstraintKind::Coincident(a, b)
            if (a, b) == (mirrored[0], originals[0]) || (a, b) == (originals[0], mirrored[0])
    )));
    // Moving an original point moves its mirror image.
    let mut solver = Solver::new();
    let report = solver.solve_drag(
        &mut s,
        &[Drag::Point {
            point: originals[2],
            target: v(20.0, 5.0),
        }],
    );
    assert!(report.converged);
    assert!(s.point(mirrored[2]).distance(v(-20.0, 5.0)) < 1e-5);
    // The two halves meet on the axis: with a line across the open ends, one region.
    let (_, end) = s.endpoints(spline).unwrap();
    let (_, other) = s.endpoints(copy).unwrap();
    s.add_line(s.point(end), s.point(other));
    assert_eq!(find_regions(&s).regions.len(), 1);

    // A closed spline mirrors to a closed spline.
    let blob = s
        .add_spline(&[v(30.0, 0.0), v(36.0, 4.0), v(33.0, 9.0)], true)
        .unwrap();
    let copy = ops::mirror(&mut s, &[blob], axis).unwrap()[0];
    assert!(s.spline_points(copy).unwrap().1);
    assert!(s.curve(copy).unwrap().is_closed());
}

#[test]
fn trim_extend_offset_and_fillet_refuse_splines_and_say_what_to_do() {
    let mut s = Sketch::new();
    let spline = wave(&mut s);
    let line = s.add_line(v(20.0, -10.0), v(20.0, 10.0));
    let before = s.clone();

    let message = unsupported(ops::trim(&mut s, spline, v(10.0, 8.0)));
    assert!(message.contains("trimming a spline") && message.contains("fit points"));
    let message = unsupported(ops::extend(&mut s, spline, v(40.0, 6.0)));
    assert!(message.contains("extending a spline") && message.contains("end point"));
    let message = unsupported(ops::offset(&mut s, &[spline], 2.0));
    assert!(message.contains("offsetting a spline"));
    let message = unsupported(ops::offset(&mut s, &[line, spline], 2.0));
    assert!(message.contains("offsetting a spline"));
    assert_eq!(s, before, "a refusal changes nothing");

    // A fillet between a line and a spline that share a corner.
    let (points, _) = s.spline_points(spline).unwrap();
    let start = points[0];
    let to_corner = s.add_line(v(-10.0, 0.0), v(0.0, 0.0));
    let (_, end) = s.endpoints(to_corner).unwrap();
    s.add_constraint(ConstraintKind::Coincident(end, start))
        .unwrap();
    let message = unsupported(ops::fillet(&mut s, start, 2.0));
    assert!(message.contains("two lines"), "{message}");
}

#[test]
fn lines_are_trimmed_and_extended_at_a_spline() {
    // The spline cuts a line that crosses it: the cut end stays where it was cut, with
    // no relation (a spline can't hold a point).
    let mut s = Sketch::new();
    let spline = wave(&mut s);
    let line = s.add_line(v(20.0, -10.0), v(20.0, 10.0));
    let relations = s.constraints().count();
    let left = ops::trim(&mut s, line, v(20.0, 9.0)).unwrap();
    assert_eq!(left, vec![line]);
    let curve = s.curve(line).unwrap();
    let cut = if curve.start().y > curve.end().y {
        curve.start()
    } else {
        curve.end()
    };
    assert!(s.curve(spline).unwrap().distance(cut) < 1e-6);
    assert!((curve.length() - (cut.y + 10.0)).abs() < 1e-6);
    assert_eq!(s.constraints().count(), relations);

    // A line short of the spline is extended up to it.
    let mut s = Sketch::new();
    let spline = wave(&mut s);
    let line = s.add_line(v(30.0, -20.0), v(30.0, -12.0));
    ops::extend(&mut s, line, v(30.0, -12.0)).unwrap();
    let end = s.curve(line).unwrap().end();
    assert!((end.x - 30.0).abs() < 1e-9);
    assert!(s.curve(spline).unwrap().distance(end) < 1e-6);
}

#[test]
fn inference_snaps_to_fit_points_not_to_the_curve() {
    use peet_sketch::infer::{InferenceSettings, Inferred, infer};
    let mut s = Sketch::new();
    let spline = wave(&mut s);
    let points = s.spline_points(spline).unwrap().0.to_vec();
    let settings = InferenceSettings {
        snap_distance: 1.0,
        angle_tolerance_deg: 3.0,
        enabled: true,
    };
    let near_point = infer(&s, v(10.3, 8.2), None, &settings, &[]);
    assert_eq!(near_point.pos, v(10.0, 8.0));
    assert!(
        near_point
            .relations
            .contains(&Inferred::Coincident(points[1]))
    );
    // On the curve between two fit points: nothing to hold the point there.
    let on_curve = s.curve(spline).unwrap().point_at(0.5);
    let r = infer(&s, on_curve, None, &settings, &[]);
    assert!(
        !r.relations
            .iter()
            .any(|r| matches!(r, Inferred::OnCurve(_)))
    );
}
