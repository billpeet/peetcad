//! Options of solid features that SolidWorks parts use: regions picked side by side.

mod common;

use std::f64::consts::PI;

use common::{FRONT, Part, TOP, assert_close};
use peet_math::{DVec2, DVec3};
use peet_model::{EndCondition, Operation, RegionSelection, Scalar, StdPlane};

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
fn a_cut_can_take_the_other_side_of_its_sketch_and_lean_its_walls() {
    // Outside a circle on the top, with walls leaning 45° from the cut's direction: what is
    // left is a frustum, radius 10 at the top and 20 at the bottom, as SolidWorks chamfers
    // a bolt's head with a drafted cut flipped to the outside.
    let mut p = plate();
    let top = p.on_face(DVec3::Z, DVec3::new(1.0, 1.0, 10.0));
    let sketch = p.sketch(top, |s| {
        s.add_circle(DVec2::new(25.0, 25.0), 10.0);
    });
    let cut = p.extrude(sketch, Operation::Cut, |e| {
        e.end = EndCondition::ThroughAll;
        e.flip_side = true;
        e.draft = Scalar::new(45.0);
    });
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), PI * 10.0 / 3.0 * (100.0 + 200.0 + 400.0));
    // Without the draft: a cylinder is left.
    p.extrude_mut(cut).params.draft = Scalar::new(0.0);
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), PI * 100.0 * 10.0);
}

#[test]
fn an_extrusion_with_drafted_walls() {
    // A 10 × 10 square, 10 high, its walls leaning 10° in: a frustum of a pyramid.
    let mut p = Part::default();
    let base = p.sketch(TOP, |s| {
        peet_sketch::shapes::rectangle(s, DVec2::ZERO, DVec2::new(10.0, 10.0));
    });
    let boss = p.extrude(base, Operation::Add, |e| {
        e.depth = Scalar::new(10.0);
        e.draft = Scalar::new(10.0);
    });
    p.rebuild();
    p.assert_ok();
    let side = 10.0 - 2.0 * 10.0 * 10f64.to_radians().tan();
    let frustum = |a: f64, b: f64| 10.0 / 3.0 * (a * a + b * b + a * b);
    assert_close(p.volume(), frustum(10.0, side));
    // Leaning out, it grows.
    p.extrude_mut(boss).params.draft_outward = true;
    p.rebuild();
    p.assert_ok();
    let side = 10.0 + 2.0 * 10.0 * 10f64.to_radians().tan();
    assert_close(p.volume(), frustum(10.0, side));
}

/// A sweep of a circle of radius 2 on the top plane along the line from `a` to `b`, drawn on
/// the front plane.
fn swept(a: DVec3, b: DVec3) -> Part {
    let mut p = Part::default();
    let profile = p.sketch(TOP, |s| {
        s.add_circle(DVec2::ZERO, 2.0);
    });
    let front = StdPlane::Front.plane();
    let path = p.sketch(FRONT, |s| {
        s.add_line(front.to_plane_coords(a), front.to_plane_coords(b));
    });
    p.model.add_sweep(profile, Some(path), Operation::Add);
    p.rebuild();
    p
}

#[test]
fn a_sweep_follows_its_path_both_ways_from_the_profile() {
    // The profile part way along the path, as SolidWorks allows: the whole path is swept.
    let p = swept(DVec3::new(0.0, 0.0, -10.0), DVec3::new(0.0, 0.0, 20.0));
    p.assert_ok();
    assert_close(p.volume(), PI * 4.0 * 30.0);
}

#[test]
fn a_sweep_along_a_slanted_path_keeps_the_profile_as_drawn() {
    // A path leaving the profile's plane at a slant: the profile is carried along it
    // without turning, an oblique cylinder as tall as the path rises.
    let p = swept(DVec3::ZERO, DVec3::new(5.0, 0.0, 20.0));
    p.assert_ok();
    assert!(
        (p.volume() - PI * 4.0 * 20.0).abs() < 1e-6 * PI * 80.0,
        "{}",
        p.volume()
    );
    let b = p.bodies()[0].solid.bounds();
    assert!(
        (b.max.x - 7.0).abs() < 1e-3 && (b.min.x + 2.0).abs() < 1e-3,
        "{b:?}"
    );
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
