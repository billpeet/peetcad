//! Revolve tests: topology counts, validity, exact areas and volumes, meshes, and booleans
//! with the surfaces revolving makes.

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

use peet_math::{DQuat, DVec2, DVec3, Frame, Plane};
use peet_sketch::Sketch;
use peet_sketch::region::{Region, find_regions};

use super::*;
use crate::boolean::{BooleanOp, boolean};
use crate::geom::Surface;
use crate::tessellate::{SolidMesh, tessellate};
use crate::topo::test_shapes::cuboid;
use crate::validate::{Counts, assert_valid, measure};

/// The sketch's Y axis: the world Z axis for sketches on the front plane.
const Y_AXIS: RevolveAxis = RevolveAxis {
    origin: DVec2::ZERO,
    dir: DVec2::Y,
};

fn v2(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

fn v3(x: f64, y: f64, z: f64) -> DVec3 {
    DVec3::new(x, y, z)
}

/// A closed polygon.
fn polygon(points: &[DVec2]) -> Sketch {
    let mut s = Sketch::new();
    for i in 0..points.len() {
        s.add_line(points[i], points[(i + 1) % points.len()]);
    }
    s
}

fn region_at(s: &Sketch, at: DVec2) -> Vec<Region> {
    let profile = find_regions(s);
    let i = profile.region_at(at).expect("a region at the pick point");
    vec![profile.regions[i].clone()]
}

/// Revolves the region at `at` about the sketch's Y axis, on the front plane (so the
/// axis is the world Z axis and sketch x is world X).
fn turned(s: &Sketch, at: DVec2, from: f64, to: f64) -> Solid {
    match revolve(&Plane::front(), &region_at(s, at), &Y_AXIS, from, to) {
        Ok(solid) => solid,
        Err(e) => panic!("revolve failed: {e}"),
    }
}

fn full(s: &Sketch, at: DVec2) -> Solid {
    turned(s, at, 0.0, TAU)
}

fn tuple(c: Counts) -> (usize, usize, usize, usize, usize, i64) {
    (c.vertices, c.edges, c.faces, c.rings, c.shells, c.genus)
}

#[track_caller]
fn assert_close(got: f64, expected: f64) {
    assert!(
        (got - expected).abs() <= 1e-9 * expected.abs().max(1.0),
        "{got} instead of {expected}"
    );
}

fn area(s: &Solid) -> f64 {
    s.face_ids().map(|f| measure::face_area(s, f)).sum()
}

fn bits(p: DVec3) -> [u64; 3] {
    [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]
}

/// Tessellates and checks the mesh: closed, facing outwards, with the solid's volume and
/// each face's area.
#[track_caller]
fn check_mesh(solid: &Solid, tol: f64) -> SolidMesh {
    let mesh = tessellate(solid, tol).unwrap();
    type Key = ([u64; 3], [u64; 3]);
    let mut uses: HashMap<Key, usize> = HashMap::new();
    for f in &mesh.faces {
        for t in &f.triangles {
            for k in 0..3 {
                let a = bits(f.positions[t[k] as usize]);
                let b = bits(f.positions[t[(k + 1) % 3] as usize]);
                assert_ne!(a, b, "face {}: a triangle without area", f.face.0);
                *uses.entry((a, b)).or_default() += 1;
            }
        }
    }
    for (&(a, b), &n) in &uses {
        assert_eq!(n, 1, "a mesh edge is used {n} times in one direction");
        assert_eq!(uses.get(&(b, a)), Some(&1), "a mesh edge has no opposite");
    }
    let mut volume = 0.0;
    for f in &mesh.faces {
        assert_eq!(f.positions.len(), f.normals.len());
        for t in &f.triangles {
            let [a, b, c] = t.map(|i| f.positions[i as usize]);
            volume += a.dot(b.cross(c)) / 6.0;
            let n = (b - a).cross(c - a);
            for &i in t {
                assert!(
                    n.dot(f.normals[i as usize]) > 0.0,
                    "face {}: a triangle against its normals",
                    f.face.0
                );
            }
        }
        let exact = measure::face_area(solid, f.face);
        assert!(
            (f.area() - exact).abs() <= 0.02 * exact + 1e-6,
            "face {}: mesh area {} vs {exact}",
            f.face.0,
            f.area()
        );
    }
    let exact = measure::volume(solid);
    assert!(
        (volume - exact).abs() <= 0.01 * exact.abs(),
        "mesh volume {volume} vs {exact}"
    );
    mesh
}

#[track_caller]
fn checked(solid: &Solid) -> Counts {
    let c = assert_valid(solid);
    check_mesh(solid, 0.02);
    c
}

// ---- Shapes ----

fn rod(radius: f64, height: f64) -> Solid {
    let s = polygon(&[
        v2(0.0, 0.0),
        v2(radius, 0.0),
        v2(radius, height),
        v2(0.0, height),
    ]);
    full(&s, v2(radius / 2.0, height / 2.0))
}

/// A ball centred on the origin.
fn ball(radius: f64) -> Solid {
    let mut s = Sketch::new();
    s.add_arc(DVec2::ZERO, v2(0.0, -radius), v2(0.0, radius));
    s.add_line(v2(0.0, radius), v2(0.0, -radius));
    full(&s, v2(radius / 2.0, 0.0))
}

/// A cone standing on the plane z = 0, apex up.
fn cone(radius: f64, height: f64) -> Solid {
    let s = polygon(&[v2(0.0, 0.0), v2(radius, 0.0), v2(0.0, height)]);
    full(&s, v2(radius / 4.0, height / 4.0))
}

fn ring(major: f64, minor: f64) -> Solid {
    let mut s = Sketch::new();
    s.add_circle(v2(major, 0.0), minor);
    full(&s, v2(major + 0.3 * minor, 0.2 * minor))
}

#[test]
fn rod_is_a_cylinder() {
    let s = rod(5.0, 10.0);
    assert_eq!(tuple(checked(&s)), (2, 3, 3, 0, 1, 0));
    assert_close(measure::volume(&s), PI * 250.0);
    assert_close(area(&s), 2.0 * PI * 25.0 + 2.0 * PI * 50.0);
    let b = s.bounds();
    assert!(b.min.abs_diff_eq(v3(-5.0, -5.0, 0.0), 1e-9), "{b:?}");
    assert!(b.max.abs_diff_eq(v3(5.0, 5.0, 10.0), 1e-9), "{b:?}");
}

#[test]
fn tube() {
    let s = polygon(&[v2(3.0, 0.0), v2(5.0, 0.0), v2(5.0, 10.0), v2(3.0, 10.0)]);
    let s = full(&s, v2(4.0, 5.0));
    // Two rings (the flat ends) and a hole through the middle.
    assert_eq!(tuple(checked(&s)), (4, 6, 4, 2, 1, 1));
    assert_close(measure::volume(&s), PI * 160.0);
    // The bore faces the axis.
    let bore = s
        .faces
        .iter()
        .filter(|f| matches!(f.surface, Surface::Cylinder(c) if (c.radius - 3.0).abs() < 1e-9))
        .collect::<Vec<_>>();
    assert_eq!(bore.len(), 1);
    assert!(bore[0].reversed);
}

#[test]
fn cone_with_its_apex() {
    let s = cone(5.0, 10.0);
    assert_eq!(tuple(checked(&s)), (2, 2, 2, 0, 1, 0));
    assert_close(measure::volume(&s), PI * 25.0 * 10.0 / 3.0);
    assert_close(area(&s), PI * 25.0 + PI * 5.0 * 125f64.sqrt());
    // Upside down (the apex at the bottom) and as a frustum.
    let s = polygon(&[v2(0.0, 0.0), v2(5.0, 10.0), v2(0.0, 10.0)]);
    let s = full(&s, v2(1.0, 8.0));
    assert_eq!(tuple(checked(&s)), (2, 2, 2, 0, 1, 0));
    assert_close(measure::volume(&s), PI * 25.0 * 10.0 / 3.0);
    let s = polygon(&[v2(0.0, 0.0), v2(6.0, 0.0), v2(3.0, 4.0), v2(0.0, 4.0)]);
    let s = full(&s, v2(1.0, 1.0));
    assert_eq!(tuple(checked(&s)), (2, 3, 3, 0, 1, 0));
    assert_close(measure::volume(&s), PI * 4.0 * (36.0 + 18.0 + 9.0) / 3.0);
}

#[test]
fn double_cone_has_two_apexes() {
    // A groove's shape: the material is between two cones that meet at their tips.
    let s = polygon(&[v2(0.0, 0.0), v2(4.0, -4.0), v2(4.0, 4.0)]);
    let s = full(&s, v2(3.0, 0.0));
    // A vertex for each cone at the tip, where each wraps all the way round.
    assert_eq!(tuple(checked(&s)), (4, 5, 3, 0, 1, 0));
    assert_close(
        measure::volume(&s),
        PI * 16.0 * 8.0 - 2.0 * PI * 16.0 * 4.0 / 3.0,
    );
}

#[test]
fn ball_from_a_half_circle() {
    let s = ball(5.0);
    assert_eq!(tuple(checked(&s)), (2, 1, 1, 0, 1, 0));
    assert_close(measure::volume(&s), 4.0 / 3.0 * PI * 125.0);
    assert_close(area(&s), 4.0 * PI * 25.0);
    let b = s.bounds();
    assert!(b.min.cmple(DVec3::splat(-5.0)).all() && b.max.cmpge(DVec3::splat(5.0)).all());
    assert!(b.size().max_element() < 10.5, "{b:?}");
}

#[test]
fn ring_from_a_circle() {
    let s = ring(10.0, 3.0);
    assert_eq!(tuple(checked(&s)), (1, 2, 1, 0, 1, 1));
    assert_close(measure::volume(&s), 2.0 * PI * PI * 10.0 * 9.0);
    assert_close(area(&s), 4.0 * PI * PI * 30.0);
}

#[test]
fn rounded_profile_mixes_surfaces() {
    // A puck with a rounded rim: flat, torus, cylinder, torus, flat.
    let mut s = Sketch::new();
    s.add_line(v2(0.0, 0.0), v2(8.0, 0.0));
    s.add_arc(v2(8.0, 2.0), v2(8.0, 0.0), v2(10.0, 2.0));
    s.add_line(v2(10.0, 2.0), v2(10.0, 4.0));
    s.add_arc(v2(8.0, 4.0), v2(10.0, 4.0), v2(8.0, 6.0));
    s.add_line(v2(8.0, 6.0), v2(0.0, 6.0));
    s.add_line(v2(0.0, 6.0), v2(0.0, 0.0));
    let solid = full(&s, v2(4.0, 3.0));
    assert_eq!(tuple(checked(&solid)), (4, 7, 5, 0, 1, 0));
    let tori = solid
        .faces
        .iter()
        .filter(|f| matches!(f.surface, Surface::Torus(_)))
        .count();
    assert_eq!(tori, 2);
    // Pappus: each quarter disc's centroid is 4r/3π from its corner.
    let quarter = PI * 4.0 / 4.0;
    let expected = PI * 64.0 * 6.0
        + PI * (100.0 - 64.0) * 2.0
        + 2.0 * TAU * (8.0 + 8.0 / (3.0 * PI)) * quarter;
    assert_close(measure::volume(&solid), expected);
}

#[test]
fn partial_turns() {
    let s = polygon(&[v2(2.0, 0.0), v2(5.0, 0.0), v2(5.0, 4.0), v2(2.0, 4.0)]);
    let quarter = turned(&s, v2(3.0, 2.0), 0.0, FRAC_PI_2);
    assert_eq!(tuple(checked(&quarter)), (8, 12, 6, 0, 1, 0));
    assert_close(measure::volume(&quarter), PI * 21.0 * 4.0 / 4.0);
    // The front plane's normal is −Y and its Y is world Z: a positive turn about +Z
    // takes the profile from +X towards +Y.
    let b = quarter.bounds();
    assert!(b.min.abs_diff_eq(v3(0.0, 0.0, 0.0), 1e-9), "{b:?}");
    assert!(b.max.abs_diff_eq(v3(5.0, 5.0, 4.0), 1e-9), "{b:?}");
    // A profile touching the axis: a wedge of a cylinder, with an edge on the axis.
    let s = polygon(&[v2(0.0, 0.0), v2(5.0, 0.0), v2(5.0, 4.0), v2(0.0, 4.0)]);
    let wedge = turned(&s, v2(3.0, 2.0), -0.5, 1.0);
    assert_eq!(tuple(checked(&wedge)), (6, 9, 5, 0, 1, 0));
    assert_close(measure::volume(&wedge), 25.0 * 1.5 / 2.0 * 4.0);
    // A slice of a ball, and of a ring.
    let mut s = Sketch::new();
    s.add_arc(DVec2::ZERO, v2(0.0, -5.0), v2(0.0, 5.0));
    s.add_line(v2(0.0, 5.0), v2(0.0, -5.0));
    let slice = turned(&s, v2(2.0, 0.0), 0.0, 2.0);
    assert_eq!(tuple(checked(&slice)), (2, 3, 3, 0, 1, 0));
    assert_close(measure::volume(&slice), 4.0 / 3.0 * PI * 125.0 * 2.0 / TAU);
    let mut s = Sketch::new();
    s.add_circle(v2(10.0, 0.0), 3.0);
    let slice = turned(&s, v2(10.5, 0.5), 1.0, 4.0);
    assert_eq!(tuple(checked(&slice)), (2, 3, 3, 0, 1, 0));
    assert_close(measure::volume(&slice), 2.0 * PI * PI * 90.0 * 3.0 / TAU);
}

#[test]
fn profile_on_the_other_side_and_other_planes() {
    // To the left of the axis: the same solid, and a partial turn still goes the way
    // the right-hand rule about the given axis says.
    let s = polygon(&[v2(-2.0, 0.0), v2(-5.0, 0.0), v2(-5.0, 4.0), v2(-2.0, 4.0)]);
    let solid = turned(&s, v2(-3.0, 2.0), 0.0, FRAC_PI_2);
    checked(&solid);
    assert_close(measure::volume(&solid), PI * 21.0);
    let b = solid.bounds();
    assert!(b.min.abs_diff_eq(v3(-5.0, -5.0, 0.0), 1e-9), "{b:?}");
    assert!(b.max.abs_diff_eq(v3(0.0, 0.0, 4.0), 1e-9), "{b:?}");
    // The axis the other way round turns the other way.
    let down = RevolveAxis {
        origin: DVec2::ZERO,
        dir: -DVec2::Y,
    };
    let solid = revolve(
        &Plane::front(),
        &region_at(&s, v2(-3.0, 2.0)),
        &down,
        0.0,
        FRAC_PI_2,
    )
    .unwrap();
    checked(&solid);
    let b = solid.bounds();
    assert!(b.min.abs_diff_eq(v3(-5.0, 0.0, 0.0), 1e-9), "{b:?}");
    // A tilted plane and an axis that is neither at its origin nor along its axes.
    let plane = Plane {
        frame: Frame {
            origin: v3(3.0, -2.0, 7.0),
            rotation: DQuat::from_euler(peet_math::EulerRot::XYZ, 0.4, -0.9, 2.1),
        },
    };
    let s = polygon(&[v2(1.0, 0.0), v2(4.0, 1.0), v2(3.0, 5.0)]);
    let axis = RevolveAxis {
        origin: v2(-1.0, -1.0),
        dir: v2(1.0, -3.0),
    };
    let regions = region_at(&s, v2(3.0, 2.0));
    let solid = revolve(&plane, &regions, &axis, 0.0, TAU).unwrap();
    checked(&solid);
    // Pappus: area times the distance its centroid travels.
    let centroid = (v2(1.0, 0.0) + v2(4.0, 1.0) + v2(3.0, 5.0)) / 3.0;
    let dir = axis.dir.normalize();
    let distance = (centroid - axis.origin).perp_dot(dir).abs();
    let tri_area = 0.5 * (v2(3.0, 1.0)).perp_dot(v2(2.0, 5.0)).abs();
    assert_close(measure::volume(&solid), TAU * distance * tri_area);
}

#[test]
fn hollow_profile_leaves_a_void() {
    let mut s = polygon(&[v2(2.0, 0.0), v2(12.0, 0.0), v2(12.0, 10.0), v2(2.0, 10.0)]);
    s.add_circle(v2(7.0, 5.0), 2.0);
    let profile = find_regions(&s);
    let plate: Vec<Region> = profile
        .regions
        .iter()
        .filter(|r| r.holes.len() == 1)
        .cloned()
        .collect();
    let solid = revolve(&Plane::front(), &plate, &Y_AXIS, 0.0, TAU).unwrap();
    let c = checked(&solid);
    assert_eq!(c.shells, 2);
    assert_close(
        measure::volume(&solid),
        PI * 140.0 * 10.0 - 2.0 * PI * PI * 7.0 * 4.0,
    );
    // A partial turn joins everything through the caps.
    let solid = revolve(&Plane::front(), &plate, &Y_AXIS, 0.0, PI).unwrap();
    let c = checked(&solid);
    assert_eq!((c.shells, c.genus), (1, 1));
    assert_close(
        measure::volume(&solid),
        (PI * 140.0 * 10.0 - 2.0 * PI * PI * 7.0 * 4.0) / 2.0,
    );
}

#[test]
fn faces_are_traced_to_their_edges() {
    let s = polygon(&[v2(2.0, 0.0), v2(5.0, 0.0), v2(5.0, 4.0), v2(2.0, 4.0)]);
    let regions = region_at(&s, v2(3.0, 2.0));
    let (solid, faces) = revolve_traced(&Plane::front(), &regions, &Y_AXIS, 0.0, 1.0).unwrap();
    assert_eq!(faces.len(), solid.faces.len());
    let caps = faces
        .iter()
        .filter(|f| matches!(f, RevolveFace::Start { .. } | RevolveFace::End { .. }))
        .count();
    assert_eq!(caps, 2);
    // The start cap is in the sketch plane, the end cap one radian on.
    for (i, f) in faces.iter().enumerate() {
        let Surface::Plane(p) = solid.faces[i].surface else {
            continue;
        };
        match f {
            RevolveFace::Start { .. } => assert!(p.normal().abs_diff_eq(-DVec3::Y, 1e-12)),
            RevolveFace::End { .. } => {
                assert!(
                    p.normal()
                        .abs_diff_eq(v3(-(1f64.sin()), 1f64.cos(), 0.0), 1e-12)
                );
            }
            RevolveFace::Side { .. } => assert!(p.normal().z.abs() > 0.99),
        }
    }
    let mut edges: Vec<usize> = faces
        .iter()
        .filter_map(|f| match f {
            RevolveFace::Side { edge, .. } => Some(*edge),
            _ => None,
        })
        .collect();
    edges.sort_unstable();
    assert_eq!(edges, vec![0, 1, 2, 3]);
}

#[test]
fn invalid_profiles() {
    let s = polygon(&[v2(-2.0, 0.0), v2(5.0, 0.0), v2(5.0, 4.0), v2(-2.0, 4.0)]);
    let r = region_at(&s, v2(1.0, 2.0));
    let err = revolve(&Plane::front(), &r, &Y_AXIS, 0.0, TAU).unwrap_err();
    assert!(err.to_string().contains("crosses the axis"), "{err}");
    let s = polygon(&[v2(2.0, 0.0), v2(5.0, 0.0), v2(5.0, 4.0)]);
    let r = region_at(&s, v2(4.0, 1.0));
    for (from, to) in [(0.0, 0.0), (1.0, 0.5), (0.0, 7.0), (0.0, f64::NAN)] {
        assert!(matches!(
            revolve(&Plane::front(), &r, &Y_AXIS, from, to),
            Err(KernelError::InvalidInput(_))
        ));
    }
    assert!(revolve(&Plane::front(), &[], &Y_AXIS, 0.0, 1.0).is_err());
    // A circle touching the axis would pinch the torus shut.
    let mut s = Sketch::new();
    s.add_circle(v2(3.0, 0.0), 3.0);
    let r = region_at(&s, v2(3.5, 0.5));
    assert!(matches!(
        revolve(&Plane::front(), &r, &Y_AXIS, 0.0, TAU),
        Err(KernelError::Unsupported(_))
    ));
}

// ---- Booleans with turned shapes ----

#[track_caller]
fn run(a: &Solid, b: &Solid, op: BooleanOp) -> Solid {
    let s = match boolean(a, b, op) {
        Ok(s) => s,
        Err(e) => panic!("{op:?} failed: {e}"),
    };
    checked(&s);
    s
}

#[test]
fn ball_cut_by_planes() {
    let b = ball(5.0);
    let full_volume = 4.0 / 3.0 * PI * 125.0;
    // At the equator (a circle of latitude).
    let top = cuboid(v3(-6.0, -6.0, 0.0), v3(6.0, 6.0, 6.0));
    let s = run(&b, &top, BooleanOp::Subtract);
    assert_close(measure::volume(&s), full_volume / 2.0);
    let s = run(&b, &top, BooleanOp::Intersect);
    assert_close(measure::volume(&s), full_volume / 2.0);
    // Off the equator: a cap of height 2.
    let cap = PI * 4.0 * (15.0 - 2.0) / 3.0;
    let high = cuboid(v3(-6.0, -6.0, 3.0), v3(6.0, 6.0, 6.0));
    let s = run(&b, &high, BooleanOp::Subtract);
    assert_close(measure::volume(&s), full_volume - cap);
    // Through both poles (the plane holds the axis).
    let side = cuboid(v3(-6.0, 0.0, -6.0), v3(6.0, 6.0, 6.0));
    let s = run(&b, &side, BooleanOp::Subtract);
    assert_close(measure::volume(&s), full_volume / 2.0);
    // Parallel to the axis, beside it: a circle that is neither a parallel nor a meridian.
    let beside = cuboid(v3(3.0, -6.0, -6.0), v3(6.0, 6.0, 6.0));
    let s = run(&b, &beside, BooleanOp::Subtract);
    assert_close(measure::volume(&s), full_volume - cap);
    let beside = cuboid(v3(-6.0, -6.0, -6.0), v3(-3.0, 6.0, 6.0));
    let s = run(&b, &beside, BooleanOp::Subtract);
    assert_close(measure::volume(&s), full_volume - cap);
    // An eighth.
    let octant = cuboid(v3(0.0, 0.0, 0.0), v3(6.0, 6.0, 6.0));
    let s = run(&b, &octant, BooleanOp::Intersect);
    assert_close(measure::volume(&s), full_volume / 8.0);
    let s = run(&b, &octant, BooleanOp::Subtract);
    assert_close(measure::volume(&s), full_volume * 7.0 / 8.0);
}

#[test]
fn ball_on_a_block() {
    let b = ball(5.0);
    let block = cuboid(v3(-8.0, -8.0, -10.0), v3(8.0, 8.0, -3.0));
    let s = run(&block, &b, BooleanOp::Union);
    let cap = PI * 4.0 * (15.0 - 2.0) / 3.0;
    assert_close(
        measure::volume(&s),
        16.0 * 16.0 * 7.0 + 4.0 / 3.0 * PI * 125.0 - cap,
    );
    // A dimple in the block.
    let s = run(&block, &b, BooleanOp::Subtract);
    assert_close(measure::volume(&s), 16.0 * 16.0 * 7.0 - cap);
}

#[test]
fn cone_cut_by_planes() {
    let c = cone(5.0, 10.0);
    let volume = PI * 25.0 * 10.0 / 3.0;
    // Square to the axis: a frustum.
    let top = cuboid(v3(-6.0, -6.0, 5.0), v3(6.0, 6.0, 11.0));
    let s = run(&c, &top, BooleanOp::Subtract);
    assert_close(measure::volume(&s), volume * 7.0 / 8.0);
    // Through the apex, holding the axis: two rulings.
    let half = cuboid(v3(-6.0, 0.0, -1.0), v3(6.0, 6.0, 11.0));
    let s = run(&c, &half, BooleanOp::Subtract);
    assert_close(measure::volume(&s), volume / 2.0);
    let quarter = cuboid(v3(0.0, 0.0, -1.0), v3(6.0, 6.0, 11.0));
    let s = run(&c, &quarter, BooleanOp::Intersect);
    assert_close(measure::volume(&s), volume / 4.0);
    // At a slant steeper than the cone's side: an ellipse.
    let frame = Frame {
        origin: v3(0.0, 0.0, 5.0),
        rotation: DQuat::from_rotation_y(0.3),
    };
    let slab = crate::transform::solid(&cuboid(v3(-9.0, -9.0, 0.0), v3(9.0, 9.0, 9.0)), &frame);
    let s = run(&c, &slab, BooleanOp::Subtract);
    assert!(
        s.edges
            .iter()
            .any(|e| matches!(e.curve, Curve3::Ellipse(_)))
    );
    let rest = run(&c, &slab, BooleanOp::Intersect);
    assert_close(measure::volume(&s) + measure::volume(&rest), volume);
    // Along its length: a hyperbola, which the kernel can't hold.
    let beside = cuboid(v3(2.0, -6.0, -1.0), v3(6.0, 6.0, 11.0));
    let err = boolean(&c, &beside, BooleanOp::Subtract).unwrap_err();
    assert!(matches!(err, KernelError::Unsupported(_)), "{err}");
}

#[test]
fn ring_cut_by_planes() {
    let r = ring(10.0, 3.0);
    let volume = 2.0 * PI * PI * 90.0;
    let top = cuboid(v3(-14.0, -14.0, 0.0), v3(14.0, 14.0, 4.0));
    let s = run(&r, &top, BooleanOp::Subtract);
    assert_close(measure::volume(&s), volume / 2.0);
    let side = cuboid(v3(-14.0, 0.0, -4.0), v3(14.0, 14.0, 4.0));
    let s = run(&r, &side, BooleanOp::Subtract);
    assert_close(measure::volume(&s), volume / 2.0);
    let quarter = cuboid(v3(0.0, 0.0, -4.0), v3(14.0, 14.0, 4.0));
    let s = run(&r, &quarter, BooleanOp::Intersect);
    assert_close(measure::volume(&s), volume / 4.0);
    // Resting on a plate, which it only touches along a circle.
    let plate = cuboid(v3(-14.0, -14.0, -5.0), v3(14.0, 14.0, -3.0));
    let s = run(&plate, &r, BooleanOp::Union);
    assert_close(measure::volume(&s), 28.0 * 28.0 * 2.0 + volume);
    // At a slant: not representable.
    let frame = Frame {
        origin: DVec3::ZERO,
        rotation: DQuat::from_rotation_y(0.3),
    };
    let slab = crate::transform::solid(&top, &frame);
    assert!(matches!(
        boolean(&r, &slab, BooleanOp::Subtract),
        Err(KernelError::Unsupported(_))
    ));
}

#[test]
fn shapes_about_one_axis() {
    // A ball drilled along its axis: the "napkin ring".
    let b = ball(5.0);
    let drill = crate::transform::solid(
        &rod(3.0, 12.0),
        &Frame {
            origin: v3(0.0, 0.0, -6.0),
            rotation: DQuat::IDENTITY,
        },
    );
    let s = run(&b, &drill, BooleanOp::Subtract);
    assert_close(measure::volume(&s), 4.0 / 3.0 * PI * 64.0);
    // A rod with a V groove turned into it.
    let shaft = rod(5.0, 10.0);
    let groove = polygon(&[v2(3.0, 5.0), v2(6.0, 3.0), v2(6.0, 7.0)]);
    let groove = full(&groove, v2(5.0, 5.0));
    let s = run(&shaft, &groove, BooleanOp::Subtract);
    // The groove's section inside the rod is a triangle from (3, 5) to x = 5: Pappus.
    let section = 0.5 * 2.0 * (8.0 / 3.0);
    let centroid = 3.0 + 2.0 * 2.0 / 3.0;
    assert_close(measure::volume(&s), PI * 250.0 - TAU * centroid * section);
    // A countersink: a cone into a plate, about the same axis as the hole.
    let plate = cuboid(v3(-10.0, -10.0, 0.0), v3(10.0, 10.0, 4.0));
    let sink = polygon(&[
        v2(0.0, -1.0),
        v2(1.5, -1.0),
        v2(1.5, 2.5),
        v2(3.5, 4.5),
        v2(0.0, 4.5),
    ]);
    let sink = full(&sink, v2(0.5, 1.0));
    let s = run(&plate, &sink, BooleanOp::Subtract);
    let removed = PI * 2.25 * 2.5 + PI * 1.5 * (2.25 + 1.5 * 3.0 + 9.0) / 3.0;
    assert_close(measure::volume(&s), 1600.0 - removed);
    assert!(
        s.faces
            .iter()
            .any(|f| matches!(f.surface, Surface::Cone(_)))
    );
    // A ball on a rod of the same axis: they meet in a circle.
    let s = run(
        &shaft,
        &crate::transform::solid(
            &b,
            &Frame {
                origin: v3(0.0, 0.0, 12.0),
                rotation: DQuat::IDENTITY,
            },
        ),
        BooleanOp::Union,
    );
    // The ball's centre is 2 above the rod's end; the rod's rim is outside the ball.
    let cap = PI * 9.0 * (15.0 - 3.0) / 3.0;
    assert_close(
        measure::volume(&s),
        PI * 250.0 + 4.0 / 3.0 * PI * 125.0 - cap,
    );
}

#[test]
fn turned_shapes_miss_or_contain_each_other() {
    let b = ball(5.0);
    let far = cuboid(v3(20.0, 0.0, 0.0), v3(22.0, 2.0, 2.0));
    let s = run(&b, &far, BooleanOp::Union);
    assert_eq!(assert_valid(&s).shells, 2);
    let inside = cuboid(v3(-1.0, -1.0, -1.0), v3(1.0, 1.0, 1.0));
    let s = run(&b, &inside, BooleanOp::Subtract);
    assert_eq!(assert_valid(&s).shells, 2);
    assert_close(measure::volume(&s), 4.0 / 3.0 * PI * 125.0 - 8.0);
    let s = run(&b, &inside, BooleanOp::Intersect);
    assert_close(measure::volume(&s), 8.0);
    // A ring around a ball: no contact, though their boxes overlap.
    let r = ring(10.0, 3.0);
    let s = run(&b, &r, BooleanOp::Union);
    assert_eq!(assert_valid(&s).shells, 2);
}
