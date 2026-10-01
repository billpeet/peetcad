//! Boolean tests: topology counts, validity and volumes against analytic values.

use std::f64::consts::PI;

use peet_math::{DQuat, DVec2, DVec3, Frame, Plane};
use peet_sketch::Sketch;

use super::{BooleanOp, boolean};
use crate::geom::{Curve3, Surface};
use crate::validate::{Counts, assert_valid, measure};
use crate::{KernelError, Solid};

/// Solids for tests.
pub(crate) mod shapes {
    use std::f64::consts::TAU;

    use peet_math::{DVec2, DVec3, Frame, Plane};
    use peet_sketch::Sketch;
    use peet_sketch::region::find_regions;

    use crate::Solid;
    use crate::geom::{Circle3, Curve3, Cylinder, Ellipse3, Line3, Surface};
    pub(crate) use crate::topo::test_shapes::cuboid;

    /// A hand-built cylinder (independent of the extrude code): `base` is the centre of
    /// the bottom cap, the seam is along the frame's X direction.
    pub fn cylinder(base: DVec3, axis: DVec3, radius: f64, height: f64) -> Solid {
        let x_hint = if axis.normalize().dot(DVec3::X).abs() > 0.9 {
            DVec3::Y
        } else {
            DVec3::X
        };
        let frame = Frame::from_origin_z_x(base, axis, x_hint).unwrap();
        let top_frame = Frame {
            origin: base + frame.z_axis() * height,
            ..frame
        };
        let mut s = Solid::new();
        let shell = s.add_shell();
        let vb = s.add_vertex(frame.to_world(DVec3::X * radius));
        let vt = s.add_vertex(top_frame.to_world(DVec3::X * radius));
        let circle = |frame: Frame| Curve3::Circle(Circle3 { frame, radius });
        let bottom = s.add_edge(circle(frame), vb, vb, 0.0, TAU);
        let top = s.add_edge(circle(top_frame), vt, vt, 0.0, TAU);
        let seam = s.add_line_edge(vb, vt);
        let side = s.add_face(shell, Surface::Cylinder(Cylinder { frame, radius }), false);
        s.add_loop(
            side,
            &[(bottom, false), (seam, false), (top, true), (seam, true)],
        );
        let cap = s.add_face(shell, Surface::Plane(Plane { frame: top_frame }), false);
        s.add_loop(cap, &[(top, false)]);
        let cap = s.add_face(shell, Surface::Plane(Plane { frame }), true);
        s.add_loop(cap, &[(bottom, true)]);
        s
    }

    /// Extrudes the region of `sketch` containing `at` from `from` to `to` along the
    /// plane's normal.
    pub fn prism(plane: &Plane, sketch: &Sketch, at: DVec2, from: f64, to: f64) -> Solid {
        let profile = find_regions(sketch);
        let region = profile.region_at(at).expect("a region at the pick point");
        crate::extrude::extrude(plane, &[profile.regions[region].clone()], from, to).unwrap()
    }

    /// An extruded rectangle on `plane`.
    pub fn block(plane: &Plane, a: DVec2, b: DVec2, from: f64, to: f64) -> Solid {
        let mut sketch = Sketch::new();
        peet_sketch::shapes::rectangle(&mut sketch, a, b);
        prism(plane, &sketch, 0.5 * (a + b), from, to)
    }

    /// An extruded circle on `plane`.
    pub fn rod(plane: &Plane, center: DVec2, radius: f64, from: f64, to: f64) -> Solid {
        let mut sketch = Sketch::new();
        sketch.add_circle(center, radius);
        // Not the centre itself: `region_at` can't decide a point on the chord between
        // the two arcs a circle is split into.
        let pick = center + DVec2::new(0.3, 0.2) * radius;
        prism(plane, &sketch, pick, from, to)
    }

    /// The solid moved by `frame` (as a placement of its local coordinates).
    pub fn placed(solid: &Solid, frame: &Frame) -> Solid {
        let mut s = solid.clone();
        for v in &mut s.vertices {
            v.point = frame.to_world(v.point);
        }
        for e in &mut s.edges {
            e.curve = match e.curve {
                Curve3::Line(l) => Curve3::Line(Line3 {
                    origin: frame.to_world(l.origin),
                    dir: frame.vector_to_world(l.dir),
                }),
                Curve3::Circle(c) => Curve3::Circle(Circle3 {
                    frame: frame.compose(&c.frame),
                    radius: c.radius,
                }),
                Curve3::Ellipse(c) => Curve3::Ellipse(Ellipse3 {
                    frame: frame.compose(&c.frame),
                    ..c
                }),
            };
        }
        for f in &mut s.faces {
            f.surface = match f.surface {
                Surface::Plane(p) => Surface::Plane(Plane {
                    frame: frame.compose(&p.frame),
                }),
                Surface::Cylinder(c) => Surface::Cylinder(Cylinder {
                    frame: frame.compose(&c.frame),
                    radius: c.radius,
                }),
            };
        }
        s
    }
}

use shapes::{block, cuboid, cylinder, placed, prism, rod};

fn v(x: f64, y: f64, z: f64) -> DVec3 {
    DVec3::new(x, y, z)
}

/// Runs a boolean and checks the result is valid.
#[track_caller]
fn run(a: &Solid, b: &Solid, op: BooleanOp) -> (Solid, Counts) {
    let s = match boolean(a, b, op) {
        Ok(s) => s,
        Err(e) => panic!("{op:?} failed: {e}"),
    };
    let counts = assert_valid(&s);
    if let Err(e) = crate::tessellate::tessellate(&s, 0.05) {
        panic!("{op:?} gave a solid that can't be tessellated: {e}");
    }
    (s, counts)
}

fn volume(s: &Solid) -> f64 {
    measure::volume(s)
}

#[track_caller]
fn assert_volume(s: &Solid, expected: f64) {
    let got = volume(s);
    assert!(
        (got - expected).abs() <= 1e-9 * expected.abs().max(1.0),
        "volume {got} instead of {expected}"
    );
}

/// `(vertices, edges, faces, rings, shells, genus)`.
fn tuple(c: Counts) -> (usize, usize, usize, usize, usize, i64) {
    (c.vertices, c.edges, c.faces, c.rings, c.shells, c.genus)
}

fn count_surfaces(s: &Solid) -> (usize, usize) {
    let planes = s
        .faces
        .iter()
        .filter(|f| matches!(f.surface, Surface::Plane(_)))
        .count();
    (planes, s.faces.len() - planes)
}

fn unit_box() -> Solid {
    cuboid(DVec3::ZERO, DVec3::splat(10.0))
}

// ---- Box and box ----

#[test]
fn box_minus_box_corner_notch() {
    let a = unit_box();
    let b = cuboid(v(6.0, 6.0, 6.0), v(12.0, 12.0, 12.0));
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, 1000.0 - 64.0);
    // Three faces lose a corner, three new faces appear.
    assert_eq!(tuple(c), (14, 21, 9, 0, 1, 0));
}

#[test]
fn box_minus_box_through_slot() {
    let a = unit_box();
    // A slot across the top, open at both ends and above.
    let b = cuboid(v(3.0, -1.0, 6.0), v(7.0, 11.0, 12.0));
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, 1000.0 - 4.0 * 10.0 * 4.0);
    assert_eq!(tuple(c), (16, 24, 10, 0, 1, 0));
}

#[test]
fn box_minus_box_pocket_starting_on_the_face() {
    let a = unit_box();
    let b = cuboid(v(3.0, 3.0, 6.0), v(7.0, 7.0, 10.0));
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, 1000.0 - 64.0);
    // The top face keeps one region with a hole: 6 + 5 faces, one ring.
    assert_eq!(tuple(c), (16, 24, 11, 1, 1, 0));
}

#[test]
fn box_minus_box_blind_pocket_from_above() {
    let a = unit_box();
    let b = cuboid(v(3.0, 3.0, 6.0), v(7.0, 7.0, 14.0));
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, 1000.0 - 64.0);
    assert_eq!(tuple(c), (16, 24, 11, 1, 1, 0));
}

#[test]
fn box_minus_box_through_window() {
    let a = unit_box();
    let b = cuboid(v(3.0, 3.0, 0.0), v(7.0, 7.0, 10.0));
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, 1000.0 - 160.0);
    // Exactly reaching both faces leaves no zero-thickness caps: a handle.
    assert_eq!(tuple(c), (16, 24, 10, 2, 1, 1));
}

#[test]
fn box_minus_inner_box_leaves_a_void() {
    let a = unit_box();
    let b = cuboid(v(3.0, 3.0, 3.0), v(7.0, 7.0, 7.0));
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, 1000.0 - 64.0);
    assert_eq!(tuple(c), (16, 24, 12, 0, 2, 0));
    assert!(measure::shell_volume(&s, crate::ShellId(0)) > 0.0);
    assert!(measure::shell_volume(&s, crate::ShellId(1)) < 0.0);
}

#[test]
fn box_union_box_overlapping() {
    let a = unit_box();
    let b = cuboid(v(5.0, 5.0, 5.0), v(15.0, 15.0, 15.0));
    let (s, c) = run(&a, &b, BooleanOp::Union);
    assert_volume(&s, 2000.0 - 125.0);
    assert_eq!(tuple(c), (20, 30, 12, 0, 1, 0));
}

#[test]
fn box_union_box_touching_face_to_face() {
    let a = unit_box();
    // Same cross-section: the result is one plain box, with no seam around it.
    let b = cuboid(v(10.0, 0.0, 0.0), v(15.0, 10.0, 10.0));
    let (s, c) = run(&a, &b, BooleanOp::Union);
    assert_volume(&s, 1500.0);
    assert_eq!(tuple(c), (8, 12, 6, 0, 1, 0));
    // A smaller box on the face: a boss.
    let b = cuboid(v(10.0, 3.0, 3.0), v(15.0, 7.0, 7.0));
    let (s, c) = run(&a, &b, BooleanOp::Union);
    assert_volume(&s, 1000.0 + 80.0);
    assert_eq!(tuple(c), (16, 24, 11, 1, 1, 0));
    // Order doesn't matter.
    let (s, c) = run(&b, &a, BooleanOp::Union);
    assert_volume(&s, 1000.0 + 80.0);
    assert_eq!(tuple(c), (16, 24, 11, 1, 1, 0));
}

#[test]
fn box_union_box_sharing_part_of_a_face() {
    let a = unit_box();
    // Touching face to face, shifted: the shared rectangle disappears.
    let b = cuboid(v(10.0, 5.0, 5.0), v(15.0, 15.0, 15.0));
    let (s, c) = run(&a, &b, BooleanOp::Union);
    assert_volume(&s, 1500.0);
    assert_eq!(c.shells, 1);
    assert_eq!(c.genus, 0);
    assert_eq!(c.faces, 12);
}

#[test]
fn box_union_box_coplanar_sides() {
    let a = unit_box();
    // Overlapping, with four faces in common planes: one longer box.
    let b = cuboid(v(5.0, 0.0, 0.0), v(15.0, 10.0, 10.0));
    let (s, c) = run(&a, &b, BooleanOp::Union);
    assert_volume(&s, 1500.0);
    assert_eq!(tuple(c), (8, 12, 6, 0, 1, 0));
}

#[test]
fn box_union_box_disjoint() {
    let a = unit_box();
    let b = cuboid(v(20.0, 0.0, 0.0), v(25.0, 5.0, 5.0));
    let (s, c) = run(&a, &b, BooleanOp::Union);
    assert_volume(&s, 1125.0);
    assert_eq!(tuple(c), (16, 24, 12, 0, 2, 0));
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, 1000.0);
    assert_eq!(tuple(c), (8, 12, 6, 0, 1, 0));
    let (s, _) = run(&a, &b, BooleanOp::Intersect);
    assert!(s.faces.is_empty() && s.shells.is_empty());
}

#[test]
fn box_union_contained_box() {
    let a = unit_box();
    let b = cuboid(v(2.0, 2.0, 2.0), v(5.0, 5.0, 5.0));
    let (s, c) = run(&a, &b, BooleanOp::Union);
    assert_volume(&s, 1000.0);
    assert_eq!(tuple(c), (8, 12, 6, 0, 1, 0));
    let (s, c) = run(&b, &a, BooleanOp::Union);
    assert_volume(&s, 1000.0);
    assert_eq!(tuple(c), (8, 12, 6, 0, 1, 0));
}

#[test]
fn box_intersect_box() {
    let a = unit_box();
    let b = cuboid(v(5.0, 6.0, 7.0), v(15.0, 15.0, 15.0));
    let (s, c) = run(&a, &b, BooleanOp::Intersect);
    assert_volume(&s, 5.0 * 4.0 * 3.0);
    assert_eq!(tuple(c), (8, 12, 6, 0, 1, 0));
    // With shared face planes.
    let b = cuboid(v(5.0, 0.0, 0.0), v(15.0, 10.0, 4.0));
    let (s, c) = run(&a, &b, BooleanOp::Intersect);
    assert_volume(&s, 5.0 * 10.0 * 4.0);
    assert_eq!(tuple(c), (8, 12, 6, 0, 1, 0));
    // Touching only: nothing in common.
    let b = cuboid(v(10.0, 0.0, 0.0), v(15.0, 10.0, 10.0));
    let (s, _) = run(&a, &b, BooleanOp::Intersect);
    assert!(s.faces.is_empty());
}

#[test]
fn cut_that_removes_everything() {
    let a = unit_box();
    let b = cuboid(v(-1.0, -1.0, -1.0), v(11.0, 11.0, 11.0));
    let (s, _) = run(&a, &b, BooleanOp::Subtract);
    assert!(s.faces.is_empty() && s.shells.is_empty() && s.vertices.is_empty());
    // The same box exactly.
    let (s, _) = run(&a, &unit_box(), BooleanOp::Subtract);
    assert!(s.faces.is_empty());
    let (s, c) = run(&a, &unit_box(), BooleanOp::Union);
    assert_volume(&s, 1000.0);
    assert_eq!(tuple(c), (8, 12, 6, 0, 1, 0));
    let (s, c) = run(&a, &unit_box(), BooleanOp::Intersect);
    assert_volume(&s, 1000.0);
    assert_eq!(tuple(c), (8, 12, 6, 0, 1, 0));
}

#[test]
fn cut_splits_a_body_in_two() {
    let a = unit_box();
    let b = cuboid(v(4.0, -1.0, -1.0), v(6.0, 11.0, 11.0));
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, 800.0);
    assert_eq!(tuple(c), (16, 24, 12, 0, 2, 0));
}

#[test]
fn bodies_touching_along_an_edge_or_at_a_corner() {
    let a = unit_box();
    // Along an edge: two shells, each with its own copy of the edge.
    let b = cuboid(v(10.0, 10.0, 0.0), v(20.0, 20.0, 10.0));
    let (s, c) = run(&a, &b, BooleanOp::Union);
    assert_volume(&s, 2000.0);
    assert_eq!(tuple(c), (16, 24, 12, 0, 2, 0));
    // At a corner.
    let b = cuboid(v(10.0, 10.0, 10.0), v(20.0, 20.0, 20.0));
    let (s, c) = run(&a, &b, BooleanOp::Union);
    assert_volume(&s, 2000.0);
    assert_eq!(tuple(c), (16, 24, 12, 0, 2, 0));
    // A cut leaving two parts that touch along an edge.
    let big = cuboid(DVec3::ZERO, v(20.0, 20.0, 10.0));
    let (s, _) = run(&big, &a, BooleanOp::Subtract);
    let b = cuboid(v(10.0, 10.0, -1.0), v(21.0, 21.0, 11.0));
    let (s, c) = run(&s, &b, BooleanOp::Subtract);
    assert_volume(&s, 2000.0);
    assert_eq!(c.shells, 2);
}

// ---- Cylinders ----

#[test]
fn box_minus_cylinder_through_hole() {
    let a = unit_box();
    let b = cylinder(v(5.0, 5.0, -2.0), DVec3::Z, 2.0, 14.0);
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, 1000.0 - PI * 4.0 * 10.0);
    // 8 box vertices + 2 on the seam; 12 + 2 circles + seam; 6 + the wall.
    assert_eq!(tuple(c), (10, 15, 7, 2, 1, 1));
    assert_eq!(count_surfaces(&s), (6, 1));
}

#[test]
fn box_minus_cylinder_through_hole_exactly_to_the_faces() {
    let a = unit_box();
    let b = cylinder(v(5.0, 5.0, 0.0), DVec3::Z, 2.0, 10.0);
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, 1000.0 - PI * 4.0 * 10.0);
    assert_eq!(tuple(c), (10, 15, 7, 2, 1, 1));
}

#[test]
fn box_minus_cylinder_blind_hole() {
    let a = unit_box();
    for start in [10.0, 13.0] {
        let b = cylinder(v(5.0, 5.0, 4.0), DVec3::Z, 2.0, start - 4.0);
        let (s, c) = run(&a, &b, BooleanOp::Subtract);
        assert_volume(&s, 1000.0 - PI * 4.0 * 6.0);
        assert_eq!(tuple(c), (10, 15, 8, 1, 1, 0));
        assert_eq!(count_surfaces(&s), (7, 1));
    }
}

#[test]
fn box_minus_cylinder_tangent_to_a_side() {
    let a = unit_box();
    // The hole's wall touches the face x = 10 along a line.
    let b = cylinder(v(8.0, 5.0, -2.0), DVec3::Z, 2.0, 14.0);
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, 1000.0 - PI * 4.0 * 10.0);
    assert_eq!(c.shells, 1);
    assert_eq!(count_surfaces(&s), (6, 1));
}

#[test]
fn box_minus_cylinder_breaking_through_a_side() {
    let a = unit_box();
    // Centre 1 mm inside the face x = 10: the hole opens the side.
    let b = cylinder(v(9.0, 5.0, -2.0), DVec3::Z, 2.0, 14.0);
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    // Circular segment outside the box: r² acos(d/r) − d √(r² − d²), d = 1.
    let outside = 4.0 * (0.5_f64).acos() - 3.0_f64.sqrt();
    assert_volume(&s, 1000.0 - (PI * 4.0 - outside) * 10.0);
    assert_eq!(c.shells, 1);
    assert_eq!(c.genus, 0);
    // The side face is cut in two.
    assert_eq!(count_surfaces(&s), (7, 1));
}

#[test]
fn box_minus_cylinder_breaking_through_an_edge() {
    let a = unit_box();
    // Centred on the vertical edge x = 10, y = 10: a quarter of the cylinder is removed.
    let b = cylinder(v(10.0, 10.0, -2.0), DVec3::Z, 3.0, 14.0);
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, 1000.0 - PI * 9.0 / 4.0 * 10.0);
    assert_eq!(tuple(c), (10, 15, 7, 0, 1, 0));
}

#[test]
fn cylinder_along_x_cuts_a_box_extruded_along_z() {
    let a = block(&Plane::TOP, DVec2::ZERO, DVec2::splat(10.0), 0.0, 10.0);
    // A groove along X in the top face: half the cylinder is in the box.
    let b = cylinder(v(-2.0, 5.0, 10.0), DVec3::X, 3.0, 14.0);
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, 1000.0 - 0.5 * PI * 9.0 * 10.0);
    assert_eq!(c.shells, 1);
    assert_eq!(c.genus, 0);
    assert_eq!(count_surfaces(&s), (7, 1));
    // A hole along X through the middle.
    let b = cylinder(v(-2.0, 5.0, 5.0), DVec3::X, 3.0, 14.0);
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, 1000.0 - PI * 9.0 * 10.0);
    assert_eq!(tuple(c), (10, 15, 7, 2, 1, 1));
}

#[test]
fn slanted_cut_through_a_cylinder_gives_an_ellipse() {
    let a = cylinder(DVec3::ZERO, DVec3::Z, 2.0, 10.0);
    // A big box tilted about the Y axis, its bottom face passing through (0, 0, 5).
    let tilt = Frame {
        origin: v(0.0, 0.0, 5.0),
        rotation: DQuat::from_rotation_y(0.4),
    };
    let b = placed(&cuboid(v(-20.0, -20.0, 0.0), v(20.0, 20.0, 30.0)), &tilt);
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    // The plane passes through the axis at height 5, so the mean height is 5.
    assert_volume(&s, PI * 4.0 * 5.0);
    // Bottom circle, seam and the ellipse, which is closed with its vertex on the seam.
    assert_eq!(tuple(c), (2, 3, 3, 0, 1, 0));
    let ellipses = s
        .edges
        .iter()
        .filter(|e| matches!(e.curve, Curve3::Ellipse(_)))
        .count();
    assert_eq!(ellipses, 1);
    // And the other half.
    let (s, _) = run(&a, &b, BooleanOp::Intersect);
    assert_volume(&s, PI * 4.0 * 5.0);
}

#[test]
fn cylinder_unions() {
    let a = cylinder(DVec3::ZERO, DVec3::Z, 2.0, 5.0);
    // Stacked, same radius: one longer cylinder (the walls are one surface).
    let b = cylinder(v(0.0, 0.0, 5.0), DVec3::Z, 2.0, 3.0);
    let (s, c) = run(&a, &b, BooleanOp::Union);
    assert_volume(&s, PI * 4.0 * 8.0);
    assert_eq!(tuple(c), (2, 3, 3, 0, 1, 0));
    // Overlapping along the axis.
    let b = cylinder(v(0.0, 0.0, 3.0), DVec3::Z, 2.0, 5.0);
    let (s, c) = run(&a, &b, BooleanOp::Union);
    assert_volume(&s, PI * 4.0 * 8.0);
    assert_eq!(tuple(c), (2, 3, 3, 0, 1, 0));
    let (s, _) = run(&a, &b, BooleanOp::Intersect);
    assert_volume(&s, PI * 4.0 * 2.0);
    let (s, _) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, PI * 4.0 * 3.0);
    // A thinner cylinder on top: a stepped shaft.
    let b = cylinder(v(0.0, 0.0, 5.0), DVec3::Z, 1.0, 3.0);
    let (s, c) = run(&a, &b, BooleanOp::Union);
    assert_volume(&s, PI * (4.0 * 5.0 + 3.0));
    assert_eq!(c.faces, 5);
    assert_eq!(c.genus, 0);
    // A coaxial bore: a tube.
    let b = cylinder(v(0.0, 0.0, -1.0), DVec3::Z, 1.0, 7.0);
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, PI * 3.0 * 5.0);
    assert_eq!(c.genus, 1);
    assert_eq!(c.faces, 4);
}

#[test]
fn parallel_cylinders_intersect_in_lines() {
    let a = cylinder(DVec3::ZERO, DVec3::Z, 2.0, 5.0);
    let b = cylinder(v(2.0, 0.0, 1.0), DVec3::Z, 2.0, 5.0);
    // Lens between two circles of radius r at distance d: 2 r² acos(d/2r) − d/2 √(4r² − d²).
    let lens = 2.0 * 4.0 * (0.5_f64).acos() - 12.0_f64.sqrt();
    let (s, c) = run(&a, &b, BooleanOp::Intersect);
    assert_volume(&s, lens * 4.0);
    assert_eq!(c.shells, 1);
    let (s, _) = run(&a, &b, BooleanOp::Union);
    assert_volume(&s, 2.0 * PI * 4.0 * 5.0 - lens * 4.0);
    let (s, _) = run(&a, &b, BooleanOp::Subtract);
    assert_volume(&s, PI * 4.0 * 5.0 - lens * 4.0);
    // Externally tangent: they only touch.
    let b = cylinder(v(4.0, 0.0, 0.0), DVec3::Z, 2.0, 5.0);
    let (s, c) = run(&a, &b, BooleanOp::Union);
    assert_volume(&s, 2.0 * PI * 4.0 * 5.0);
    assert_eq!(c.shells, 2);
}

#[test]
fn crossing_cylinders_are_unsupported() {
    let a = cylinder(v(0.0, 0.0, -5.0), DVec3::Z, 2.0, 10.0);
    let b = cylinder(v(-5.0, 0.0, 0.0), DVec3::X, 1.0, 10.0);
    for op in [BooleanOp::Union, BooleanOp::Subtract, BooleanOp::Intersect] {
        let err = boolean(&a, &b, op).unwrap_err();
        assert!(matches!(err, KernelError::Unsupported(_)), "{err:?}");
        assert!(err.to_string().contains("non-parallel"), "{err}");
    }
    // Non-parallel cylinders that don't touch are fine.
    let b = cylinder(v(-5.0, 0.0, 8.0), DVec3::X, 1.0, 10.0);
    let (s, c) = run(&a, &b, BooleanOp::Union);
    assert_eq!(c.shells, 2);
    assert_volume(&s, PI * (40.0 + 10.0));
    // A cross hole through a plate that already has a hole elsewhere.
    let plate = cuboid(v(-10.0, -10.0, 0.0), v(10.0, 10.0, 4.0));
    let (plate, _) = run(
        &plate,
        &cylinder(v(-6.0, 0.0, -1.0), DVec3::Z, 1.0, 6.0),
        BooleanOp::Subtract,
    );
    let cross = cylinder(v(5.0, -11.0, 2.0), DVec3::Y, 1.0, 22.0);
    let (s, c) = run(&plate, &cross, BooleanOp::Subtract);
    assert_eq!(c.genus, 2);
    assert_volume(&s, 1600.0 - PI * 4.0 - PI * 20.0);
}

// ---- Sketch on face ----

#[test]
fn sketch_on_face_pocket_and_boss() {
    let base = block(&Plane::TOP, DVec2::ZERO, DVec2::new(40.0, 30.0), 0.0, 10.0);
    assert_eq!(tuple(assert_valid(&base)), (8, 12, 6, 0, 1, 0));
    let top = Plane {
        frame: Frame {
            origin: v(0.0, 0.0, 10.0),
            ..Frame::WORLD
        },
    };
    // A pocket cut down from the top face.
    let tool = block(
        &top,
        DVec2::new(10.0, 10.0),
        DVec2::new(20.0, 20.0),
        -4.0,
        0.0,
    );
    let (pocket, c) = run(&base, &tool, BooleanOp::Subtract);
    assert_volume(&pocket, 12000.0 - 400.0);
    assert_eq!(tuple(c), (16, 24, 11, 1, 1, 0));
    // A boss added on the top face.
    let tool = block(
        &top,
        DVec2::new(10.0, 10.0),
        DVec2::new(20.0, 20.0),
        0.0,
        4.0,
    );
    let (boss, c) = run(&base, &tool, BooleanOp::Union);
    assert_volume(&boss, 12000.0 + 400.0);
    assert_eq!(tuple(c), (16, 24, 11, 1, 1, 0));
    // A round boss, then a hole through boss and plate from the boss's top.
    let (boss, c) = run(
        &base,
        &rod(&top, DVec2::new(20.0, 15.0), 6.0, 0.0, 5.0),
        BooleanOp::Union,
    );
    assert_volume(&boss, 12000.0 + PI * 36.0 * 5.0);
    assert_eq!(tuple(c), (10, 15, 8, 1, 1, 0));
    let (drilled, c) = run(
        &boss,
        &rod(&top, DVec2::new(20.0, 15.0), 3.0, -10.0, 5.0),
        BooleanOp::Subtract,
    );
    assert_volume(&drilled, 12000.0 + PI * 36.0 * 5.0 - PI * 9.0 * 15.0);
    assert_eq!(c.genus, 1);
    assert_eq!(c.faces, 9);
    // A boss as wide as the plate, flush with three sides.
    let tool = block(&top, DVec2::new(0.0, 0.0), DVec2::new(40.0, 10.0), 0.0, 4.0);
    let (step, c) = run(&base, &tool, BooleanOp::Union);
    assert_volume(&step, 12000.0 + 1600.0);
    assert_eq!(tuple(c), (12, 18, 8, 0, 1, 0));
}

#[test]
fn slot_cut_and_plate_with_hole() {
    // A plate with a round hole (inner cylinder face from the extrude), cut by a slot
    // whose rounded ends are tangent to its flat sides.
    let mut sketch = Sketch::new();
    peet_sketch::shapes::rectangle(&mut sketch, DVec2::ZERO, DVec2::new(60.0, 40.0));
    sketch.add_circle(DVec2::new(45.0, 20.0), 6.0);
    let plate = prism(&Plane::TOP, &sketch, DVec2::new(5.0, 5.0), 0.0, 8.0);
    let c = assert_valid(&plate);
    assert_eq!(c.genus, 1);
    let plate_volume = (2400.0 - PI * 36.0) * 8.0;
    assert_volume(&plate, plate_volume);

    let mut slot = Sketch::new();
    peet_sketch::shapes::slot(
        &mut slot,
        DVec2::new(10.0, 20.0),
        DVec2::new(25.0, 20.0),
        4.0,
    );
    let tool = prism(&Plane::TOP, &slot, DVec2::new(15.0, 21.0), -1.0, 9.0);
    let slot_area = 15.0 * 8.0 + PI * 16.0;
    let (s, c) = run(&plate, &tool, BooleanOp::Subtract);
    assert_volume(&s, plate_volume - slot_area * 8.0);
    assert_eq!(c.genus, 2);
    assert_eq!(c.shells, 1);
    // A blind slot from the top face.
    let tool = prism(&Plane::TOP, &slot, DVec2::new(15.0, 21.0), 5.0, 8.0);
    let (s, c) = run(&plate, &tool, BooleanOp::Subtract);
    assert_volume(&s, plate_volume - slot_area * 3.0);
    assert_eq!(c.genus, 1);
    // A slot that runs into the hole.
    let mut long = Sketch::new();
    peet_sketch::shapes::slot(
        &mut long,
        DVec2::new(10.0, 20.0),
        DVec2::new(45.0, 20.0),
        4.0,
    );
    let tool = prism(&Plane::TOP, &long, DVec2::new(15.0, 21.0), -1.0, 9.0);
    let (s, c) = run(&plate, &tool, BooleanOp::Subtract);
    assert_eq!(c.genus, 1);
    assert_eq!(c.shells, 1);
    assert!(volume(&s) < plate_volume - slot_area * 8.0);
    // Filling the hole back in with a plug of the same radius.
    let plug = rod(&Plane::TOP, DVec2::new(45.0, 20.0), 6.0, 0.0, 8.0);
    let (s, c) = run(&plate, &plug, BooleanOp::Union);
    assert_volume(&s, 2400.0 * 8.0);
    assert_eq!(tuple(c), (8, 12, 6, 0, 1, 0));
}

#[test]
fn features_in_different_directions() {
    // A base extruded along Z, a rib extruded along X, then a hole along Y through both.
    let base = block(&Plane::TOP, DVec2::ZERO, DVec2::new(40.0, 30.0), 0.0, 5.0);
    let right = Plane::right(); // normal +X, local x = +Y, local y = +Z
    let rib = block(
        &right,
        DVec2::new(10.0, 5.0),
        DVec2::new(20.0, 25.0),
        0.0,
        40.0,
    );
    let (body, c) = run(&base, &rib, BooleanOp::Union);
    assert_volume(&body, 6000.0 + 10.0 * 20.0 * 40.0);
    assert_eq!(tuple(c), (16, 24, 10, 0, 1, 0));
    let front = Plane::front(); // normal −Y
    let hole = rod(&front, DVec2::new(20.0, 15.0), 4.0, -40.0, 10.0);
    let (s, c) = run(&body, &hole, BooleanOp::Subtract);
    assert_volume(&s, 6000.0 + 8000.0 - PI * 16.0 * 10.0);
    assert_eq!(c.genus, 1);
    assert_eq!(c.faces, 11);
}

#[test]
fn inputs_are_not_modified_and_empty_inputs_work() {
    let a = unit_box();
    let b = cylinder(v(5.0, 5.0, -2.0), DVec3::Z, 2.0, 14.0);
    let (a0, b0) = (a.clone(), b.clone());
    let _ = run(&a, &b, BooleanOp::Subtract);
    assert_eq!(a, a0);
    assert_eq!(b, b0);
    let empty = Solid::new();
    assert_eq!(boolean(&a, &empty, BooleanOp::Union).unwrap(), a);
    assert_eq!(boolean(&empty, &a, BooleanOp::Union).unwrap(), a);
    assert_eq!(boolean(&a, &empty, BooleanOp::Subtract).unwrap(), a);
    assert!(
        boolean(&empty, &a, BooleanOp::Subtract)
            .unwrap()
            .faces
            .is_empty()
    );
    assert!(
        boolean(&a, &empty, BooleanOp::Intersect)
            .unwrap()
            .faces
            .is_empty()
    );
}

#[test]
fn plate_pocket_then_hole_from_the_pocket_floor() {
    // The Phase 2 exit criterion, at kernel level.
    let plate = block(
        &Plane::TOP,
        DVec2::new(-50.0, -30.0),
        DVec2::new(50.0, 30.0),
        0.0,
        10.0,
    );
    let top = Plane {
        frame: Frame {
            origin: v(0.0, 0.0, 10.0),
            ..Frame::WORLD
        },
    };
    let pocket = block(
        &top,
        DVec2::new(-20.0, -10.0),
        DVec2::new(20.0, 10.0),
        -4.0,
        0.0,
    );
    let (s, c) = run(&plate, &pocket, BooleanOp::Subtract);
    assert_eq!(tuple(c), (16, 24, 11, 1, 1, 0));
    let floor = Plane {
        frame: Frame {
            origin: v(0.0, 0.0, 6.0),
            ..Frame::WORLD
        },
    };
    // Through all: from the floor to 1 mm past the bottom.
    let hole = rod(&floor, DVec2::ZERO, 5.0, -7.0, 0.0);
    let (s, c) = run(&s, &hole, BooleanOp::Subtract);
    assert_volume(&s, 60000.0 - 800.0 * 4.0 - PI * 25.0 * 6.0);
    assert_eq!(tuple(c), (18, 27, 12, 3, 1, 1));
    // A boss on the bottom face.
    let bottom = Plane::from_origin_normal_x(DVec3::ZERO, -DVec3::Z, DVec3::X).unwrap();
    let boss = rod(&bottom, DVec2::new(30.0, 0.0), 8.0, 0.0, 10.0);
    let (s, c) = run(&s, &boss, BooleanOp::Union);
    assert_volume(
        &s,
        60000.0 - 800.0 * 4.0 - PI * 25.0 * 6.0 + PI * 64.0 * 10.0,
    );
    assert_eq!(c.faces, 14);
    assert_eq!(c.genus, 1);
}

#[test]
fn features_on_a_slanted_cylinder() {
    // A rod cut off at a slant, then drilled: the slanted face is bounded by an ellipse and
    // every hole meets it in another ellipse.
    let rod_ = cylinder(DVec3::ZERO, DVec3::Z, 3.0, 10.0);
    let tilt = Frame {
        origin: v(0.0, 0.0, 6.0),
        rotation: DQuat::from_rotation_y(0.5),
    };
    let cutter = placed(&cuboid(v(-20.0, -20.0, 0.0), v(20.0, 20.0, 30.0)), &tilt);
    let (wedge, _) = run(&rod_, &cutter, BooleanOp::Subtract);
    let wedge_volume = PI * 9.0 * 6.0;
    assert_volume(&wedge, wedge_volume);

    // A coaxial bore: ellipse inside ellipse on the slanted face.
    let bore = cylinder(v(0.0, 0.0, -1.0), DVec3::Z, 1.0, 12.0);
    let (s, c) = run(&wedge, &bore, BooleanOp::Subtract);
    assert_volume(&s, wedge_volume - PI * 6.0);
    assert_eq!(c.genus, 1);
    assert_eq!(tuple(c), (4, 6, 4, 2, 1, 1));
    let ellipses = s
        .edges
        .iter()
        .filter(|e| matches!(e.curve, Curve3::Ellipse(_)))
        .count();
    assert_eq!(ellipses, 2);

    // An off-centre hole: mean height over the hole is the plane's height at its axis.
    let slope = -(0.5_f64).tan();
    let off = cylinder(v(1.0, 0.5, -1.0), DVec3::Z, 1.0, 12.0);
    let (s, c) = run(&wedge, &off, BooleanOp::Subtract);
    assert_volume(&s, wedge_volume - PI * (6.0 + slope * 1.0));
    assert_eq!(c.genus, 1);

    // A hole breaking out through the wall: parallel cylinders meet in two lines, and on
    // the slanted face two ellipses cross (found numerically).
    let edge_hole = cylinder(v(3.0, 0.0, -1.0), DVec3::Z, 1.0, 12.0);
    let (cut, c) = run(&wedge, &edge_hole, BooleanOp::Subtract);
    let (common, _) = run(&wedge, &edge_hole, BooleanOp::Intersect);
    assert_eq!(c.genus, 0);
    assert_eq!(c.shells, 1);
    assert!((volume(&cut) + volume(&common) - wedge_volume).abs() < 1e-9);
    // Lens of the two circles (r = 3 and 1, centres 3 apart), times the mean height there.
    assert!(volume(&common) > 0.0 && volume(&common) < PI * 6.0);

    // A flat milled on the side, parallel to the axis: lines on the wall, an elliptic arc
    // gone from the slanted face.
    let flat = cuboid(v(2.0, -5.0, -1.0), v(5.0, 5.0, 11.0));
    let (s, c) = run(&wedge, &flat, BooleanOp::Subtract);
    let (common, _) = run(&wedge, &flat, BooleanOp::Intersect);
    assert_eq!(c.genus, 0);
    assert_eq!(count_surfaces(&s), (3, 1));
    assert!((volume(&s) + volume(&common) - wedge_volume).abs() < 1e-9);

    // The other piece of the rod, glued back on: the ellipse disappears again.
    let (top, _) = run(&rod_, &cutter, BooleanOp::Intersect);
    let (whole, c) = run(&wedge, &top, BooleanOp::Union);
    assert_volume(&whole, PI * 90.0);
    assert_eq!(tuple(c), (2, 3, 3, 0, 1, 0));
}

#[test]
fn drilling_an_existing_hole_deeper() {
    // The new tool's wall lies on the old hole's wall (coaxial, same radius), with its seam
    // at another angle.
    let a = unit_box();
    let (blind, c) = run(
        &a,
        &cylinder(v(5.0, 5.0, 7.0), DVec3::Z, 2.0, 3.0),
        BooleanOp::Subtract,
    );
    assert_eq!(tuple(c), (10, 15, 8, 1, 1, 0));
    let turned = Frame {
        origin: v(5.0, 5.0, 0.0),
        rotation: DQuat::from_rotation_z(2.0),
    };
    let deeper = placed(&cylinder(v(0.0, 0.0, 4.0), DVec3::Z, 2.0, 6.0), &turned);
    let (s, c) = run(&blind, &deeper, BooleanOp::Subtract);
    assert_volume(&s, 1000.0 - PI * 4.0 * 6.0);
    assert_eq!(c.genus, 0);
    assert_eq!(count_surfaces(&s), (7, 1));
    // All the way through.
    let through = placed(&cylinder(v(0.0, 0.0, -1.0), DVec3::Z, 2.0, 12.0), &turned);
    let (s, c) = run(&blind, &through, BooleanOp::Subtract);
    assert_volume(&s, 1000.0 - PI * 4.0 * 10.0);
    assert_eq!(c.genus, 1);
    assert_eq!(count_surfaces(&s), (6, 1));
    // A counterbore: wider, shallower, coaxial.
    let bore = cylinder(v(5.0, 5.0, 9.0), DVec3::Z, 3.0, 1.0);
    let (s, c) = run(&blind, &bore, BooleanOp::Subtract);
    assert_volume(&s, 1000.0 - PI * 4.0 * 2.0 - PI * 9.0);
    assert_eq!(c.genus, 0);
    assert_eq!(count_surfaces(&s), (8, 2));
    // Plugging the hole flush gives the box back.
    let plug = placed(&cylinder(v(0.0, 0.0, 7.0), DVec3::Z, 2.0, 3.0), &turned);
    let (s, c) = run(&blind, &plug, BooleanOp::Union);
    assert_volume(&s, 1000.0);
    assert_eq!(tuple(c), (8, 12, 6, 0, 1, 0));
    // A plug that sticks out becomes a pin.
    let pin = placed(&cylinder(v(0.0, 0.0, 7.0), DVec3::Z, 2.0, 6.0), &turned);
    let (s, c) = run(&blind, &pin, BooleanOp::Union);
    assert_volume(&s, 1000.0 + PI * 4.0 * 3.0);
    assert_eq!(c.genus, 0);
    assert_eq!(count_surfaces(&s), (7, 1));
}

#[test]
fn tools_with_several_shells_and_with_holes() {
    use peet_sketch::region::find_regions;
    let plate = cuboid(DVec3::ZERO, v(40.0, 20.0, 5.0));
    // Two separate circles in one sketch: one tool with two shells drills two holes.
    let mut sketch = Sketch::new();
    sketch.add_circle(DVec2::new(10.0, 10.0), 3.0);
    sketch.add_circle(DVec2::new(30.0, 10.0), 4.0);
    let regions = find_regions(&sketch).regions;
    assert_eq!(regions.len(), 2);
    let tool = crate::extrude::extrude(&Plane::TOP, &regions, -1.0, 6.0).unwrap();
    assert_eq!(tool.shells.len(), 2);
    let (s, c) = run(&plate, &tool, BooleanOp::Subtract);
    assert_volume(&s, 4000.0 - PI * (9.0 + 16.0) * 5.0);
    assert_eq!(c.genus, 2);
    assert_eq!(c.faces, 8);
    // Adding them back as pins sticking out of the top.
    let pins = crate::extrude::extrude(&Plane::TOP, &regions, 0.0, 8.0).unwrap();
    let (s, c) = run(&s, &pins, BooleanOp::Union);
    assert_volume(&s, 4000.0 + PI * (9.0 + 16.0) * 3.0);
    assert_eq!(c.genus, 0);
    assert_eq!(c.shells, 1);

    // A ring (a region with a hole): its inner wall is a reversed cylinder face.
    let mut sketch = Sketch::new();
    sketch.add_circle(DVec2::new(20.0, 10.0), 8.0);
    sketch.add_circle(DVec2::new(20.0, 10.0), 5.0);
    let ring = |from: f64, to: f64| prism(&Plane::TOP, &sketch, DVec2::new(26.5, 10.0), from, to);
    let ring_area = PI * (64.0 - 25.0);
    assert_volume(&ring(0.0, 1.0), ring_area);
    // An annular groove in the top face.
    let (s, c) = run(&plate, &ring(3.0, 5.0), BooleanOp::Subtract);
    assert_volume(&s, 4000.0 - ring_area * 2.0);
    assert_eq!(c.genus, 0);
    assert_eq!(
        c.faces,
        6 + 3 + 1,
        "groove floor, two walls, and the island's top"
    );
    // Cut right through: the middle falls out as a separate body.
    let (s, c) = run(&plate, &ring(-1.0, 6.0), BooleanOp::Subtract);
    assert_volume(&s, 4000.0 - ring_area * 5.0);
    assert_eq!(c.shells, 2);
    // An annular boss on the top face, then a pin in its middle of the same radius as
    // the bore: the bore's wall disappears.
    let (boss, c) = run(&plate, &ring(5.0, 9.0), BooleanOp::Union);
    assert_volume(&boss, 4000.0 + ring_area * 4.0);
    assert_eq!(c.genus, 0);
    let pin = rod(&Plane::TOP, DVec2::new(20.0, 10.0), 5.0, 5.0, 9.0);
    let (s, c) = run(&boss, &pin, BooleanOp::Union);
    assert_volume(&s, 4000.0 + PI * 64.0 * 4.0);
    assert_eq!(c.faces, 8);
    // The ring and a box through its side.
    let bar = cuboid(v(0.0, 8.0, 5.0), v(40.0, 12.0, 7.0));
    let (s, c) = run(&ring(5.0, 9.0), &bar, BooleanOp::Subtract);
    let (common, _) = run(&ring(5.0, 9.0), &bar, BooleanOp::Intersect);
    assert!((volume(&s) + volume(&common) - ring_area * 4.0).abs() < 1e-9);
    assert_eq!(c.shells, 1);
    assert_eq!(c.genus, 1);
}

#[test]
fn nearly_coincident_faces_are_coincident() {
    // Within tolerance of the top face: treated as starting on it, no sliver.
    let a = unit_box();
    let b = cuboid(v(3.0, 3.0, 6.0), v(7.0, 7.0, 10.0 + 2e-7));
    let (s, c) = run(&a, &b, BooleanOp::Subtract);
    assert_eq!(tuple(c), (16, 24, 11, 1, 1, 0));
    assert!((volume(&s) - (1000.0 - 64.0)).abs() < 1e-4);
    let b = cuboid(v(10.0 - 3e-7, 3.0, 3.0), v(15.0, 7.0, 7.0));
    let (s, c) = run(&a, &b, BooleanOp::Union);
    assert_eq!(tuple(c), (16, 24, 11, 1, 1, 0));
    assert!((volume(&s) - 1080.0).abs() < 1e-4);
}

#[test]
fn twenty_face_body_minus_a_box_is_fast() {
    // A plate with a pocket and six holes: 6 + 5 + 6 + ... faces.
    let mut body = cuboid(DVec3::ZERO, v(100.0, 60.0, 10.0));
    let pocket = cuboid(v(10.0, 10.0, 6.0), v(50.0, 50.0, 10.0));
    body = run(&body, &pocket, BooleanOp::Subtract).0;
    for i in 0..3 {
        for j in 0..3 {
            let at = v(60.0 + 12.0 * f64::from(i), 12.0 + 18.0 * f64::from(j), -1.0);
            body = run(
                &body,
                &cylinder(at, DVec3::Z, 4.0, 12.0),
                BooleanOp::Subtract,
            )
            .0;
        }
    }
    assert_eq!(body.faces.len(), 20);
    // The tool: a slot across the pocket and two of the holes.
    let tool = cuboid(v(30.0, 25.0, 4.0), v(90.0, 35.0, 12.0));
    let runs = 30;
    let mut best = std::time::Duration::MAX;
    let mut result = None;
    for _ in 0..runs {
        let start = std::time::Instant::now();
        let r = boolean(&body, &tool, BooleanOp::Subtract).unwrap();
        best = best.min(start.elapsed());
        result = Some(r);
    }
    let result = result.unwrap();
    assert_valid(&result);
    println!(
        "20-face body − 6-face tool: best of {runs} runs {:.3} ms ({} faces)",
        best.as_secs_f64() * 1e3,
        result.faces.len()
    );
    // The budget is for optimised builds.
    if !cfg!(debug_assertions) {
        assert!(best.as_secs_f64() < 0.010, "boolean took {best:?}");
    }
}

// ---- Randomized ----

mod random {
    use super::*;
    use proptest::prelude::*;

    /// Cases per property: a quick run by default, `PEET_BOOLEAN_CASES=50000` to hunt.
    fn cases() -> u32 {
        std::env::var("PEET_BOOLEAN_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(200)
    }

    /// A box with corners on a coarse grid, so coplanar faces, shared edges and touching
    /// corners come up all the time.
    #[derive(Clone, Copy, Debug)]
    struct GridBox {
        min: [i32; 3],
        max: [i32; 3],
    }

    impl GridBox {
        fn solid(&self) -> Solid {
            let f = |a: [i32; 3]| v(f64::from(a[0]), f64::from(a[1]), f64::from(a[2]));
            cuboid(f(self.min), f(self.max))
        }

        fn volume(&self) -> f64 {
            (0..3)
                .map(|k| f64::from(self.max[k] - self.min[k]))
                .product()
        }

        /// The common part, if it has volume.
        fn meet(&self, o: &Self) -> Option<Self> {
            let mut r = *self;
            for k in 0..3 {
                r.min[k] = self.min[k].max(o.min[k]);
                r.max[k] = self.max[k].min(o.max[k]);
                if r.min[k] >= r.max[k] {
                    return None;
                }
            }
            Some(r)
        }
    }

    fn overlap(a: &GridBox, b: &GridBox) -> f64 {
        a.meet(b).map_or(0.0, |m| m.volume())
    }

    fn overlap3(a: &GridBox, b: &GridBox, c: &GridBox) -> f64 {
        a.meet(b)
            .and_then(|m| m.meet(c))
            .map_or(0.0, |m| m.volume())
    }

    fn grid_box(size: i32) -> impl Strategy<Value = GridBox> {
        proptest::array::uniform3((0..size, 1..=size)).prop_map(move |axes| {
            let mut b = GridBox {
                min: [0; 3],
                max: [0; 3],
            };
            for (k, (lo, len)) in axes.into_iter().enumerate() {
                b.min[k] = lo;
                b.max[k] = (lo + len).min(size + 1);
            }
            b
        })
    }

    #[derive(Clone, Copy, Debug)]
    enum Profile {
        Rect([i32; 2], [i32; 2]),
        Circle([i32; 2], i32),
        Slot([i32; 2], [i32; 2], i32),
    }

    /// A sketch feature: a profile on a half-unit grid on one of the principal planes,
    /// extruded between two whole offsets.
    #[derive(Clone, Copy, Debug)]
    struct Feature {
        add: bool,
        plane: usize,
        profile: Profile,
        range: (i32, i32),
    }

    impl Feature {
        fn solid(&self) -> Option<Solid> {
            let h = |a: [i32; 2]| DVec2::new(f64::from(a[0]), f64::from(a[1])) * 0.5;
            let mut sketch = Sketch::new();
            let pick = match self.profile {
                Profile::Rect(a, b) => {
                    if a[0] == b[0] || a[1] == b[1] {
                        return None;
                    }
                    peet_sketch::shapes::rectangle(&mut sketch, h(a), h(b));
                    0.5 * (h(a) + h(b))
                }
                Profile::Circle(c, r) => {
                    sketch.add_circle(h(c), f64::from(r) * 0.5);
                    h(c) + DVec2::new(0.11, 0.07) * f64::from(r)
                }
                Profile::Slot(a, b, r) => {
                    if a == b {
                        return None;
                    }
                    peet_sketch::shapes::slot(&mut sketch, h(a), h(b), f64::from(r) * 0.5);
                    h(a) + DVec2::new(0.11, 0.07) * f64::from(r)
                }
            };
            let (lo, hi) = (
                f64::from(self.range.0),
                f64::from(self.range.0 + self.range.1),
            );
            let (plane, from, to) = match self.plane {
                0 => (Plane::TOP, lo, hi),
                1 => (Plane::right(), lo, hi),
                // Front faces −Y: offsets along its normal run backwards in y.
                _ => (Plane::front(), -hi, -lo),
            };
            Some(prism(&plane, &sketch, pick, from, to))
        }
    }

    fn feature() -> impl Strategy<Value = Feature> {
        let point = || proptest::array::uniform2(0..=8i32);
        let profile = prop_oneof![
            (point(), point()).prop_map(|(a, b)| Profile::Rect(a, b)),
            (point(), 1..=3i32).prop_map(|(c, r)| Profile::Circle(c, r)),
            (point(), point(), 1..=2i32).prop_map(|(a, b, r)| Profile::Slot(a, b, r)),
        ];
        (any::<bool>(), 0..3usize, profile, (-1..4i32, 1..=5i32)).prop_map(
            |(add, plane, profile, range)| Feature {
                add,
                plane,
                profile,
                range,
            },
        )
    }

    /// Complaints of `validate` that random inputs are allowed to provoke:
    /// - slivers: a face or shell too thin to have an area or volume within tolerance
    ///   (bodies grazing each other by microns);
    /// - a void touching the outer shell (a hole tangent to the outside from within):
    ///   `validate` recognises voids by bounding boxes without tolerance, and rounding in
    ///   the circle samples can put such a void a hair outside.
    fn excusable(message: &str) -> bool {
        message.contains("encloses no") || message.contains("inside out")
    }

    /// Whether `s` is valid. Panics if it is invalid for a reason that isn't excusable.
    #[track_caller]
    fn valid_or_excused(s: &Solid) -> bool {
        match crate::validate::validate(s) {
            Ok(_) => true,
            Err(problems) if problems.iter().all(|p| excusable(&p.message)) => false,
            Err(problems) => panic!("invalid solid: {problems:#?}"),
        }
    }

    /// The valid result of a boolean, or `None` when the only complaint is excusable.
    /// Anything else wrong panics.
    #[track_caller]
    fn unless_sliver(a: &Solid, b: &Solid, op: BooleanOp) -> Option<Solid> {
        let s = match boolean(a, b, op) {
            Ok(s) => s,
            Err(KernelError::InvalidResult(m)) if excusable(&m) => return None,
            Err(e) => panic!("{op:?} failed: {e}"),
        };
        valid_or_excused(&s).then_some(s)
    }

    /// An add or cut feature: a grid box, or a cylinder on a half-unit grid along an axis.
    #[derive(Clone, Copy, Debug)]
    struct Tool {
        add: bool,
        shape: GridBox,
        /// `Some((axis, radius in half units))` for a cylinder centred in `shape`'s footprint.
        round: Option<(usize, i32)>,
    }

    impl Tool {
        fn solid(&self) -> Solid {
            let Some((axis, radius)) = self.round else {
                return self.shape.solid();
            };
            let lo = self.shape.min.map(f64::from);
            let hi = self.shape.max.map(f64::from);
            let mut base = [0.0; 3];
            for k in 0..3 {
                base[k] = if k == axis {
                    lo[k]
                } else {
                    0.5 * (lo[k] + hi[k])
                };
            }
            let mut dir = DVec3::ZERO;
            dir[axis] = 1.0;
            cylinder(
                DVec3::from_array(base),
                dir,
                f64::from(radius) * 0.5,
                hi[axis] - lo[axis],
            )
        }
    }

    fn tool() -> impl Strategy<Value = Tool> {
        (
            any::<bool>(),
            grid_box(5),
            proptest::option::of((0..3usize, 1..=3i32)),
        )
            .prop_map(|(add, shape, round)| Tool { add, shape, round })
    }

    #[track_caller]
    fn checked(a: &Solid, b: &Solid, op: BooleanOp, expected: f64) -> Solid {
        let (s, _) = run(a, b, op);
        let got = volume(&s);
        assert!(
            (got - expected).abs() <= 1e-9 * expected.abs().max(1.0),
            "{op:?}: volume {got} instead of {expected}"
        );
        assert_eq!(s.faces.is_empty(), expected == 0.0);
        s
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(cases()))]

        #[test]
        fn two_boxes(a in grid_box(4), b in grid_box(4)) {
            let (sa, sb) = (a.solid(), b.solid());
            let common = overlap(&a, &b);
            checked(&sa, &sb, BooleanOp::Union, a.volume() + b.volume() - common);
            checked(&sa, &sb, BooleanOp::Subtract, a.volume() - common);
            checked(&sa, &sb, BooleanOp::Intersect, common);
        }

        #[test]
        fn three_boxes(a in grid_box(4), b in grid_box(4), c in grid_box(4)) {
            let (sa, sb, sc) = (a.solid(), b.solid(), c.solid());
            let (ab, ac, bc) = (overlap(&a, &b), overlap(&a, &c), overlap(&b, &c));
            let abc = overlap3(&a, &b, &c);
            let (va, vb, vc) = (a.volume(), b.volume(), c.volume());
            // (A ∪ B) op C
            let u = checked(&sa, &sb, BooleanOp::Union, va + vb - ab);
            let u_and_c = ac + bc - abc;
            checked(&u, &sc, BooleanOp::Subtract, va + vb - ab - u_and_c);
            checked(&u, &sc, BooleanOp::Intersect, u_and_c);
            checked(&u, &sc, BooleanOp::Union, va + vb - ab + vc - u_and_c);
            // (A − B) op C
            let d = checked(&sa, &sb, BooleanOp::Subtract, va - ab);
            let d_and_c = ac - abc;
            checked(&d, &sc, BooleanOp::Subtract, va - ab - d_and_c);
            checked(&d, &sc, BooleanOp::Union, va - ab + vc - d_and_c);
            checked(&d, &sc, BooleanOp::Intersect, d_and_c);
            // C − (A − B): the tool has reversed pieces and voids.
            checked(&sc, &d, BooleanOp::Subtract, vc - d_and_c);
        }

        #[test]
        fn box_and_cylinder(
            a in grid_box(4),
            center in (0..=8i32, 0..=8i32),
            radius in 1..=3i32,
            z in (0..4i32, 1..=4i32),
            axis in 0..3usize,
        ) {
            // Centres on a half-unit grid and whole or half radii: tangent walls, walls
            // through edges and corners, and caps in face planes are all common.
            let sa = a.solid();
            let r = f64::from(radius) * 0.5;
            let (cx, cy) = (f64::from(center.0) * 0.5, f64::from(center.1) * 0.5);
            let (base, dir) = match axis {
                0 => (v(f64::from(z.0), cx, cy), DVec3::X),
                1 => (v(cx, f64::from(z.0), cy), DVec3::Y),
                _ => (v(cx, cy, f64::from(z.0)), DVec3::Z),
            };
            let height = f64::from(z.1);
            let sb = cylinder(base, dir, r, height);
            let (va, vb) = (a.volume(), PI * r * r * height);
            let (i, _) = run(&sa, &sb, BooleanOp::Intersect);
            let (u, _) = run(&sa, &sb, BooleanOp::Union);
            let (e, _) = run(&sb, &sa, BooleanOp::Subtract);
            let vi = volume(&i);
            prop_assert!(vi >= -1e-9 && vi <= va.min(vb) + 1e-9);
            prop_assert!((volume(&u) - (va + vb - vi)).abs() < 1e-8, "union {} vs {}", volume(&u), va + vb - vi);
            // A cylinder wholly inside the box but touching its faces leaves a void that
            // touches the outer shell. `validate` decides what is a void by bounding
            // boxes without tolerance, and rounding in the circle samples can put the
            // void a hair outside: skip that one configuration.
            let bb = sb.bounds();
            let eps = 1e-9;
            let inside = (0..3).all(|k| {
                bb.min[k] >= f64::from(a.min[k]) - eps && bb.max[k] <= f64::from(a.max[k]) + eps
            });
            let strictly = (0..3).all(|k| {
                bb.min[k] > f64::from(a.min[k]) + eps && bb.max[k] < f64::from(a.max[k]) - eps
            });
            if inside && !strictly {
                return Ok(());
            }
            let (d, _) = run(&sa, &sb, BooleanOp::Subtract);
            prop_assert!((volume(&d) - (va - vi)).abs() < 1e-8, "a − b {} vs {}", volume(&d), va - vi);
            prop_assert!((volume(&e) - (vb - vi)).abs() < 1e-8, "b − a {} vs {}", volume(&e), vb - vi);
        }

        #[test]
        fn feature_sequences(tools in proptest::collection::vec(tool(), 1..5)) {
            // A block, then a few add / cut features in different directions. Every step
            // must give a valid solid whose volume agrees with the intersection's, or say
            // that crossing cylinders are not supported.
            let mut body = cuboid(DVec3::ZERO, DVec3::splat(4.0));
            for t in &tools {
                let solid = t.solid();
                let op = if t.add { BooleanOp::Union } else { BooleanOp::Subtract };
                let (next, common) = match (boolean(&body, &solid, op), boolean(&body, &solid, BooleanOp::Intersect)) {
                    (Ok(n), Ok(c)) => (n, c),
                    (Err(KernelError::Unsupported(_)), _) | (_, Err(KernelError::Unsupported(_))) => break,
                    (Err(KernelError::InvalidResult(m)), _) | (_, Err(KernelError::InvalidResult(m)))
                        if excusable(&m) => break,
                    (Err(e), _) | (_, Err(e)) => panic!("{op:?} with {t:?} failed: {e}"),
                };
                if !(valid_or_excused(&next) && valid_or_excused(&common)) {
                    break;
                }
                let (vb, vt, vc) = (volume(&body), volume(&solid), volume(&common));
                let expected = if t.add { vb + vt - vc } else { vb - vc };
                prop_assert!(
                    (volume(&next) - expected).abs() < 1e-7,
                    "{op:?} with {t:?}: volume {} instead of {expected}", volume(&next)
                );
                if next.faces.is_empty() {
                    break;
                }
                body = next;
            }
        }

        #[test]
        fn tilted_box_and_cylinder(
            angle in 0.05..1.0f64,
            spin in 0.0..std::f64::consts::TAU,
            height in 2.0..8.0f64,
            offset in -0.9..0.9f64,
        ) {
            // A plane at a random tilt through a cylinder: elliptical cuts with exact volumes
            // as long as the plane stays clear of the caps.
            let a = cylinder(DVec3::ZERO, DVec3::Z, 1.0, 10.0);
            prop_assume!(height - angle.tan() > 0.2 && height + angle.tan() < 9.8);
            let tilt = Frame {
                origin: v(0.0, 0.0, height),
                rotation: DQuat::from_rotation_z(spin) * DQuat::from_rotation_y(angle),
            };
            let b = placed(&cuboid(v(-30.0 + offset, -30.0, 0.0), v(30.0, 30.0, 40.0)), &tilt);
            let (low, c) = run(&a, &b, BooleanOp::Subtract);
            prop_assert_eq!(tuple(c), (2, 3, 3, 0, 1, 0));
            prop_assert!((volume(&low) - PI * height).abs() < 1e-9, "{}", volume(&low));
            let (high, c) = run(&a, &b, BooleanOp::Intersect);
            prop_assert_eq!(tuple(c), (2, 3, 3, 0, 1, 0));
            prop_assert!((volume(&high) - PI * (10.0 - height)).abs() < 1e-9);
            let (all, _) = run(&low, &high, BooleanOp::Union);
            prop_assert!((volume(&all) - PI * 10.0).abs() < 1e-9);
        }

        #[test]
        fn rotated_boxes(
            angle in 0.0..3.2f64,
            shift in (-3.0..3.0f64, -3.0..3.0f64, -3.0..3.0f64),
            size in (0.5..4.0f64, 0.5..4.0f64, 0.5..4.0f64),
        ) {
            // General position: nothing is coplanar, everything crosses transversally.
            let a = cuboid(v(-2.0, -2.0, -2.0), v(2.0, 2.0, 2.0));
            let place = Frame {
                origin: v(shift.0, shift.1, shift.2),
                rotation: DQuat::from_euler(peet_math::EulerRot::XYZ, angle, 0.7 * angle + 0.3, 0.4),
            };
            let b = placed(&cuboid(DVec3::ZERO, v(size.0, size.1, size.2)), &place);
            let (va, vb) = (64.0, size.0 * size.1 * size.2);
            // A corner within a few tolerances of the other box's face is beyond what
            // a tolerance-based kernel can decide consistently: there, any clean
            // "invalid result" error is acceptable (a corrupt solid or a panic is not).
            let near = |s: &Solid, t: &Solid| {
                s.vertices.iter().any(|vx| {
                    t.faces
                        .iter()
                        .any(|f| f.surface.signed_distance(vx.point).abs() < 1e-4)
                })
            };
            if near(&a, &b) || near(&b, &a) {
                for (x, y, op) in [
                    (&a, &b, BooleanOp::Intersect),
                    (&a, &b, BooleanOp::Union),
                    (&a, &b, BooleanOp::Subtract),
                    (&b, &a, BooleanOp::Subtract),
                ] {
                    match boolean(x, y, op) {
                        Ok(s) => prop_assert!(crate::validate::validate(&s).is_ok()),
                        Err(KernelError::InvalidResult(_)) => {}
                        Err(e) => panic!("{op:?} failed: {e}"),
                    }
                }
                return Ok(());
            }
            let (Some(i), Some(u), Some(d), Some(e)) = (
                unless_sliver(&a, &b, BooleanOp::Intersect),
                unless_sliver(&a, &b, BooleanOp::Union),
                unless_sliver(&a, &b, BooleanOp::Subtract),
                unless_sliver(&b, &a, BooleanOp::Subtract),
            ) else {
                return Ok(());
            };
            let vi = volume(&i);
            // An edge passing within tolerance of another is snapped onto it, which moves
            // a face by up to `LINEAR`: volumes agree to about that times the face's area.
            let slack = 100.0 * peet_math::tolerance::LINEAR;
            prop_assert!((volume(&u) - (va + vb - vi)).abs() < slack);
            prop_assert!((volume(&d) - (va - vi)).abs() < slack);
            prop_assert!((volume(&e) - (vb - vi)).abs() < slack);
        }

        #[test]
        fn extruded_features(features in proptest::collection::vec(feature(), 1..4)) {
            // The same, with tools made by the extruder from sketches on the three
            // principal planes: rectangles, circles and slots (whose flat sides are
            // tangent to their round ends), so arcs, partial cylinders and seams all occur.
            let mut body = block(&Plane::TOP, DVec2::ZERO, DVec2::splat(4.0), 0.0, 4.0);
            for f in &features {
                let Some(solid) = f.solid() else { break };
                let op = if f.add { BooleanOp::Union } else { BooleanOp::Subtract };
                let (next, common) = match (boolean(&body, &solid, op), boolean(&body, &solid, BooleanOp::Intersect)) {
                    (Ok(n), Ok(c)) => (n, c),
                    (Err(KernelError::Unsupported(_)), _) | (_, Err(KernelError::Unsupported(_))) => break,
                    (Err(KernelError::InvalidResult(m)), _) | (_, Err(KernelError::InvalidResult(m)))
                        if excusable(&m) => break,
                    (Err(e), _) | (_, Err(e)) => panic!("{op:?} with {f:?} failed: {e}"),
                };
                if !(valid_or_excused(&next) && valid_or_excused(&common)) {
                    break;
                }
                let (vb, vt, vc) = (volume(&body), volume(&solid), volume(&common));
                let expected = if f.add { vb + vt - vc } else { vb - vc };
                prop_assert!(
                    (volume(&next) - expected).abs() < 1e-7,
                    "{op:?} with {f:?}: volume {} instead of {expected}", volume(&next)
                );
                if next.faces.is_empty() {
                    break;
                }
                body = next;
            }
        }
    }
}
