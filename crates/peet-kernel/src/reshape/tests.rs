//! Offset, shell and draft tests: validity, meshes and volumes against hand calculations.

use std::f64::consts::PI;

use peet_math::{DVec2, DVec3, Plane};
use peet_sketch::Sketch;
use peet_sketch::region::find_regions;

use super::*;
use crate::blend::{Blend, blend};
use crate::boolean::boolean;
use crate::extrude::extrude;
use crate::primitive::cuboid;
use crate::tessellate::tessellate;
use crate::validate::{assert_valid, measure};

fn v3(x: f64, y: f64, z: f64) -> DVec3 {
    DVec3::new(x, y, z)
}

#[track_caller]
fn assert_close(got: f64, expected: f64) {
    assert!(
        (got - expected).abs() <= 1e-8 * expected.abs().max(1.0),
        "{got} instead of {expected}"
    );
}

#[track_caller]
fn checked(s: &Solid) -> f64 {
    assert_valid(s);
    let mesh = tessellate(s, 0.02).unwrap();
    let volume: f64 = mesh
        .triangles()
        .iter()
        .map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0)
        .sum();
    let exact = measure::volume(s);
    assert!(
        (volume - exact).abs() <= 0.01 * exact,
        "{volume} vs {exact}"
    );
    exact
}

/// The flat face whose outward normal is `n`.
fn face_facing(s: &Solid, n: DVec3) -> FaceId {
    let found: Vec<FaceId> = s
        .face_ids()
        .filter(|&f| {
            let l = s.face(f).loops[0];
            let c = s.loop_coedges(l)[0];
            let p = s.edge(s.coedge(c).edge).point_at_fraction(0.5);
            matches!(s.face(f).surface, Surface::Plane(_)) && s.face_normal_at(f, p).dot(n) > 0.999
        })
        .collect();
    assert_eq!(found.len(), 1, "one face facing {n}");
    found[0]
}

fn rod(radius: f64, height: f64) -> Solid {
    let mut s = Sketch::new();
    s.add_circle(DVec2::ZERO, radius);
    extrude(&Plane::TOP, &find_regions(&s).regions, 0.0, height).unwrap()
}

#[test]
fn offset_a_block() {
    let s = cuboid(DVec3::ZERO, v3(10.0, 20.0, 30.0));
    let bigger = offset(&s, |_| 1.0).unwrap();
    assert_close(checked(&bigger), 12.0 * 22.0 * 32.0);
    let smaller = offset(&s, |_| -2.0).unwrap();
    assert_close(checked(&smaller), 6.0 * 16.0 * 26.0);
    // One face only.
    let top = face_facing(&s, DVec3::Z);
    let taller = offset(&s, |f| if f == top { 5.0 } else { 0.0 }).unwrap();
    assert_close(checked(&taller), 10.0 * 20.0 * 35.0);
    // Too far: the block would turn inside out.
    let err = offset(&s, |_| -6.0).unwrap_err();
    assert!(err.to_string().contains("can't be moved that far"), "{err}");
}

#[test]
fn offset_round_bodies() {
    let s = rod(5.0, 10.0);
    let bigger = offset(&s, |_| 1.0).unwrap();
    assert_close(checked(&bigger), PI * 36.0 * 12.0);
    let smaller = offset(&s, |_| -1.0).unwrap();
    assert_close(checked(&smaller), PI * 16.0 * 8.0);
    assert!(offset(&s, |_| -5.0).is_err());
    // A plate with a hole: the hole grows as the plate shrinks.
    let mut sk = Sketch::new();
    peet_sketch::shapes::rectangle(&mut sk, DVec2::ZERO, DVec2::new(40.0, 20.0));
    sk.add_circle(DVec2::new(10.0, 10.0), 3.0);
    let plate: Vec<_> = find_regions(&sk)
        .regions
        .into_iter()
        .filter(|r| r.holes.len() == 1)
        .collect();
    let s = extrude(&Plane::TOP, &plate, 0.0, 6.0).unwrap();
    let smaller = offset(&s, |_| -1.0).unwrap();
    assert_close(checked(&smaller), (38.0 * 18.0 - PI * 16.0) * 4.0);
    // A ball.
    let ball = crate::primitive::ball(&peet_math::Frame::WORLD, 5.0).unwrap();
    let smaller = offset(&ball, |_| -2.0).unwrap();
    assert_close(checked(&smaller), 4.0 / 3.0 * PI * 27.0);
}

#[test]
fn shell_a_block() {
    let s = cuboid(DVec3::ZERO, v3(40.0, 30.0, 20.0));
    let top = face_facing(&s, DVec3::Z);
    let tray = shell(&s, &[top], 2.0).unwrap();
    assert_close(checked(&tray.solid), 24000.0 - 36.0 * 26.0 * 18.0);
    // Five walls outside, five inside, and the rim.
    assert_eq!(tray.solid.faces.len(), 11);
    let inner = tray
        .faces
        .iter()
        .filter(|l| l.iter().any(|x| matches!(x, ShellFace::Inner(_))))
        .count();
    assert_eq!(inner, 5);
    // The rim is what is left of the opened face.
    let rim = tray
        .faces
        .iter()
        .position(|l| l == &vec![ShellFace::Outer(top)])
        .unwrap();
    assert_eq!(tray.solid.faces[rim].loops.len(), 2);
    // Two opposite faces open: a tube.
    let bottom = face_facing(&s, -DVec3::Z);
    let tube = shell(&s, &[top, bottom], 2.0).unwrap();
    assert_close(checked(&tube.solid), 24000.0 - 36.0 * 26.0 * 20.0);
    // Nothing open: a closed hollow.
    let hollow = shell(&s, &[], 2.0).unwrap();
    assert_close(checked(&hollow.solid), 24000.0 - 36.0 * 26.0 * 16.0);
    assert_eq!(assert_valid(&hollow.solid).shells, 2);
    // Walls thicker than half the block.
    let err = shell(&s, &[top], 16.0).unwrap_err();
    assert!(err.to_string().contains("too thick"), "{err}");
    assert!(shell(&s, &[top], 0.0).is_err());
}

#[test]
fn shell_round_and_stepped_bodies() {
    // A cup.
    let s = rod(10.0, 30.0);
    let top = face_facing(&s, DVec3::Z);
    let cup = shell(&s, &[top], 1.5).unwrap();
    assert_close(
        checked(&cup.solid),
        PI * 100.0 * 30.0 - PI * 8.5 * 8.5 * 28.5,
    );
    // An L-shaped block: the inside corner turns into an outside one in the hollow.
    let a = cuboid(DVec3::ZERO, v3(40.0, 20.0, 10.0));
    let b = cuboid(v3(0.0, 0.0, 10.0), v3(10.0, 20.0, 30.0));
    let s = boolean(&a, &b, BooleanOp::Union).unwrap();
    let end = face_facing(&s, -DVec3::Y);
    let shelled = shell(&s, &[end], 2.0).unwrap();
    // The section (an L of area 600) less the L offset inwards by 2, 18 deep.
    let inner_section = 36.0 * 6.0 + 6.0 * 20.0;
    assert_close(checked(&shelled.solid), 600.0 * 20.0 - inner_section * 18.0);
    // A block with rounded edges: the fillets inside are tighter by the wall thickness.
    let s = cuboid(DVec3::ZERO, v3(40.0, 30.0, 20.0));
    let uprights: Vec<EdgeId> = s
        .edge_ids()
        .filter(|&e| (s.edge(e).point_at_fraction(0.5).z - 10.0).abs() < 1e-9)
        .collect();
    let rounded = blend(&s, &uprights, Blend::Fillet { radius: 5.0 })
        .unwrap()
        .solid;
    let top = face_facing(&rounded, DVec3::Z);
    let shelled = shell(&rounded, &[top], 2.0).unwrap();
    let outer = 40.0 * 30.0 - (4.0 - PI) * 25.0;
    let inner = 36.0 * 26.0 - (4.0 - PI) * 9.0;
    assert_close(checked(&shelled.solid), outer * 20.0 - inner * 18.0);
    // A wall as thick as the fillet radius would leave no fillet inside.
    assert!(shell(&rounded, &[top], 5.0).is_err());
}

#[test]
fn draft_the_walls_of_a_block() {
    let s = cuboid(DVec3::ZERO, v3(40.0, 30.0, 20.0));
    let walls: Vec<FaceId> = [DVec3::X, -DVec3::X, DVec3::Y, -DVec3::Y]
        .into_iter()
        .map(|n| face_facing(&s, n))
        .collect();
    let angle = 5f64.to_radians();
    // About the bottom: the top shrinks by h tan(a) on every side.
    let drafted = draft(&s, &walls, &Plane::TOP, angle).unwrap();
    let k = 20.0 * angle.tan();
    let (a1, a2) = (40.0 * 30.0, (40.0 - 2.0 * k) * (30.0 - 2.0 * k));
    // A prismatoid: Simpson's rule over the height is exact.
    let mid = (40.0 - k) * (30.0 - k);
    assert_close(checked(&drafted), 20.0 / 6.0 * (a1 + 4.0 * mid + a2));
    assert_eq!(drafted.faces.len(), 6);
    // Outwards with a negative angle.
    let flared = draft(&s, &walls, &Plane::TOP, -angle).unwrap();
    let a2 = (40.0 + 2.0 * k) * (30.0 + 2.0 * k);
    let mid = (40.0 + k) * (30.0 + k);
    assert_close(checked(&flared), 20.0 / 6.0 * (a1 + 4.0 * mid + a2));
    // One wall, about a plane half way up: what it gains below it loses above.
    let half = Plane::from_origin_normal_x(v3(0.0, 0.0, 10.0), DVec3::Z, DVec3::X).unwrap();
    let one = draft(&s, &walls[..1], &half, angle).unwrap();
    assert_close(checked(&one), 24000.0);
    // The top can't be drafted about a plane parallel to it.
    let top = face_facing(&s, DVec3::Z);
    let err = draft(&s, &[top], &Plane::TOP, angle).unwrap_err();
    assert!(err.to_string().contains("parallel"), "{err}");
    assert!(draft(&s, &walls, &Plane::TOP, 2.0).is_err());
    assert!(draft(&s, &[], &Plane::TOP, angle).is_err());
    // So steep that the walls meet below the top.
    let err = draft(&s, &walls, &Plane::TOP, 60f64.to_radians()).unwrap_err();
    assert!(err.to_string().contains("can't be drafted"), "{err}");
}

#[test]
fn draft_round_walls() {
    let angle = 5f64.to_radians();
    // A rod becomes a cone frustum, narrowing upwards from its foot.
    let r = rod(5.0, 10.0);
    let side = r
        .face_ids()
        .find(|&f| matches!(r.face(f).surface, Surface::Cylinder(_)))
        .unwrap();
    let drafted = draft(&r, &[side], &Plane::TOP, angle).unwrap();
    let top = 5.0 - 10.0 * angle.tan();
    assert_close(
        checked(&drafted),
        PI * 10.0 / 3.0 * (25.0 + 5.0 * top + top * top),
    );
    assert!(
        drafted
            .faces
            .iter()
            .any(|f| matches!(f.surface, Surface::Cone(_)))
    );
    // A hole through a plate widens upwards instead.
    let mut sk = Sketch::new();
    peet_sketch::shapes::rectangle(&mut sk, DVec2::ZERO, DVec2::new(40.0, 20.0));
    sk.add_circle(DVec2::new(10.0, 10.0), 3.0);
    let plate: Vec<_> = find_regions(&sk)
        .regions
        .into_iter()
        .filter(|r| r.holes.len() == 1)
        .collect();
    let s = extrude(&Plane::TOP, &plate, 0.0, 6.0).unwrap();
    let hole = s
        .face_ids()
        .find(|&f| matches!(s.face(f).surface, Surface::Cylinder(_)))
        .unwrap();
    let drafted = draft(&s, &[hole], &Plane::TOP, angle).unwrap();
    let wide = 3.0 + 6.0 * angle.tan();
    assert_close(
        checked(&drafted),
        4800.0 - PI * 6.0 / 3.0 * (9.0 + 3.0 * wide + wide * wide),
    );
    // A block with rounded upright edges: the walls and the corners draft together and
    // stay tangent.
    let b = cuboid(DVec3::ZERO, v3(40.0, 30.0, 20.0));
    let uprights: Vec<EdgeId> = b
        .edge_ids()
        .filter(|&e| (b.edge(e).point_at_fraction(0.5).z - 10.0).abs() < 1e-9)
        .collect();
    let rounded = blend(&b, &uprights, Blend::Fillet { radius: 5.0 })
        .unwrap()
        .solid;
    let walls: Vec<FaceId> = rounded
        .face_ids()
        .filter(|&f| {
            let l = rounded.face(f).loops[0];
            let c = rounded.loop_coedges(l)[0];
            let p = rounded.edge(rounded.coedge(c).edge).point_at_fraction(0.5);
            rounded.face_normal_at(f, p).z.abs() < 0.5
        })
        .collect();
    assert_eq!(walls.len(), 8);
    let drafted = draft(&rounded, &walls, &Plane::TOP, angle).unwrap();
    // Every section is the rounded rectangle offset inwards by z tan(a): its area is
    // (w − 2k)(h − 2k) − (4 − π)(r − k)², integrated exactly by Simpson's rule.
    let section = |z: f64| {
        let k = z * angle.tan();
        (40.0 - 2.0 * k) * (30.0 - 2.0 * k) - (4.0 - PI) * (5.0 - k) * (5.0 - k)
    };
    assert_close(
        checked(&drafted),
        20.0 / 6.0 * (section(0.0) + 4.0 * section(10.0) + section(20.0)),
    );
    // An axis across the pull is refused.
    let lying = crate::transform::solid(
        &rod(5.0, 10.0),
        &peet_math::Frame {
            origin: DVec3::ZERO,
            rotation: peet_math::DQuat::from_rotation_x(1.0),
        },
    );
    let side = lying
        .face_ids()
        .find(|&f| matches!(lying.face(f).surface, Surface::Cylinder(_)))
        .unwrap();
    assert!(matches!(
        draft(&lying, &[side], &Plane::TOP, angle),
        Err(KernelError::Unsupported(_))
    ));
}

// ---- Freeform faces ----

use crate::loft::{LoftSection, loft};
use peet_sketch::region::Region;

fn plane_at(z: f64) -> Plane {
    Plane::from_origin_normal_x(v3(0.0, 0.0, z), DVec3::Z, DVec3::X).unwrap()
}

fn rectangle(w: f64, h: f64) -> Region {
    let mut s = Sketch::new();
    peet_sketch::shapes::rectangle(
        &mut s,
        DVec2::new(-w / 2.0, -h / 2.0),
        DVec2::new(w / 2.0, h / 2.0),
    );
    find_regions(&s).regions.remove(0)
}

fn circle(r: f64) -> Region {
    let mut s = Sketch::new();
    s.add_circle(DVec2::ZERO, r);
    find_regions(&s).regions.remove(0)
}

fn lofted(sections: &[(Plane, Region)]) -> Solid {
    let list: Vec<LoftSection<'_>> = sections
        .iter()
        .map(|(plane, region)| LoftSection { plane, region })
        .collect();
    loft(&list).unwrap()
}

/// The volume of a result. (The operations validate what they return; validating again
/// and meshing, as [`checked`] does, is slow with freeform faces, so one test does it.)
fn valid(s: &Solid) -> f64 {
    measure::volume(s)
}

#[track_caller]
fn near(got: f64, expected: f64, relative: f64) {
    assert!(
        (got - expected).abs() <= relative * expected.abs().max(1.0),
        "{got} instead of {expected}"
    );
}

/// The faces whose outward normal is square to Z, more or less: the walls.
fn walls(s: &Solid) -> Vec<FaceId> {
    s.face_ids()
        .filter(|&f| {
            let l = s.face(f).loops[0];
            let c = s.loop_coedges(l)[0];
            let p = s.edge(s.coedge(c).edge).point_at_fraction(0.5);
            s.face_normal_at(f, p).z.abs() < 0.5
        })
        .collect()
}

/// Every inside wall of a shell stands `thickness` behind the face it lines: checked at
/// the points of a mesh of the shell.
#[track_caller]
fn assert_wall(body: &Solid, shelled: &Shelled, thickness: f64) {
    let mut seen = 0;
    for (i, labels) in shelled.faces.iter().enumerate() {
        let [ShellFace::Inner(f)] = labels[..] else {
            continue;
        };
        let (outer, inner) = (&body.face(f).surface, &shelled.solid.faces[i].surface);
        // Points of the inside wall: along its edges, and for a freeform wall all over
        // the part of its surface behind the face.
        let mut points: Vec<DVec3> = Vec::new();
        for &l in &shelled.solid.faces[i].loops {
            for c in shelled.solid.loop_coedges(l) {
                let e = shelled.solid.edge(shelled.solid.coedge(c).edge);
                points.extend((0..=4).map(|k| e.point_at_fraction(f64::from(k) / 4.0)));
            }
        }
        if let (Surface::Nurbs(o), Surface::Nurbs(n)) = (outer, inner) {
            let (lo, hi) = o.domain();
            for a in 0..=10 {
                for b in 0..=10 {
                    let at = DVec2::new(f64::from(a), f64::from(b)) / 10.0;
                    points.push(n.point(lo + (hi - lo) * at));
                }
            }
        }
        for p in points {
            // Only where the point is square to the face itself, not past its edge.
            if let Surface::Nurbs(s) = outer {
                let (lo, hi) = s.domain();
                let uv = s.param(p);
                if uv.cmple(lo).any() || uv.cmpge(hi).any() {
                    continue;
                }
            }
            let d = outer.signed_distance(p).abs();
            assert!(
                (d - thickness).abs() < 2e-6,
                "the wall behind face {} is {d} thick at {p}",
                f.0
            );
            seen += 1;
        }
    }
    assert!(seen > 50, "{seen} points checked");
}

/// π ∫ (a + b z)² dz from `z0` to `z1`: the volume of a cone frustum.
fn frustum(a: f64, b: f64, z0: f64, z1: f64) -> f64 {
    let cube = |z: f64| (a + b * z).powi(3);
    PI * (cube(z1) - cube(z0)) / (3.0 * b)
}

#[test]
fn offset_and_shell_a_lofted_cylinder() {
    // A loft between two equal circles is a cylinder written as four freeform faces.
    let s = lofted(&[
        (plane_at(0.0), circle(10.0)),
        (plane_at(30.0), circle(10.0)),
    ]);
    assert_eq!(
        s.faces
            .iter()
            .filter(|f| matches!(f.surface, Surface::Nurbs(_)))
            .count(),
        4
    );
    let smaller = offset(&s, |_| -1.0).unwrap();
    near(valid(&smaller), PI * 81.0 * 28.0, 1e-8);
    // It is still bounded by circles and straight rails.
    let kinds = |s: &Solid| {
        (
            s.edges
                .iter()
                .filter(|e| matches!(e.curve, Curve3::Circle(_)))
                .count(),
            s.edges
                .iter()
                .filter(|e| matches!(e.curve, Curve3::Line(_)))
                .count(),
        )
    };
    assert_eq!(kinds(&smaller), (8, 4));
    for e in &smaller.edges {
        if let Curve3::Circle(c) = &e.curve {
            assert!((c.radius - 9.0).abs() < 1e-6, "{}", c.radius);
        }
    }
    // A cup.
    let top = face_facing(&s, DVec3::Z);
    let cup = shell(&s, &[top], 1.5).unwrap();
    near(
        valid(&cup.solid),
        PI * 100.0 * 30.0 - PI * 8.5 * 8.5 * 28.5,
        1e-8,
    );
    // Four walls and a floor outside and inside, and the rim.
    assert_eq!(cup.solid.faces.len(), 11);
    let rim = cup
        .faces
        .iter()
        .position(|l| l == &vec![ShellFace::Outer(top)])
        .unwrap();
    assert_eq!(cup.solid.faces[rim].loops.len(), 2);
    assert_wall(&s, &cup, 1.5);
    // A wall as thick as the cylinder's radius.
    let err = shell(&s, &[top], 10.0).unwrap_err();
    assert!(err.to_string().contains("tightest curve"), "{err}");
    assert!(err.to_string().contains("radius ≈ 10.000 mm"), "{err}");
    let err = offset(&s, |_| -10.5).unwrap_err();
    assert!(err.to_string().contains("fold over itself"), "{err}");
}

#[test]
fn offset_and_shell_a_lofted_cone() {
    // Radius 10 − 0.4 z, up to z = 15.
    let s = lofted(&[(plane_at(0.0), circle(10.0)), (plane_at(15.0), circle(4.0))]);
    // The side moves by d square to itself: by d·√(1 + 0.4²) at a given height.
    let slant = (1.0_f64 + 0.16).sqrt();
    // Outwards the sides have to carry on past their ends to meet the caps.
    let bigger = offset(&s, |_| 0.5).unwrap();
    near(
        valid(&bigger),
        frustum(10.0 + 0.5 * slant, -0.4, -0.5, 15.5),
        1e-8,
    );
    // Open at the narrow end: the inside walls carry on up to the opening.
    let top = face_facing(&s, DVec3::Z);
    let whole = frustum(10.0, -0.4, 0.0, 15.0);
    let funnel = shell(&s, &[top], 1.0).unwrap();
    near(
        valid(&funnel.solid),
        whole - frustum(10.0 - slant, -0.4, 1.0, 15.0),
        1e-8,
    );
    assert_wall(&s, &funnel, 1.0);
    // Closed.
    let bottom = face_facing(&s, -DVec3::Z);
    let hollow = shell(&s, &[], 1.0).unwrap();
    near(
        valid(&hollow.solid),
        whole - frustum(10.0 - slant, -0.4, 1.0, 14.0),
        1e-8,
    );
    assert_eq!(assert_valid(&hollow.solid).shells, 2);
    // Both ends open: a tube.
    let tube = shell(&s, &[top, bottom], 1.0).unwrap();
    near(
        valid(&tube.solid),
        whole - frustum(10.0 - slant, -0.4, 0.0, 15.0),
        1e-8,
    );
    // The narrow end has a radius of 4: walls thicker than that would fold there.
    let err = shell(&s, &[top], 4.5).unwrap_err();
    assert!(err.to_string().contains("tightest curve"), "{err}");
}

#[test]
fn shell_a_square_to_round_duct() {
    let s = lofted(&[
        (plane_at(0.0), rectangle(20.0, 20.0)),
        (plane_at(25.0), circle(8.0)),
    ]);
    let top = face_facing(&s, DVec3::Z);
    let whole = measure::volume(&s);
    let shelled = shell(&s, &[top], 1.0).unwrap();
    // It meshes, to its own volume.
    let volume = valid(&shelled.solid);
    let meshed: f64 = tessellate(&shelled.solid, 0.05)
        .unwrap()
        .triangles()
        .iter()
        .map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0)
        .sum();
    near(meshed, volume, 0.01);
    assert_eq!(shelled.solid.faces.len(), 11);
    assert_wall(&s, &shelled, 1.0);
    // The walls' volume is their thickness times an area between the inside's and the
    // outside's.
    let area = |solid: &Solid, inner: bool| -> f64 {
        shelled
            .faces
            .iter()
            .enumerate()
            .filter(|(_, l)| matches!(l[..], [ShellFace::Inner(_)]) == inner)
            .filter(|(_, l)| l[..] != [ShellFace::Outer(top)])
            .map(|(i, _)| measure::face_area(solid, FaceId(i as u32)))
            .sum()
    };
    let (inside, outside) = (area(&shelled.solid, true), area(&shelled.solid, false));
    assert!(inside < outside && volume > inside && volume < outside);
    assert!(volume < whole);
    // Every vertex is on its faces.
    for e in &shelled.solid.edges {
        for &c in &e.coedges {
            let surface = &shelled.solid.face(shelled.solid.coedge_face(c)).surface;
            for v in [e.start, e.end] {
                let off = surface.signed_distance(shelled.solid.vertex(v).point).abs();
                assert!(off < 2e-6, "{off}");
            }
        }
    }
}

#[test]
fn shell_a_vase_and_a_drilled_duct() {
    // Smooth through four circles: its sides bend both ways.
    let vase = lofted(&[
        (plane_at(0.0), circle(6.0)),
        (plane_at(10.0), circle(10.0)),
        (plane_at(22.0), circle(5.0)),
        (plane_at(30.0), circle(7.0)),
    ]);
    let top = face_facing(&vase, DVec3::Z);
    let shelled = shell(&vase, &[top], 0.8).unwrap();
    assert_wall(&vase, &shelled, 0.8);
    // About the wall thickness times the area of its walls and floor.
    let walled: f64 = vase
        .face_ids()
        .filter(|f| *f != top)
        .map(|f| measure::face_area(&vase, f))
        .sum();
    let volume = valid(&shelled.solid);
    assert!(
        volume > 0.8 * 0.8 * walled && volume < 0.8 * walled,
        "{volume}"
    );
    // Its waist has a radius of 5.
    let err = shell(&vase, &[top], 5.5).unwrap_err();
    assert!(err.to_string().contains("tightest curve"), "{err}");

    // A hole drilled across the duct: its rims are freeform curves between the duct's
    // freeform sides and the hole's cylinder.
    let duct = lofted(&[
        (plane_at(0.0), rectangle(20.0, 20.0)),
        (plane_at(25.0), circle(8.0)),
    ]);
    let mut sk = Sketch::new();
    sk.add_circle(DVec2::new(0.0, 12.0), 3.0);
    let rod = extrude(&Plane::right(), &find_regions(&sk).regions, -30.0, 30.0).unwrap();
    let drilled = boolean(&duct, &rod, BooleanOp::Subtract).unwrap();
    let top = face_facing(&drilled, DVec3::Z);
    let shelled = shell(&drilled, &[top], 1.0).unwrap();
    assert_valid(&shelled.solid);
    assert_wall(&drilled, &shelled, 1.0);
    // The hole has become a tube through the hollow: a cylinder a wall's thickness
    // wider around it, facing outwards.
    assert!(shelled.solid.faces.iter().any(|f| {
        matches!(&f.surface, Surface::Cylinder(c) if (c.radius - 4.0).abs() < 1e-9) && !f.reversed
    }));
    // One more wall than the plain duct's shell has, and so more material.
    let plain = shell(&duct, &[face_facing(&duct, DVec3::Z)], 1.0).unwrap();
    let (with, without) = (valid(&shelled.solid), valid(&plain.solid));
    assert!(with > without && with < without + 400.0, "{with} {without}");
}

fn stadium(length: f64, r: f64) -> Region {
    let mut s = Sketch::new();
    peet_sketch::shapes::slot(
        &mut s,
        DVec2::new(-length / 2.0, 0.0),
        DVec2::new(length / 2.0, 0.0),
        r,
    );
    find_regions(&s).regions.remove(0)
}

/// Simpson's rule over a height: exact for sections whose area is a quadratic.
fn simpson(height: f64, area: impl Fn(f64) -> f64) -> f64 {
    height / 6.0 * (area(0.0) + 4.0 * area(height / 2.0) + area(height))
}

#[test]
fn draft_freeform_walls() {
    let angle = 5f64.to_radians();
    let k = angle.tan();
    // A stadium lofted straight up: two flat walls and two round ones, which are
    // freeform (as the walls of an extruded spline are).
    let s = lofted(&[
        (plane_at(0.0), stadium(20.0, 6.0)),
        (plane_at(20.0), stadium(20.0, 6.0)),
    ]);
    let sides = walls(&s);
    assert_eq!(sides.len(), 4);
    assert!(
        sides
            .iter()
            .any(|f| matches!(s.face(*f).surface, Surface::Nurbs(_)))
    );
    // Its section at height z is the stadium with its radius less by z tan(a).
    let section = |r: f64| 40.0 * r + PI * r * r;
    let drafted = draft(&s, &sides, &Plane::TOP, angle).unwrap();
    near(
        checked(&drafted),
        simpson(20.0, |z| section(6.0 - z * k)),
        1e-8,
    );
    assert_eq!(drafted.faces.len(), 6);
    // About a plane half way up, the other way: wider at the top, narrower at the foot.
    let half = plane_at(10.0);
    let flared = draft(&s, &sides, &half, -angle).unwrap();
    near(
        valid(&flared),
        simpson(20.0, |z| section(6.0 + (z - 10.0) * k)),
        1e-8,
    );
    // About a plane below the body: its walls are straight, so they carry on down to it.
    let below = draft(&s, &sides, &plane_at(-5.0), angle).unwrap();
    near(
        valid(&below),
        simpson(20.0, |z| section(6.0 - (z + 5.0) * k)),
        1e-8,
    );
    // The same shape as a pocket through a block: it widens upwards.
    let block = cuboid(v3(-30.0, -20.0, 0.0), v3(30.0, 20.0, 20.0));
    let pocketed = boolean(&block, &s, BooleanOp::Subtract).unwrap();
    let pocket: Vec<FaceId> = walls(&pocketed)
        .into_iter()
        .filter(|f| {
            pocketed.face(*f).reversed || {
                // The pocket's flat walls face inwards too.
                let l = pocketed.face(*f).loops[0];
                let c = pocketed.loop_coedges(l)[0];
                let p = pocketed
                    .edge(pocketed.coedge(c).edge)
                    .point_at_fraction(0.5);
                p.x.abs() < 29.0 && p.y.abs() < 19.0
            }
        })
        .collect();
    assert_eq!(pocket.len(), 4);
    let drafted = draft(&pocketed, &pocket, &Plane::TOP, angle).unwrap();
    near(
        valid(&drafted),
        60.0 * 40.0 * 20.0 - simpson(20.0, |z| section(6.0 + z * k)),
        1e-8,
    );

    // A vase about the plane of its second profile: the cone through that circle.
    let vase = lofted(&[
        (plane_at(0.0), circle(6.0)),
        (plane_at(10.0), circle(10.0)),
        (plane_at(22.0), circle(5.0)),
        (plane_at(30.0), circle(7.0)),
    ]);
    let sides: Vec<FaceId> = vase
        .face_ids()
        .filter(|f| matches!(vase.face(*f).surface, Surface::Nurbs(_)))
        .collect();
    let drafted = draft(&vase, &sides, &plane_at(10.0), angle).unwrap();
    near(
        valid(&drafted),
        frustum(10.0 + 10.0 * k, -k, 0.0, 30.0),
        1e-8,
    );
    // The neutral plane has to cut right across the faces: above the vase it doesn't.
    let err = draft(&vase, &sides, &plane_at(40.0), angle).unwrap_err();
    assert!(err.to_string().contains("neutral plane"), "{err}");
    // Along the faces' edges it leaves them nothing to tilt about, one way or another.
    assert!(draft(&vase, &sides, &Plane::front(), angle).is_err());

    // A square that turns on the way up has ruled sides that are not flat; drafted
    // about its foot they are, and the body is a frustum of a pyramid.
    let turned = Plane {
        frame: peet_math::Frame {
            origin: v3(0.0, 0.0, 10.0),
            rotation: peet_math::DQuat::from_rotation_z(30f64.to_radians()),
        },
    };
    let prism = lofted(&[
        (plane_at(0.0), rectangle(10.0, 10.0)),
        (turned, rectangle(10.0, 10.0)),
    ]);
    let drafted = draft(&prism, &walls(&prism), &Plane::TOP, angle).unwrap();
    near(
        valid(&drafted),
        simpson(10.0, |z| (10.0 - 2.0 * z * k).powi(2)),
        1e-8,
    );
}

#[test]
fn draft_about_a_tilted_plane_and_draft_cones() {
    let angle = 3f64.to_radians();
    // A lofted cylinder about a plane that is not square to it: the neutral curve is an
    // ellipse, found by tracing, and stays where it was.
    let s = lofted(&[
        (plane_at(0.0), circle(10.0)),
        (plane_at(30.0), circle(10.0)),
    ]);
    let pull = v3(0.15, 0.0, 1.0).normalize();
    let tilted = Plane::from_origin_normal_x(v3(0.0, 0.0, 15.0), pull, DVec3::Y).unwrap();
    let drafted = draft(&s, &walls(&s), &tilted, angle).unwrap();
    assert_valid(&drafted);
    for i in 0..24 {
        // A point of the ellipse: on the cylinder, in the neutral plane.
        let a = f64::from(i) * 0.26;
        let (x, y) = (10.0 * a.cos(), 10.0 * a.sin());
        let p = v3(x, y, 15.0 - 0.15 * x);
        let off = drafted
            .faces
            .iter()
            .filter(|f| matches!(f.surface, Surface::Nurbs(_)))
            .map(|f| f.surface.signed_distance(p).abs())
            .fold(f64::INFINITY, f64::min);
        assert!(off < 2e-6, "{off}");
    }
    // Narrower along the pull above the plane, wider below it: the top cap shrinks.
    let area = |solid: &Solid, n: DVec3| measure::face_area(solid, face_facing(solid, n));
    assert!(area(&drafted, DVec3::Z) < area(&s, DVec3::Z));
    assert!(area(&drafted, -DVec3::Z) > area(&s, -DVec3::Z));

    // A cone drafts to the cone through its neutral circle: drafting twice is drafting
    // by the second angle, and by nothing gives the cylinder back.
    let r = rod(5.0, 10.0);
    let side = |s: &Solid| {
        s.face_ids()
            .find(|&f| !matches!(s.face(f).surface, Surface::Plane(_)))
            .unwrap()
    };
    let once = draft(&r, &[side(&r)], &Plane::TOP, 0.2).unwrap();
    let twice = draft(&once, &[side(&once)], &Plane::TOP, angle).unwrap();
    let top = 5.0 - 10.0 * angle.tan();
    assert_close(
        checked(&twice),
        PI * 10.0 / 3.0 * (25.0 + 5.0 * top + top * top),
    );
    let back = draft(&once, &[side(&once)], &Plane::TOP, 0.0).unwrap();
    assert_close(checked(&back), PI * 250.0);
    assert!(matches!(
        back.face(side(&back)).surface,
        Surface::Cylinder(_)
    ));
    // A ball has no walls to draft.
    let ball = crate::primitive::ball(&peet_math::Frame::WORLD, 5.0).unwrap();
    let err = draft(&ball, &[FaceId(0)], &Plane::TOP, angle).unwrap_err();
    assert!(err.to_string().contains("spherical"), "{err}");
}

#[test]
fn a_freeform_face_that_closes_on_itself_is_refused_cleanly() {
    // No kernel operation makes such a face yet (lofts and imports cut a closed side
    // into pieces), so this one is built by hand: a cylinder as one freeform face with
    // a seam. Its offset surface and seam are worked out, but following a loop round a
    // closed freeform face depends on which side of the seam the closest-point search
    // happens to pick, so the result does not validate. It must be refused, not
    // returned.
    use crate::geom::Circle3;
    use crate::nurbs::{NurbsCurve, NurbsSurface};
    use peet_math::Frame;
    use std::f64::consts::TAU;
    let up = Frame {
        origin: v3(0.0, 0.0, 10.0),
        ..Frame::WORLD
    };
    let surface = NurbsSurface::skin(&[
        NurbsCurve::arc(&Frame::WORLD, 5.0, 0.0, TAU),
        NurbsCurve::arc(&up, 5.0, 0.0, TAU),
    ])
    .unwrap();
    let mut s = Solid::new();
    let shell_id = s.add_shell();
    let (v0, v1) = (
        s.add_vertex(v3(5.0, 0.0, 0.0)),
        s.add_vertex(v3(5.0, 0.0, 10.0)),
    );
    let circle = |frame: Frame| Curve3::Circle(Circle3 { frame, radius: 5.0 });
    let bottom = s.add_edge(circle(Frame::WORLD), v0, v0, 0.0, TAU);
    let top = s.add_edge(circle(up), v1, v1, 0.0, TAU);
    let seam = s.add_line_edge(v0, v1);
    let side = s.add_face(
        shell_id,
        Surface::Nurbs(std::sync::Arc::new(surface)),
        false,
    );
    s.add_loop(
        side,
        &[(bottom, false), (seam, false), (top, true), (seam, true)],
    );
    let cap = s.add_face(shell_id, Surface::Plane(Plane { frame: up }), false);
    s.add_loop(cap, &[(top, false)]);
    let floor = s.add_face(shell_id, Surface::Plane(Plane::TOP), true);
    s.add_loop(floor, &[(bottom, true)]);
    assert_valid(&s);
    near(measure::volume(&s), PI * 250.0, 1e-9);
    match offset(&s, |_| -1.0) {
        Ok(smaller) => near(valid(&smaller), PI * 16.0 * 8.0, 1e-8),
        Err(e) => assert!(matches!(e, KernelError::InvalidResult(_)), "{e}"),
    }
}
