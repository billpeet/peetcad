//! Loft tests: validity, meshes, and volumes against hand calculations.

use std::f64::consts::PI;

use peet_math::{DQuat, DVec2, DVec3, Frame, Plane};
use peet_sketch::Sketch;
use peet_sketch::region::{Region, find_regions};

use super::*;
use crate::tessellate::tessellate;
use crate::validate::{assert_valid, measure};

fn v2(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

fn plane_at(z: f64) -> Plane {
    Plane::from_origin_normal_x(DVec3::new(0.0, 0.0, z), DVec3::Z, DVec3::X).unwrap()
}

fn rectangle(w: f64, h: f64) -> Region {
    let mut s = Sketch::new();
    peet_sketch::shapes::rectangle(&mut s, v2(-w / 2.0, -h / 2.0), v2(w / 2.0, h / 2.0));
    find_regions(&s).regions.remove(0)
}

fn circle(r: f64) -> Region {
    let mut s = Sketch::new();
    s.add_circle(DVec2::ZERO, r);
    find_regions(&s).regions.remove(0)
}

fn polygon(points: &[DVec2]) -> Region {
    let mut s = Sketch::new();
    for i in 0..points.len() {
        s.add_line(points[i], points[(i + 1) % points.len()]);
    }
    find_regions(&s).regions.remove(0)
}

#[track_caller]
fn close(got: f64, expected: f64, tol: f64) {
    assert!(
        (got - expected).abs() <= tol * expected.abs().max(1.0),
        "{got} instead of {expected}"
    );
}

/// Lofts and checks the result: valid, meshable, the mesh closed and of the right volume.
#[track_caller]
fn lofted(sections: &[(Plane, Region)]) -> Solid {
    let list: Vec<LoftSection<'_>> = sections
        .iter()
        .map(|(plane, region)| LoftSection { plane, region })
        .collect();
    let (solid, faces) = match loft_traced(&list) {
        Ok(out) => out,
        Err(e) => panic!("loft failed: {e}"),
    };
    assert_valid(&solid);
    assert_eq!(faces.len(), solid.faces.len());
    let mesh = tessellate(&solid, 0.01).unwrap();
    let mut uses = std::collections::HashMap::new();
    let bits = |p: DVec3| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
    let mut volume = 0.0;
    for f in &mesh.faces {
        for t in &f.triangles {
            let [a, b, c] = t.map(|i| f.positions[i as usize]);
            volume += a.dot(b.cross(c)) / 6.0;
            for (p, q) in [(a, b), (b, c), (c, a)] {
                *uses.entry((bits(p), bits(q))).or_insert(0) += 1;
            }
            let n = (b - a).cross(c - a);
            for &i in t {
                assert!(n.dot(f.normals[i as usize]) > 0.0, "face {}", f.face.0);
            }
        }
    }
    for (&(a, b), &count) in &uses {
        assert_eq!(count, 1);
        assert_eq!(uses.get(&(b, a)), Some(&1), "the mesh has a crack");
    }
    let exact = measure::volume(&solid);
    assert!(
        (volume - exact).abs() <= 0.01 * exact,
        "{volume} vs {exact}"
    );
    solid
}

#[test]
fn frustum_between_two_rectangles_is_flat_sided() {
    let s = lofted(&[
        (plane_at(0.0), rectangle(40.0, 30.0)),
        (plane_at(20.0), rectangle(20.0, 15.0)),
    ]);
    assert_eq!(s.faces.len(), 6);
    assert!(
        s.faces
            .iter()
            .all(|f| matches!(f.surface, Surface::Plane(_)))
    );
    // A frustum of similar rectangles.
    let (a1, a2) = (1200.0, 300.0_f64);
    close(
        measure::volume(&s),
        20.0 / 3.0 * (a1 + a2 + (a1 * a2).sqrt()),
        1e-12,
    );
    // The same from the top down, and with the second plane facing the other way.
    let down =
        Plane::from_origin_normal_x(DVec3::new(0.0, 0.0, 20.0), -DVec3::Z, DVec3::X).unwrap();
    let s = lofted(&[
        (down, rectangle(20.0, 15.0)),
        (plane_at(0.0), rectangle(40.0, 30.0)),
    ]);
    close(
        measure::volume(&s),
        20.0 / 3.0 * (a1 + a2 + (a1 * a2).sqrt()),
        1e-12,
    );
}

#[test]
fn twisted_prism_has_ruled_sides() {
    // A square turned by 30° on the way up: the sides are hyperbolic paraboloids.
    let top = Plane {
        frame: Frame {
            origin: DVec3::new(0.0, 0.0, 10.0),
            rotation: DQuat::from_rotation_z(30f64.to_radians()),
        },
    };
    let s = lofted(&[
        (plane_at(0.0), rectangle(10.0, 10.0)),
        (top, rectangle(10.0, 10.0)),
    ]);
    assert_eq!(s.faces.len(), 6);
    let freeform = s
        .faces
        .iter()
        .filter(|f| matches!(f.surface, Surface::Nurbs(_)))
        .count();
    assert_eq!(freeform, 4);
    // The section at height z is the square's corners, each on its way along a straight
    // rail: a square of side 10·|(1 − t) + t·e^{i30°}| . Area 100·(1 − 2t(1 − t)(1 − cos 30°)).
    let k = 1.0 - 30f64.to_radians().cos();
    let expected = 100.0 * 10.0 * (1.0 - 2.0 * k / 6.0);
    close(measure::volume(&s), expected, 1e-9);
    // The four sides are the same shape.
    let areas: Vec<f64> = s
        .face_ids()
        .filter(|&f| matches!(s.face(f).surface, Surface::Nurbs(_)))
        .map(|f| measure::face_area(&s, f))
        .collect();
    for a in &areas {
        close(*a, areas[0], 1e-9);
    }
}

#[test]
fn cone_frustum_between_two_circles() {
    let s = lofted(&[(plane_at(0.0), circle(10.0)), (plane_at(15.0), circle(4.0))]);
    // Four sides (the circles are quartered) and two caps.
    assert_eq!(s.faces.len(), 6);
    close(
        measure::volume(&s),
        PI * 15.0 / 3.0 * (100.0 + 40.0 + 16.0),
        1e-9,
    );
    // The slant side: π (R + r) s.
    let side: f64 = s
        .face_ids()
        .filter(|&f| matches!(s.face(f).surface, Surface::Nurbs(_)))
        .map(|f| measure::face_area(&s, f))
        .sum();
    close(side, PI * 14.0 * (36.0 + 225.0_f64).sqrt(), 1e-9);
    // The caps are still bounded by circles.
    assert_eq!(
        s.edges
            .iter()
            .filter(|e| matches!(e.curve, Curve3::Circle(_)))
            .count(),
        8
    );
}

#[test]
fn round_to_square() {
    // A square duct that becomes round: the circle is cut at the square's corners.
    let s = lofted(&[
        (plane_at(0.0), rectangle(20.0, 20.0)),
        (plane_at(25.0), circle(8.0)),
    ]);
    assert_eq!(s.faces.len(), 6);
    let volume = measure::volume(&s);
    // Between the prism on the circle's inscribed square and the one on the base.
    assert!(volume > 25.0 * 128.0 && volume < 25.0 * 400.0, "{volume}");
    // Symmetric: the centre of gravity is on the axis.
    let m = crate::query::mass_properties(&s).unwrap();
    assert!(
        m.centroid.x.abs() < 1e-6 && m.centroid.y.abs() < 1e-6,
        "{:?}",
        m.centroid
    );
    // Section areas run from 400 to π·64: Simpson with the middle section (each side's
    // ruling halfway) gives the volume of a ruled solid exactly only for prismatoids, so
    // just compare with a fine numeric slice.
    let mesh = tessellate(&s, 0.001).unwrap();
    let mesh_volume: f64 = mesh
        .triangles()
        .iter()
        .map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0)
        .sum();
    close(mesh_volume, volume, 1e-4);
}

#[test]
fn smooth_through_several_profiles() {
    // A vase: four circles of different sizes.
    let s = lofted(&[
        (plane_at(0.0), circle(6.0)),
        (plane_at(10.0), circle(10.0)),
        (plane_at(22.0), circle(5.0)),
        (plane_at(30.0), circle(7.0)),
    ]);
    assert_eq!(s.faces.len(), 6);
    // Its rails are curves through the profiles.
    let rails = s
        .edges
        .iter()
        .filter(|e| matches!(e.curve, Curve3::Nurbs(_)))
        .count();
    assert_eq!(rails, 4);
    // It is a solid of revolution whose radius is the cubic through the four radii at
    // the profiles' parameters: check the surface passes through each profile.
    for (z, r) in [(0.0, 6.0), (10.0, 10.0), (22.0, 5.0), (30.0, 7.0_f64)] {
        for a in [0.3_f64, 1.9, 4.0] {
            let p = DVec3::new(r * a.cos(), r * a.sin(), z);
            let on = s.faces.iter().any(|f| {
                matches!(f.surface, Surface::Nurbs(_)) && f.surface.signed_distance(p).abs() < 1e-7
            });
            assert!(on, "the loft misses its profile at z = {z}");
        }
    }
    let volume = measure::volume(&s);
    assert!(
        volume > PI * 25.0 * 30.0 && volume < PI * 100.0 * 30.0,
        "{volume}"
    );
    // Polygons too: three pentagons, the middle one bigger and turned.
    let pentagon = |r: f64, turn: f64| {
        let pts: Vec<DVec2> = (0..5)
            .map(|k| DVec2::from_angle(turn + f64::from(k) * 1.256_637_061_435_917_2) * r)
            .collect();
        polygon(&pts)
    };
    let s = lofted(&[
        (plane_at(0.0), pentagon(8.0, 0.0)),
        (plane_at(12.0), pentagon(12.0, 0.3)),
        (plane_at(20.0), pentagon(6.0, 0.6)),
    ]);
    assert_eq!(s.faces.len(), 7);
}

#[test]
fn tilted_profiles_and_arcs() {
    // A stadium (two lines, two half circles) to a smaller one on a tilted plane.
    let stadium = |length: f64, r: f64| {
        let mut s = Sketch::new();
        peet_sketch::shapes::slot(&mut s, v2(-length / 2.0, 0.0), v2(length / 2.0, 0.0), r);
        find_regions(&s).regions.remove(0)
    };
    let top = Plane {
        frame: Frame {
            origin: DVec3::new(3.0, 0.0, 18.0),
            rotation: DQuat::from_rotation_y(0.25),
        },
    };
    let s = lofted(&[
        (plane_at(0.0), stadium(20.0, 6.0)),
        (top, stadium(12.0, 4.0)),
    ]);
    assert_eq!(s.faces.len(), 6);
    let volume = measure::volume(&s);
    assert!(volume > 1000.0 && volume < 6000.0, "{volume}");
}

#[test]
fn mismatched_profiles_are_refused() {
    let tri = polygon(&[v2(0.0, 0.0), v2(10.0, 0.0), v2(0.0, 10.0)]);
    let (p0, p1) = (plane_at(0.0), plane_at(10.0));
    let rect = rectangle(10.0, 10.0);
    let err = loft(&[
        LoftSection {
            plane: &p0,
            region: &rect,
        },
        LoftSection {
            plane: &p1,
            region: &tri,
        },
    ])
    .unwrap_err();
    assert!(err.to_string().contains("same number of edges"), "{err}");
    assert!(
        loft(&[LoftSection {
            plane: &p0,
            region: &rect
        }])
        .is_err()
    );
    // Two profiles in one place.
    let err = loft(&[
        LoftSection {
            plane: &p0,
            region: &rect,
        },
        LoftSection {
            plane: &p0,
            region: &rect,
        },
    ])
    .unwrap_err();
    assert!(err.to_string().contains("same place"), "{err}");
    // A profile with a hole.
    let mut s = Sketch::new();
    peet_sketch::shapes::rectangle(&mut s, v2(-10.0, -10.0), v2(10.0, 10.0));
    s.add_circle(DVec2::ZERO, 3.0);
    let holed = find_regions(&s)
        .regions
        .into_iter()
        .find(|r| r.holes.len() == 1)
        .unwrap();
    assert!(matches!(
        loft(&[
            LoftSection {
                plane: &p0,
                region: &holed
            },
            LoftSection {
                plane: &p1,
                region: &rect
            }
        ]),
        Err(KernelError::Unsupported(_))
    ));
}

// ---- Booleans with freeform faces ----

use crate::boolean::{BooleanOp, boolean};
use crate::primitive::cuboid;

fn build(sections: &[(Plane, Region)]) -> Solid {
    let list: Vec<LoftSection<'_>> = sections
        .iter()
        .map(|(plane, region)| LoftSection { plane, region })
        .collect();
    loft(&list).unwrap()
}

/// Runs a boolean and checks the result is valid and meshes to its own volume.
#[track_caller]
fn run(a: &Solid, b: &Solid, op: BooleanOp) -> Solid {
    let s = match boolean(a, b, op) {
        Ok(s) => s,
        Err(e) => panic!("{op:?} failed: {e}"),
    };
    assert_valid(&s);
    let mesh = tessellate(&s, 0.01).unwrap();
    let volume: f64 = mesh
        .triangles()
        .iter()
        .map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0)
        .sum();
    let exact = measure::volume(&s);
    assert!(
        (volume - exact).abs() <= 0.01 * exact,
        "{volume} vs {exact}"
    );
    s
}

#[test]
fn planes_cut_freeform_sides() {
    // The twisted prism, its top half cut away.
    let top = Plane {
        frame: Frame {
            origin: DVec3::new(0.0, 0.0, 10.0),
            rotation: DQuat::from_rotation_z(30f64.to_radians()),
        },
    };
    let prism = build(&[
        (plane_at(0.0), rectangle(10.0, 10.0)),
        (top, rectangle(10.0, 10.0)),
    ]);
    let upper = cuboid(DVec3::new(-20.0, -20.0, 5.0), DVec3::new(20.0, 20.0, 20.0));
    let k = 1.0 - 30f64.to_radians().cos();
    let lower_half = 1000.0 * (0.5 - k / 6.0);
    let s = run(&prism, &upper, BooleanOp::Subtract);
    close(measure::volume(&s), lower_half, 1e-6);
    assert_eq!(s.faces.len(), 6);
    // The cut's outline is four freeform curves... which here are straight lines in
    // space (a plane across a ruled surface along its rulings' direction is not), so
    // just count them.
    let s = run(&prism, &upper, BooleanOp::Intersect);
    close(
        measure::volume(&s),
        measure::volume(&prism) - lower_half,
        1e-6,
    );
    // A slanted cut through two sides and the top.
    let wedge = crate::transform::solid(
        &cuboid(DVec3::new(-30.0, -30.0, 0.0), DVec3::new(30.0, 30.0, 30.0)),
        &Frame {
            origin: DVec3::new(0.0, 0.0, 6.0),
            rotation: DQuat::from_rotation_y(0.5),
        },
    );
    let cut = run(&prism, &wedge, BooleanOp::Subtract);
    let rest = run(&prism, &wedge, BooleanOp::Intersect);
    close(
        measure::volume(&cut) + measure::volume(&rest),
        measure::volume(&prism),
        1e-6,
    );
}

#[test]
fn cone_made_by_lofting_is_cut_like_a_cone() {
    let frustum = build(&[(plane_at(0.0), circle(10.0)), (plane_at(15.0), circle(4.0))]);
    let upper = cuboid(DVec3::new(-20.0, -20.0, 7.0), DVec3::new(20.0, 20.0, 20.0));
    let s = run(&frustum, &upper, BooleanOp::Subtract);
    // Radius 10 − 0.4 z: the part below z = 7.
    let r7 = 10.0 - 0.4 * 7.0;
    close(
        measure::volume(&s),
        PI * 7.0 / 3.0 * (100.0 + 10.0 * r7 + r7 * r7),
        1e-6,
    );
    // Half of it, cut through the axis.
    let side = cuboid(DVec3::new(-20.0, 0.0, -1.0), DVec3::new(20.0, 20.0, 20.0));
    let s = run(&frustum, &side, BooleanOp::Subtract);
    close(measure::volume(&s), measure::volume(&frustum) / 2.0, 1e-6);
    // A block standing on its base, joined on.
    let block = cuboid(DVec3::new(-12.0, -12.0, -5.0), DVec3::new(12.0, 12.0, 0.0));
    let s = run(&block, &frustum, BooleanOp::Union);
    close(
        measure::volume(&s),
        24.0 * 24.0 * 5.0 + measure::volume(&frustum),
        1e-9,
    );
}

#[test]
fn a_hole_drilled_across_a_loft() {
    let duct = build(&[
        (plane_at(0.0), rectangle(20.0, 20.0)),
        (plane_at(25.0), circle(8.0)),
    ]);
    let volume = measure::volume(&duct);
    // A rod along X through the middle: it pierces two freeform sides.
    let mut sk = Sketch::new();
    sk.add_circle(v2(0.0, 12.0), 3.0);
    let rod =
        crate::extrude::extrude(&Plane::right(), &find_regions(&sk).regions, -30.0, 30.0).unwrap();
    let drilled = run(&duct, &rod, BooleanOp::Subtract);
    let plug = run(&duct, &rod, BooleanOp::Intersect);
    close(
        measure::volume(&drilled) + measure::volume(&plug),
        volume,
        1e-6,
    );
    // The plug is a little longer than the duct is wide there on average.
    let v = measure::volume(&plug);
    assert!(v > PI * 9.0 * 16.0 && v < PI * 9.0 * 20.0, "{v}");
    // The hole's wall is still a cylinder; its ends are freeform curves on the sides.
    assert!(
        drilled
            .faces
            .iter()
            .any(|f| matches!(f.surface, Surface::Cylinder(_)) && f.reversed)
    );
    assert!(
        drilled
            .edges
            .iter()
            .any(|e| matches!(e.curve, Curve3::Nurbs(_)))
    );
}

#[test]
fn two_lofts_cross() {
    // A cone narrowing upwards, less a cone widening upwards on the same axis: their
    // sides (freeform, both) cross in a circle.
    let a = build(&[(plane_at(0.0), circle(10.0)), (plane_at(15.0), circle(4.0))]);
    let b = build(&[(plane_at(5.0), circle(3.0)), (plane_at(20.0), circle(8.0))]);
    let s = run(&a, &b, BooleanOp::Subtract);
    // What is removed, slice by slice: the smaller of the two radii, from z = 5 to 15.
    let n = 200_000;
    let removed: f64 = (0..n)
        .map(|i| {
            let z = 5.0 + 10.0 * (f64::from(i) + 0.5) / f64::from(n);
            let (ra, rb) = (10.0 - 0.4 * z, 3.0 + (z - 5.0) / 3.0);
            PI * ra.min(rb).powi(2) * 10.0 / f64::from(n)
        })
        .sum();
    close(measure::volume(&s), measure::volume(&a) - removed, 1e-5);
    let both = run(&a, &b, BooleanOp::Intersect);
    close(measure::volume(&both), removed, 1e-5);
}
