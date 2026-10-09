//! Rolling-ball blend tests: validity, closed meshes, the ball's radius and tangency
//! measured on the result, and volumes against analytic bodies and estimates.

use std::collections::HashMap;
use std::f64::consts::{PI, TAU};
use std::sync::Arc;
use std::time::Instant;

use peet_math::{DQuat, DVec2, DVec3, Frame, Plane};
use peet_sketch::Sketch;
use peet_sketch::region::{Region, find_regions};

use super::super::{Blend, BlendFace, Blended, blend, tangent_chain};
use crate::boolean::{BooleanOp, boolean};
use crate::geom::{Circle3, Curve3, Cylinder, Surface};
use crate::loft::{LoftSection, loft};
use crate::nurbs::NurbsCurve;
use crate::primitive::cuboid;
use crate::tessellate::tessellate;
use crate::topo::{EdgeId, FaceId};
use crate::validate::{assert_valid, measure};
use crate::{KernelError, Solid};

fn v2(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

fn v3(x: f64, y: f64, z: f64) -> DVec3 {
    DVec3::new(x, y, z)
}

fn plane_at(z: f64) -> Plane {
    Plane::from_origin_normal_x(v3(0.0, 0.0, z), DVec3::Z, DVec3::X).unwrap()
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

fn lofted(sections: &[(f64, Region)]) -> Solid {
    let planes: Vec<Plane> = sections.iter().map(|(z, _)| plane_at(*z)).collect();
    let list: Vec<LoftSection<'_>> = sections
        .iter()
        .zip(&planes)
        .map(|((_, region), plane)| LoftSection { plane, region })
        .collect();
    loft(&list).unwrap()
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn edges_where(s: &Solid, pick: impl Fn(DVec3) -> bool) -> Vec<EdgeId> {
    s.edge_ids()
        .filter(|&e| pick(s.edge(e).point_at_fraction(0.5)))
        .collect()
}

#[track_caller]
fn close(got: f64, expected: f64, tol: f64) {
    assert!(
        (got - expected).abs() <= tol * expected.abs().max(1.0),
        "{got} instead of {expected}"
    );
}

/// Blends and checks the result: a closed mesh with the solid's volume. Prints how long the blend took.
#[track_caller]
fn blended(name: &str, s: &Solid, edges: &[EdgeId], shape: Blend) -> Blended {
    let start = Instant::now();
    let out = match blend(s, edges, shape) {
        Ok(out) => out,
        Err(e) => panic!("{name}: blend failed: {e}"),
    };
    println!(
        "{name}: {:.1} ms, {} blend faces",
        start.elapsed().as_secs_f64() * 1e3,
        out.faces
            .iter()
            .filter(|l| matches!(l[0], BlendFace::Blend(_)))
            .count()
    );
    // (`blend` has validated the solid: it never returns one that isn't valid.)
    assert_eq!(out.faces.len(), out.solid.faces.len());
    let mesh = tessellate(&out.solid, 0.01).unwrap();
    let bits = |p: DVec3| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
    let mut uses = HashMap::new();
    let mut volume = 0.0;
    for f in &mesh.faces {
        for t in &f.triangles {
            let [a, b, c] = t.map(|i| f.positions[i as usize]);
            volume += a.dot(b.cross(c)) / 6.0;
            for (p, q) in [(a, b), (b, c), (c, a)] {
                *uses.entry((bits(p), bits(q))).or_insert(0) += 1;
            }
        }
    }
    for (&(a, b), &count) in &uses {
        assert_eq!(count, 1, "{name}: an edge of the mesh is used twice");
        assert_eq!(uses.get(&(b, a)), Some(&1), "{name}: the mesh has a crack");
    }
    let exact = measure::volume(&out.solid);
    assert!(
        (volume - exact).abs() <= 0.01 * exact,
        "{name}: mesh {volume} vs {exact}"
    );
    out
}

fn blend_faces(b: &Blended) -> Vec<FaceId> {
    b.solid
        .face_ids()
        .filter(|f| matches!(b.faces[f.index()][0], BlendFace::Blend(_)))
        .collect()
}

/// Measures a fillet on its faces: the worst difference between `radius` and the
/// distance from the ball's centre (one radius behind a point of the fillet) to each
/// neighbouring face, and the worst angle between the fillet's normal and its
/// neighbours' along the contact curves. `convex`: the ball is inside the material.
fn measured(b: &Blended, radius: f64, convex: bool) -> (f64, f64) {
    let s = &b.solid;
    let (mut off, mut turn) = (0.0_f64, 0.0_f64);
    for face in blend_faces(b) {
        let surface = &s.face(face).surface;
        // The loop is: second face's side, end, first face's side, start.
        let coedges = s.loop_coedges(s.face(face).loops[0]);
        let side = |k: usize| {
            let e = s.edge(s.coedge(coedges[k]).edge);
            let other = s.coedge_face(s.twin(coedges[k]).unwrap());
            (e.t0, e.t1, other)
        };
        let (b0, b1, fb) = side(0);
        let (a0, a1, fa) = side(2);
        // Away from the ends, where the ball sits on the faces carried on past them.
        let (lo, hi) = (a0.max(b0), a1.min(b1));
        let inward = if convex { -1.0 } else { 1.0 };
        for i in 0..=12 {
            let v = lo + (hi - lo) * (0.15 + 0.7 * f64::from(i) / 12.0);
            for (u, neighbour) in [(0.0, Some(fa)), (0.5, None), (1.0, Some(fb))] {
                let uv = DVec2::new(u, v);
                let (p, n) = (surface.point(uv), surface.normal(uv));
                let centre = p + n * (inward * radius);
                for f in [fa, fb] {
                    let d = s.face(f).surface.signed_distance(centre).abs();
                    off = off.max((d - radius).abs());
                }
                if let Some(f) = neighbour {
                    turn = turn.max(n.cross(s.face_normal_at(f, p)).length());
                }
            }
        }
    }
    (off, turn)
}

/// The cross-section a fillet of `radius` removes where the faces' outward normals are
/// `turn` apart.
fn sliver(radius: f64, turn: f64) -> f64 {
    radius * radius * ((0.5 * turn).tan() - 0.5 * turn)
}

/// A rough volume for the fillet of `edges`: the sliver at each place along them times
/// the length, taking no account of how the edge curves.
fn estimate(s: &Solid, edges: &[EdgeId], radius: f64) -> f64 {
    let mut total = 0.0;
    for &id in edges {
        let e = s.edge(id);
        let [ca, cb] = e.coedges[..] else { panic!() };
        let n = 400;
        for i in 0..n {
            let t = e.t0 + (e.t1 - e.t0) * (f64::from(i) + 0.5) / f64::from(n);
            let p = e.curve.point(t);
            let turn = s
                .face_normal_at(s.coedge_face(ca), p)
                .angle_between(s.face_normal_at(s.coedge_face(cb), p));
            total += sliver(radius, turn) * e.curve.derivative(t).length() * (e.t1 - e.t0)
                / f64::from(n);
        }
    }
    total
}

fn count_original(b: &Blended, s: &Solid) {
    for face in s.face_ids() {
        let n = b
            .faces
            .iter()
            .filter(|l| l.contains(&BlendFace::Original(face)))
            .count();
        assert_eq!(n, 1, "face {} of the input", face.0);
    }
}

#[test]
fn rim_of_a_lofted_cylinder_matches_the_analytic_fillet() {
    // A loft between two equal circles is a cylinder written as four freeform faces.
    let s = lofted(&[(0.0, circle(10.0)), (20.0, circle(10.0))]);
    let rim = edges_where(&s, |p| near(p.z, 20.0));
    assert_eq!(rim.len(), 4);
    // The whole rim follows from any one of its arcs.
    let mut chain = tangent_chain(&s, rim[1]);
    chain.sort_unstable();
    assert_eq!(chain, rim);
    // The same body made of a real cylinder, blended with a tool: a torus.
    let mut sk = Sketch::new();
    sk.add_circle(DVec2::ZERO, 10.0);
    let rod = crate::extrude::extrude(&Plane::TOP, &find_regions(&sk).regions, 0.0, 20.0).unwrap();
    let rod_rim = edges_where(&rod, |p| near(p.z, 20.0));
    let volume = measure::volume(&s);
    close(volume, measure::volume(&rod), 1e-9);

    let f = blended(
        "lofted cylinder rim, fillet",
        &s,
        &rim,
        Blend::Fillet { radius: 2.0 },
    );
    let exact = blend(&rod, &rod_rim, Blend::Fillet { radius: 2.0 }).unwrap();
    let removed = volume - measure::volume(&f.solid);
    let expected = measure::volume(&rod) - measure::volume(&exact.solid);
    println!(
        "  removed {removed}, torus {expected}, off by {:.2e}",
        removed - expected
    );
    close(removed, expected, 3e-6);
    // Four fillet faces, one per arc, each under its edge's number.
    assert_eq!(f.solid.faces.len(), s.faces.len() + 4);
    for (i, _) in rim.iter().enumerate() {
        assert_eq!(
            f.faces
                .iter()
                .filter(|l| **l == vec![BlendFace::Blend(i)])
                .count(),
            1
        );
    }
    count_original(&f, &s);
    let (off, turn) = measured(&f, 2.0, true);
    println!("  radius off by {off:.2e} mm, tangent to {turn:.2e} rad");
    assert!(off < 2e-6 && turn < 2e-5, "{off} {turn}");

    let c = blended(
        "lofted cylinder rim, chamfer",
        &s,
        &rim,
        Blend::Chamfer { distance: 1.5 },
    );
    let exact = blend(&rod, &rod_rim, Blend::Chamfer { distance: 1.5 }).unwrap();
    let removed = volume - measure::volume(&c.solid);
    let expected = measure::volume(&rod) - measure::volume(&exact.solid);
    println!(
        "  chamfer removed {removed}, cone {expected}, off by {:.2e}",
        removed - expected
    );
    close(removed, expected, 3e-6);
    assert_eq!(c.solid.faces.len(), s.faces.len() + 4);
}

#[test]
fn rims_of_a_lofted_cone_match_the_analytic_blends() {
    let s = lofted(&[(0.0, circle(10.0)), (15.0, circle(6.0))]);
    // The same frustum turned from its section.
    let mut sk = Sketch::new();
    let pts = [v2(0.0, 0.0), v2(10.0, 0.0), v2(6.0, 15.0), v2(0.0, 15.0)];
    for i in 0..4 {
        sk.add_line(pts[i], pts[(i + 1) % 4]);
    }
    let turned = crate::revolve::revolve(
        &Plane::front(),
        &find_regions(&sk).regions,
        &crate::revolve::RevolveAxis {
            origin: DVec2::ZERO,
            dir: DVec2::Y,
        },
        0.0,
        TAU,
    )
    .unwrap();
    let (volume, turned_volume) = (measure::volume(&s), measure::volume(&turned));
    close(volume, turned_volume, 1e-9);
    for (z, name, shape) in [
        (15.0, "top", Blend::Fillet { radius: 1.5 }),
        (0.0, "bottom", Blend::Fillet { radius: 1.5 }),
        (0.0, "bottom", Blend::Chamfer { distance: 1.0 }),
    ] {
        let rim = edges_where(&s, |p| near(p.z, z));
        let turned_rim = edges_where(&turned, |p| near(p.z, z) && p.truncate().length() > 1.0);
        assert_eq!((rim.len(), turned_rim.len()), (4, 1));
        {
            let f = blended(
                &format!("lofted cone {name} rim, {shape:?}"),
                &s,
                &rim,
                shape,
            );
            let exact = blend(&turned, &turned_rim, shape).unwrap();
            let removed = volume - measure::volume(&f.solid);
            let expected = turned_volume - measure::volume(&exact.solid);
            println!(
                "  removed {removed}, analytic {expected}, off by {:.2e}",
                removed - expected
            );
            close(removed, expected, 3e-6);
            if let Blend::Fillet { radius } = shape {
                let (off, turn) = measured(&f, radius, true);
                println!("  radius off by {off:.2e} mm, tangent to {turn:.2e} rad");
                assert!(off < 2e-6 && turn < 2e-5, "{off} {turn}");
            }
        }
    }
}

/// A vase through three circles: its sides are freeform in both directions.
fn vase() -> Solid {
    lofted(&[
        (0.0, circle(10.0)),
        (12.0, circle(6.0)),
        (24.0, circle(9.0)),
    ])
}

/// The volume a fillet of `radius` takes from the rim of a solid of revolution about Z,
/// worked out in its half section: `wall` is one of its side faces (`v` running up it)
/// and the rim is where the wall meets the flat end at its `v = 1` (`top`) or `v = 0`.
fn turned_fillet(wall: &Surface, radius: f64, top: bool) -> f64 {
    let Surface::Nurbs(surface) = wall else {
        panic!("a freeform wall")
    };
    let (lo, hi) = surface.domain();
    // The half section: distance from the axis and height, and the outward normal.
    let section = |v: f64| {
        let uv = DVec2::new(lo.x, v);
        let (p, n) = (surface.point(uv), surface.normal(uv));
        let radial = p.truncate().normalize();
        (
            v2(p.truncate().length(), p.z),
            v2(n.truncate().dot(radial), n.z),
        )
    };
    let rim = if top { hi.y } else { lo.y };
    let level = section(rim).0.y;
    let up = if top { 1.0 } else { -1.0 };
    // The ball's centre is one radius inside the wall and one radius below the end.
    let centre = |v: f64| {
        let (p, n) = section(v);
        p - n * radius
    };
    let (mut inside, mut outside) = (0.5 * (lo.y + hi.y), rim);
    for _ in 0..80 {
        let mid = 0.5 * (inside + outside);
        if (centre(mid).y - (level - up * radius)) * up < 0.0 {
            inside = mid;
        } else {
            outside = mid;
        }
    }
    let touch = 0.5 * (inside + outside);
    let (c, on_wall) = (centre(touch), section(touch).0);
    // Round the sliver: up the wall to the corner, along the end (no height gained),
    // back down the arc. The volume turned is π ∮ ρ² dz.
    let n = 2000;
    let mut sum = 0.0;
    for i in 0..n {
        let at = |k: f64| section(touch + (rim - touch) * k / f64::from(n)).0;
        let (a, mid, b) = (
            at(f64::from(i)),
            at(f64::from(i) + 0.5),
            at(f64::from(i + 1)),
        );
        sum += (a.x * a.x + 4.0 * mid.x * mid.x + b.x * b.x) / 6.0 * (b.y - a.y);
    }
    let from = up * 0.5 * PI;
    let to = (on_wall - c).to_angle();
    for i in 0..n {
        let at = |k: f64| c + DVec2::from_angle(from + (to - from) * k / f64::from(n)) * radius;
        let (a, mid, b) = (
            at(f64::from(i)),
            at(f64::from(i) + 0.5),
            at(f64::from(i + 1)),
        );
        sum += (a.x * a.x + 4.0 * mid.x * mid.x + b.x * b.x) / 6.0 * (b.y - a.y);
    }
    PI * sum.abs()
}

#[test]
fn both_rims_of_a_freeform_loft() {
    let s = vase();
    let volume = measure::volume(&s);
    let top = edges_where(&s, |p| near(p.z, 24.0));
    let bottom = edges_where(&s, |p| near(p.z, 0.0));
    let mut both = top.clone();
    both.extend(&bottom);
    let f = blended("vase, both rims", &s, &both, Blend::Fillet { radius: 1.5 });
    assert_eq!(f.solid.faces.len(), s.faces.len() + 8);
    count_original(&f, &s);
    let removed = volume - measure::volume(&f.solid);
    // The vase is a solid of revolution, so the fillets can be worked out in its half
    // section.
    let wall = &s.faces[0].surface;
    let expected = turned_fillet(wall, 1.5, true) + turned_fillet(wall, 1.5, false);
    println!(
        "  removed {removed}, by section {expected}, off by {:.2e}",
        removed - expected
    );
    close(removed, expected, 3e-6);
    let (off, turn) = measured(&f, 1.5, true);
    println!("  radius off by {off:.2e} mm, tangent to {turn:.2e} rad");
    assert!(off < 2e-6 && turn < 2e-5, "{off} {turn}");
    let c = blended(
        "vase, top rim chamfer",
        &s,
        &top,
        Blend::Chamfer { distance: 1.0 },
    );
    assert!(measure::volume(&c.solid) < volume);
    assert_eq!(c.solid.faces.len(), s.faces.len() + 4);

    // Part of a rim stops where its edge carries on.
    let err = blend(&s, &top[..2], Blend::Fillet { radius: 1.5 }).unwrap_err();
    assert!(err.to_string().contains("carries on"), "{err}");
    // A ball as big as the rim can't roll round inside it.
    let err = blend(&s, &top, Blend::Fillet { radius: 12.0 }).unwrap_err();
    assert!(matches!(err, KernelError::InvalidInput(_)), "{err}");
}

/// A duct through three rectangles: four freeform sides that meet at sharp freeform
/// edges.
fn duct() -> Solid {
    lofted(&[
        (0.0, rectangle(20.0, 20.0)),
        (10.0, rectangle(14.0, 16.0)),
        (20.0, rectangle(18.0, 12.0)),
    ])
}

#[test]
fn open_edges_between_freeform_faces_end_on_the_caps() {
    let s = duct();
    let volume = measure::volume(&s);
    let rails = edges_where(&s, |p| p.z > 2.0 && p.z < 18.0);
    assert_eq!(rails.len(), 4);
    assert!(
        rails
            .iter()
            .all(|&e| matches!(s.edge(e).curve, Curve3::Nurbs(_)))
    );
    let one = blended(
        "duct, one rail",
        &s,
        &rails[..1],
        Blend::Fillet { radius: 2.0 },
    );
    assert_eq!(one.solid.faces.len(), s.faces.len() + 1);
    // Nothing was added to the count of edges but the fillet's own: two contact curves
    // and two ends, less the edge it replaces; two corners more.
    assert_eq!(one.solid.edges.len(), s.edges.len() + 3);
    assert_eq!(one.solid.vertices.len(), s.vertices.len() + 2);
    let removed = volume - measure::volume(&one.solid);
    let rough = estimate(&s, &rails[..1], 2.0);
    println!("  removed {removed}, estimate {rough}");
    close(removed, rough, 0.1);
    let (off, turn) = measured(&one, 2.0, true);
    println!("  radius off by {off:.2e} mm, tangent to {turn:.2e} rad");
    assert!(off < 2e-6 && turn < 2e-5, "{off} {turn}");
    // The fillet ends in the caps' planes.
    for v in &one.solid.vertices {
        assert!(v.point.z > -1e-6 && v.point.z < 20.0 + 1e-6);
    }

    let c = blended(
        "duct, four rails chamfer",
        &s,
        &rails,
        Blend::Chamfer { distance: 1.5 },
    );
    assert_eq!(c.solid.faces.len(), s.faces.len() + 4);
    count_original(&c, &s);
    // A chamfer takes a triangle where the fillet takes its sliver.
    let cut = volume - measure::volume(&c.solid);
    assert!(
        cut > 0.0 && cut < 4.0 * 0.5 * 1.5 * 1.5 * 21.0 * 1.2,
        "{cut}"
    );

    // Fillets wide enough to meet in the middle of a side are refused.
    let err = blend(&s, &rails, Blend::Fillet { radius: 7.0 }).unwrap_err();
    assert!(matches!(err, KernelError::InvalidInput(_)), "{err}");
    // The cap's rim has corners: its four fillets would have to be mitred.
    let rim = edges_where(&s, |p| near(p.z, 20.0));
    let err = blend(&s, &rim, Blend::Fillet { radius: 1.0 }).unwrap_err();
    assert!(err.to_string().contains("mitred"), "{err}");
    // One edge of the rim ends on faces that run along it, not across.
    let err = blend(&s, &rim[..1], Blend::Fillet { radius: 1.0 }).unwrap_err();
    assert!(matches!(err, KernelError::Unsupported(_)), "{err}");
}

#[test]
fn rectangle_to_circle_loft() {
    let s = lofted(&[(0.0, rectangle(30.0, 20.0)), (25.0, circle(8.0))]);
    // The edges from the rectangle's corners up to the circle: the sides are tangent
    // to each other where they reach the circle, so a fillet would shrink to nothing.
    let rails = edges_where(&s, |p| p.z > 2.0 && p.z < 23.0);
    assert_eq!(rails.len(), 4);
    let err = blend(&s, &rails[..1], Blend::Fillet { radius: 1.0 }).unwrap_err();
    assert!(err.to_string().contains("meet smoothly"), "{err}");
    // The round rim: the sides meet at an angle just below it.
    let rim = edges_where(&s, |p| near(p.z, 25.0));
    assert_eq!(tangent_chain(&s, rim[0]).len(), 4);
    let err = blend(&s, &rim, Blend::Fillet { radius: 1.0 }).unwrap_err();
    assert!(err.to_string().contains("meet at an angle"), "{err}");
    // The rectangular rim: corners.
    let foot = edges_where(&s, |p| near(p.z, 0.0));
    let err = blend(&s, &foot, Blend::Chamfer { distance: 1.0 }).unwrap_err();
    assert!(err.to_string().contains("mitred"), "{err}");
}

/// A cylinder cut off by a slanted plane: its top edge is an ellipse.
fn slanted_cylinder() -> Solid {
    let mut sk = Sketch::new();
    sk.add_circle(DVec2::ZERO, 4.0);
    let rod = crate::extrude::extrude(&Plane::TOP, &find_regions(&sk).regions, 0.0, 20.0).unwrap();
    let tilt = Frame {
        origin: v3(0.0, 0.0, 10.0),
        rotation: DQuat::from_rotation_y(0.4),
    };
    let block =
        crate::transform::solid(&cuboid(v3(-40.0, -40.0, 0.0), v3(40.0, 40.0, 40.0)), &tilt);
    boolean(&rod, &block, BooleanOp::Subtract).unwrap()
}

#[test]
fn elliptical_edge() {
    let s = slanted_cylinder();
    let volume = measure::volume(&s);
    let ellipse = edges_where(&s, |p| p.z > 5.0 && p.truncate().length() > 3.9);
    let ellipse: Vec<EdgeId> = ellipse
        .into_iter()
        .filter(|&e| matches!(s.edge(e).curve, Curve3::Ellipse(_)))
        .collect();
    assert!(!ellipse.is_empty());
    assert_eq!(tangent_chain(&s, ellipse[0]).len(), ellipse.len());
    let f = blended(
        "ellipse, fillet",
        &s,
        &ellipse,
        Blend::Fillet { radius: 1.0 },
    );
    let removed = volume - measure::volume(&f.solid);
    let rough = estimate(&s, &ellipse, 1.0);
    println!("  removed {removed}, estimate {rough}");
    close(removed, rough, 0.1);
    let (off, turn) = measured(&f, 1.0, true);
    println!("  radius off by {off:.2e} mm, tangent to {turn:.2e} rad");
    assert!(off < 2e-6 && turn < 2e-5, "{off} {turn}");
    // No face goes all the way round.
    assert!(blend_faces(&f).len() >= 2);
    count_original(&f, &s);

    // The same edge as an inside corner: the rod standing on the slanted block.
    let mut sk = Sketch::new();
    sk.add_circle(DVec2::ZERO, 4.0);
    let rod = crate::extrude::extrude(&Plane::TOP, &find_regions(&sk).regions, 0.0, 20.0).unwrap();
    let tilt = Frame {
        origin: v3(0.0, 0.0, 10.0),
        rotation: DQuat::from_rotation_y(0.4),
    };
    let block =
        crate::transform::solid(&cuboid(v3(-10.0, -10.0, -8.0), v3(10.0, 10.0, 0.0)), &tilt);
    let s = boolean(&block, &rod, BooleanOp::Union).unwrap();
    let volume = measure::volume(&s);
    let foot: Vec<EdgeId> = s
        .edge_ids()
        .filter(|&e| {
            matches!(s.edge(e).curve, Curve3::Ellipse(_))
                && s.edge(e).point_at_fraction(0.3).z > 6.0
        })
        .collect();
    assert!(!foot.is_empty());
    let f = blended(
        "ellipse, concave fillet",
        &s,
        &foot,
        Blend::Fillet { radius: 1.0 },
    );
    let added = measure::volume(&f.solid) - volume;
    let rough = estimate(&s, &foot, 1.0);
    println!("  added {added}, estimate {rough}");
    close(added, rough, 0.12);
    let (off, turn) = measured(&f, 1.0, false);
    println!("  radius off by {off:.2e} mm, tangent to {turn:.2e} rad");
    assert!(off < 2e-6 && turn < 2e-5, "{off} {turn}");
    let c = blended(
        "ellipse, concave chamfer",
        &s,
        &foot,
        Blend::Chamfer { distance: 0.8 },
    );
    assert!(measure::volume(&c.solid) > volume);
}

/// A piecewise cubic through points with the given directions at each: what the
/// boolean operations fit to a marched intersection.
fn hermite(points: &[DVec3], directions: &[DVec3]) -> NurbsCurve {
    let mut knots = vec![0.0; 4];
    let mut control = vec![points[0]];
    let mut at = 0.0;
    let n = points.len();
    for i in 0..n - 1 {
        let length = points[i].distance(points[i + 1]);
        control.push(points[i] + directions[i].normalize() * (length / 3.0));
        control.push(points[i + 1] - directions[i + 1].normalize() * (length / 3.0));
        control.push(points[i + 1]);
        at += length;
        knots.extend(std::iter::repeat_n(at, if i + 2 == n { 4 } else { 3 }));
    }
    NurbsCurve::new(3, knots, control, None).unwrap()
}

/// A cylinder of radius `a` about Z from `z = -h` to `h`, with a hole of radius `b`
/// drilled across it along X. The booleans can't make this (two analytic cylinders
/// that cross), so it is built by hand: both faces at each rim are cylinders and the
/// rim is a freeform curve, in two halves.
fn cross_drilled(a: f64, b: f64, h: f64) -> Solid {
    let mut s = Solid::new();
    let shell = s.add_shell();
    // The wall, its seam at −Y, clear of the hole's rims.
    let frame = Frame::from_origin_z_x(v3(0.0, 0.0, -h), DVec3::Z, -DVec3::Y).unwrap();
    let top_frame = Frame {
        origin: v3(0.0, 0.0, h),
        ..frame
    };
    let vb = s.add_vertex(v3(0.0, -a, -h));
    let vt = s.add_vertex(v3(0.0, -a, h));
    let ring = |frame: Frame| Curve3::Circle(Circle3 { frame, radius: a });
    let bottom = s.add_edge(ring(frame), vb, vb, 0.0, TAU);
    let top = s.add_edge(ring(top_frame), vt, vt, 0.0, TAU);
    let seam = s.add_line_edge(vb, vt);
    // The hole: its angle runs from +Y towards +Z, its seam along the line at +Y.
    let hole_frame = Frame::from_origin_z_x(DVec3::ZERO, DVec3::X, DVec3::Y).unwrap();
    let x_at = |angle: f64| (a * a - (b * angle.cos()).powi(2)).sqrt();
    let rim = |sign: f64, from: f64| -> Curve3 {
        let n = 48;
        let mut points = Vec::with_capacity(n + 1);
        let mut directions = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let angle = from + PI * i as f64 / n as f64;
            let (sin, cos) = angle.sin_cos();
            let x = x_at(angle);
            points.push(v3(sign * x, b * cos, b * sin));
            directions.push(v3(sign * b * b * cos * sin / x, -b * sin, b * cos));
        }
        Curve3::Nurbs(Arc::new(hermite(&points, &directions)))
    };
    let mut rims = Vec::new();
    let mut seam_ends = Vec::new();
    for sign in [1.0, -1.0] {
        let p0 = s.add_vertex(v3(sign * x_at(0.0), b, 0.0));
        let p1 = s.add_vertex(v3(sign * x_at(PI), -b, 0.0));
        let (first, second) = (rim(sign, 0.0), rim(sign, PI));
        let end = |c: &Curve3| c.domain().unwrap().1;
        let e1 = s.add_edge(first.clone(), p0, p1, 0.0, end(&first));
        let e2 = s.add_edge(second.clone(), p1, p0, 0.0, end(&second));
        rims.push((e1, e2));
        seam_ends.push(p0);
    }
    let hole_seam = s.add_line_edge(seam_ends[1], seam_ends[0]);
    let ((p1, p2), (m1, m2)) = (rims[0], rims[1]);
    let wall = s.add_face(
        shell,
        Surface::Cylinder(Cylinder { frame, radius: a }),
        false,
    );
    s.add_loop(
        wall,
        &[(bottom, false), (seam, false), (top, true), (seam, true)],
    );
    s.add_loop(wall, &[(p2, true), (p1, true)]);
    s.add_loop(wall, &[(m1, false), (m2, false)]);
    let hole = s.add_face(
        shell,
        Surface::Cylinder(Cylinder {
            frame: hole_frame,
            radius: b,
        }),
        true,
    );
    s.add_loop(
        hole,
        &[
            (hole_seam, false),
            (p1, false),
            (p2, false),
            (hole_seam, true),
            (m2, true),
            (m1, true),
        ],
    );
    let cap = s.add_face(shell, Surface::Plane(Plane { frame: top_frame }), false);
    s.add_loop(cap, &[(top, false)]);
    let cap = s.add_face(shell, Surface::Plane(Plane { frame }), true);
    s.add_loop(cap, &[(bottom, true)]);
    s
}

/// The volume a fillet of radius `r` takes from one rim of [`cross_drilled`], from the
/// definition: what is left is whatever a ball of that radius can reach from inside
/// the material. Integrated over the hole's own coordinates, by the midpoint rule.
fn drilled_fillet(a: f64, b: f64, r: f64) -> f64 {
    // Where the ball's centre runs: a − r from the Z axis and b + r from the X axis.
    let spine = |angle: f64| {
        let (sin, cos) = angle.sin_cos();
        let (y, z) = ((b + r) * cos, (b + r) * sin);
        v3(((a - r).powi(2) - y * y).sqrt(), y, z)
    };
    let kept = |p: DVec3| -> bool {
        let deep = |q: DVec3| q.x.hypot(q.y) <= a - r + 1e-12;
        let clear = |q: DVec3| q.y.hypot(q.z) >= b + r - 1e-12;
        if deep(p) && clear(p) {
            return true;
        }
        // The nearest place a centre can be: straight in from the wall, straight out
        // from the hole, or on the spine.
        let rho = p.x.hypot(p.y);
        let inwards = v3(p.x * (a - r) / rho, p.y * (a - r) / rho, p.z);
        if rho > a - r && clear(inwards) && rho - (a - r) <= r {
            return true;
        }
        let s = p.y.hypot(p.z);
        let outwards = v3(p.x, p.y * (b + r) / s, p.z * (b + r) / s);
        if s < b + r && deep(outwards) && (b + r) - s <= r {
            return true;
        }
        let around = p.z.atan2(p.y);
        let (mut lo, mut hi) = (around - 0.4, around + 0.4);
        for _ in 0..40 {
            let (m1, m2) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
            if spine(m1).distance(p) < spine(m2).distance(p) {
                hi = m2;
            } else {
                lo = m1;
            }
        }
        spine(0.5 * (lo + hi)).distance(p) <= r
    };
    let (n_angle, n_s) = (64, 160);
    let reach = 2.0 * r;
    let mut total = 0.0;
    for i in 0..n_angle {
        let angle = 0.5 * PI * (f64::from(i) + 0.5) / f64::from(n_angle);
        for j in 0..n_s {
            let s = b + reach * (f64::from(j) + 0.5) / f64::from(n_s);
            let (y, z) = (s * angle.cos(), s * angle.sin());
            let surface = (a * a - y * y).sqrt();
            if kept(v3(surface, y, z)) {
                continue;
            }
            let (mut inside, mut outside) = (surface - 3.0 * r, surface);
            for _ in 0..30 {
                let mid = 0.5 * (inside + outside);
                if kept(v3(mid, y, z)) {
                    inside = mid;
                } else {
                    outside = mid;
                }
            }
            total += (surface - inside) * s;
        }
    }
    // A quarter of the rim was covered.
    4.0 * total * (0.5 * PI / f64::from(n_angle)) * (reach / f64::from(n_s))
}

#[test]
fn rim_of_a_cross_drilled_hole() {
    let (a, b, h) = (10.0, 3.0, 12.0);
    let s = cross_drilled(a, b, h);
    assert_valid(&s);
    // The hole takes out a slug: the hole's disc, as long at each place as the cylinder
    // is thick there.
    let n = 200_000;
    let slug: f64 = (0..n)
        .map(|i| {
            let y = -b + 2.0 * b * (f64::from(i) + 0.5) / f64::from(n);
            4.0 * ((a * a - y * y) * (b * b - y * y)).sqrt() * 2.0 * b / f64::from(n)
        })
        .sum();
    let volume = measure::volume(&s);
    close(volume, PI * a * a * 2.0 * h - slug, 1e-7);

    let rim = edges_where(&s, |p| p.x > 5.0 && p.z.abs() > 2.9);
    assert_eq!(rim.len(), 2);
    // The two halves carry on from each other all the way round.
    assert_eq!(tangent_chain(&s, rim[0]).len(), 2);
    let f = blended(
        "cross-drilled rim, fillet",
        &s,
        &rim,
        Blend::Fillet { radius: 1.0 },
    );
    let removed = volume - measure::volume(&f.solid);
    let expected = drilled_fillet(a, b, 1.0);
    println!("  removed {removed}, by rolling a ball numerically {expected}");
    close(removed, expected, 3e-3);
    let (off, turn) = measured(&f, 1.0, true);
    println!("  radius off by {off:.2e} mm, tangent to {turn:.2e} rad");
    assert!(off < 2e-6 && turn < 2e-5, "{off} {turn}");
    count_original(&f, &s);
    // Both faces at the rim are still cylinders; only the fillet is freeform.
    let freeform = f
        .solid
        .faces
        .iter()
        .filter(|face| matches!(face.surface, Surface::Nurbs(_)))
        .count();
    assert_eq!(freeform, blend_faces(&f).len());

    // Both rims at once, chamfered.
    let both = edges_where(&s, |p| p.x.abs() > 5.0 && p.z.abs() > 2.9);
    assert_eq!(both.len(), 4);
    let c = blended(
        "cross-drilled, both rims chamfer",
        &s,
        &both,
        Blend::Chamfer { distance: 0.8 },
    );
    assert!(measure::volume(&c.solid) < volume);
    assert_eq!(c.solid.faces.len(), s.faces.len() + 4);
    // A fillet as big as the hole can't roll round its rim.
    let err = blend(&s, &rim, Blend::Fillet { radius: 3.5 }).unwrap_err();
    assert!(matches!(err, KernelError::InvalidInput(_)), "{err}");
}

#[test]
fn analytic_and_rolled_blends_in_one_call() {
    // The slanted cylinder's ellipse is rolled; its round foot is still blended with a
    // tool, into a torus. Each keeps the number of its edge.
    let s = slanted_cylinder();
    let volume = measure::volume(&s);
    let ellipse: Vec<EdgeId> = s
        .edge_ids()
        .filter(|&e| matches!(s.edge(e).curve, Curve3::Ellipse(_)))
        .collect();
    let foot: Vec<EdgeId> = s
        .edge_ids()
        .filter(|&e| matches!(s.edge(e).curve, Curve3::Circle(_)))
        .collect();
    assert_eq!((ellipse.len(), foot.len()), (1, 1));
    let both = [foot[0], ellipse[0]];
    let f = blended(
        "ellipse and round foot",
        &s,
        &both,
        Blend::Fillet { radius: 1.0 },
    );
    let torus = f
        .solid
        .face_ids()
        .find(|x| matches!(f.solid.face(*x).surface, Surface::Torus(_)))
        .expect("the foot's fillet is a torus");
    assert_eq!(f.faces[torus.index()], vec![BlendFace::Blend(0)]);
    for face in blend_faces(&f) {
        if face != torus {
            assert_eq!(f.faces[face.index()], vec![BlendFace::Blend(1)]);
            assert!(matches!(f.solid.face(face).surface, Surface::Nurbs(_)));
        }
    }
    let one = blend(&s, &ellipse, Blend::Fillet { radius: 1.0 }).unwrap();
    let foot_only = blend(&s, &foot, Blend::Fillet { radius: 1.0 }).unwrap();
    close(
        volume - measure::volume(&f.solid),
        2.0 * volume - measure::volume(&one.solid) - measure::volume(&foot_only.solid),
        1e-8,
    );
    // Analytic bodies are untouched by all this: a block's edge is still a cylinder.
    let block = cuboid(DVec3::ZERO, DVec3::splat(10.0));
    let edge = edges_where(&block, |p| near(p.x, 10.0) && near(p.y, 10.0));
    let f = blend(&block, &edge, Blend::Fillet { radius: 2.0 }).unwrap();
    assert!(
        f.solid
            .faces
            .iter()
            .any(|x| matches!(x.surface, Surface::Cylinder(_)))
    );
    assert!(
        !f.solid
            .faces
            .iter()
            .any(|x| matches!(x.surface, Surface::Nurbs(_)))
    );
}
