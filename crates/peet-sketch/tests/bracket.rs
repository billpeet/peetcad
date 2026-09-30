//! End-to-end test for the Phase 1 exit criterion: draw and fully constrain a realistic
//! bracket profile (an L-shaped plate with an inner fillet, two holes and a slot), then
//! change a dimension and check that everything follows.

use peet_math::DVec2;
use peet_sketch::region::find_regions;
use peet_sketch::solver::DofStatus;
use peet_sketch::{ConstraintId, ConstraintKind as C, EntityId, Sketch, Solver, ops, shapes};

struct Bracket {
    sketch: Sketch,
    width: ConstraintId,
    hole: EntityId,
}

fn v(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

/// Draws a closed polyline with coincident corners; returns the lines.
fn polyline(s: &mut Sketch, pts: &[DVec2]) -> Vec<EntityId> {
    let n = pts.len();
    let lines: Vec<EntityId> = (0..n)
        .map(|i| s.add_line(pts[i], pts[(i + 1) % n]))
        .collect();
    for i in 0..n {
        let (_, end) = s.endpoints(lines[i]).unwrap();
        let (start, _) = s.endpoints(lines[(i + 1) % n]).unwrap();
        s.add_constraint(C::Coincident(end, start)).unwrap();
    }
    lines
}

fn build() -> Bracket {
    let mut s = Sketch::new();
    // Drawn roughly, as a user would, then pulled into shape by dimensions.
    let l = polyline(
        &mut s,
        &[
            v(0.0, 0.0),
            v(78.0, 0.0),
            v(78.0, 11.0),
            v(11.0, 11.0),
            v(11.0, 58.0),
            v(0.0, 58.0),
        ],
    );
    for (i, line) in l.iter().enumerate() {
        let kind = if i % 2 == 0 {
            C::Horizontal(*line)
        } else {
            C::Vertical(*line)
        };
        s.add_constraint(kind).unwrap();
    }
    let (corner, _) = s.endpoints(l[0]).unwrap();
    s.add_constraint(C::Coincident(Sketch::ORIGIN, corner))
        .unwrap();
    // Lines: bottom, end of the horizontal leg, top of that leg, inner upright, top of the
    // vertical leg, left side.
    let width = s.add_dimension(C::Length(l[0]), 80.0).unwrap();
    s.add_dimension(C::Length(l[1]), 10.0).unwrap();
    s.add_dimension(C::Length(l[4]), 10.0).unwrap();
    s.add_dimension(C::Length(l[5]), 60.0).unwrap();
    let mut solver = Solver::new();
    assert!(solver.solve(&mut s).converged);

    // Inner corner fillet.
    let (inner, _) = s.endpoints(l[3]).unwrap();
    ops::fillet(&mut s, inner, 5.0).expect("fillet the inner corner");

    // Two holes, positioned from the origin.
    let hole = s.add_circle(v(64.0, 6.0), 2.2);
    s.add_dimension(C::Diameter(hole), 4.5).unwrap();
    let hc = s.center(hole).unwrap();
    s.add_dimension(C::HorizontalDistance(Sketch::ORIGIN, hc), 65.0)
        .unwrap();
    s.add_dimension(C::VerticalDistance(Sketch::ORIGIN, hc), 5.0)
        .unwrap();
    let hole2 = s.add_circle(v(6.0, 44.0), 2.0);
    s.add_constraint(C::Equal(hole, hole2)).unwrap();
    let hc2 = s.center(hole2).unwrap();
    s.add_dimension(C::HorizontalDistance(Sketch::ORIGIN, hc2), 5.0)
        .unwrap();
    s.add_dimension(C::VerticalDistance(Sketch::ORIGIN, hc2), 45.0)
        .unwrap();

    // A vertical slot in the upright leg.
    let slot = shapes::slot(&mut s, v(5.5, 19.0), v(5.5, 33.0), 1.8);
    let arcs: Vec<EntityId> = slot
        .curves
        .iter()
        .copied()
        .filter(|c| s.kind(*c) == Some(peet_sketch::EntityKind::Arc))
        .collect();
    let (c1, c2) = (s.center(arcs[1]).unwrap(), s.center(arcs[0]).unwrap());
    s.add_constraint(C::VerticalPoints(c1, c2)).unwrap();
    s.add_dimension(C::HorizontalDistance(Sketch::ORIGIN, c1), 5.0)
        .unwrap();
    s.add_dimension(C::VerticalDistance(Sketch::ORIGIN, c1), 20.0)
        .unwrap();
    s.add_dimension(C::Distance(c1, c2), 15.0).unwrap();
    s.add_dimension(C::Radius(arcs[0]), 2.0).unwrap();

    Bracket {
        sketch: s,
        width,
        hole,
    }
}

#[test]
fn bracket_is_fully_defined_and_follows_edits() {
    let Bracket {
        mut sketch,
        width,
        hole,
    } = build();
    let mut solver = Solver::new();
    let report = solver.solve(&mut sketch);
    assert!(report.converged, "{report:?}");
    let analysis = solver.analyze(&sketch);
    assert!(analysis.diagnoses.is_empty(), "{:?}", analysis.diagnoses);
    assert_eq!(analysis.dof, 0, "bracket should be fully defined");
    for (id, _) in sketch.entities() {
        assert_eq!(
            analysis.entity_status(id),
            DofStatus::Fully,
            "entity {id:?}"
        );
    }

    // One region: the plate, with three holes (two circles and the slot).
    let profile = find_regions(&sketch);
    assert!(profile.open_ends.is_empty(), "{:?}", profile.open_ends);
    let plates: Vec<_> = profile
        .regions
        .iter()
        .filter(|r| r.holes.len() == 3)
        .collect();
    assert_eq!(plates.len(), 1, "{} regions", profile.regions.len());
    let plate = plates[0];
    // The L, plus the material the inner fillet adds, minus two holes and the slot.
    let pi = std::f64::consts::PI;
    let expected = 80.0 * 10.0 + 10.0 * 50.0 + (25.0 - pi * 25.0 / 4.0)
        - 2.0 * pi * 2.25 * 2.25
        - (15.0 * 4.0 + pi * 4.0);
    assert!(
        (plate.area() - expected).abs() < 1e-6,
        "area {} vs {expected}",
        plate.area()
    );

    // Widen the bracket: the far end moves, the hole stays put (it's dimensioned from the origin).
    sketch
        .constraint_mut(width)
        .unwrap()
        .dimension
        .as_mut()
        .unwrap()
        .value = 100.0;
    let report = solver.solve(&mut sketch);
    assert!(report.converged, "{report:?}");
    let center = sketch.curve(hole).unwrap().center().unwrap();
    assert!(center.abs_diff_eq(v(65.0, 5.0), 1e-9), "{center:?}");
    let analysis = solver.analyze(&sketch);
    assert_eq!(analysis.dof, 0);
    assert!(analysis.diagnoses.is_empty());
}
