//! Options of solid features that SolidWorks parts use: regions picked side by side.

mod common;

use std::f64::consts::PI;

use common::{Part, TOP, assert_close};
use peet_math::{DVec2, DVec3};
use peet_model::{EndCondition, Operation, RegionSelection, Scalar};

/// A 50 × 50 × 10 plate on the top plane.
fn plate() -> Part {
    let mut p = Part::default();
    let base = p.sketch(TOP, |s| {
        peet_sketch::shapes::rectangle(s, DVec2::ZERO, DVec2::new(50.0, 50.0));
    });
    p.extrude(base, Operation::Add, |e| e.depth = Scalar::new(10.0));
    p.rebuild();
    p
}

#[test]
fn regions_side_by_side_are_cut_as_one() {
    // A square and the half disc on its left side: two regions that share the side.
    let mut p = plate();
    let top = p.on_face(DVec3::Z, DVec3::new(1.0, 1.0, 10.0));
    let sketch = p.sketch(top, |s| {
        peet_sketch::shapes::rectangle(s, DVec2::new(10.0, 10.0), DVec2::new(20.0, 20.0));
        s.add_arc(
            DVec2::new(10.0, 15.0),
            DVec2::new(10.0, 20.0),
            DVec2::new(10.0, 10.0),
        );
    });
    p.extrude(sketch, Operation::Cut, |e| {
        e.end = EndCondition::ThroughAll;
        e.regions = RegionSelection::Points(vec![DVec2::new(15.0, 15.0), DVec2::new(7.0, 15.0)]);
    });
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), 25_000.0 - 10.0 * (100.0 + PI * 12.5));
}
