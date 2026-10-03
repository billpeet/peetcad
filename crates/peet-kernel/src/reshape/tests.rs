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
