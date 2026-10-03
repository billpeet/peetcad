//! Blend tests: validity, meshes and volumes against hand calculations.

use std::f64::consts::{PI, TAU};

use peet_math::{DQuat, DVec2, DVec3, Frame, Plane};
use peet_sketch::Sketch;
use peet_sketch::region::{Region, find_regions};

use super::*;
use crate::boolean::boolean;
use crate::extrude::extrude;
use crate::primitive::cuboid;
use crate::tessellate::tessellate;
use crate::validate::{assert_valid, measure};

fn v3(x: f64, y: f64, z: f64) -> DVec3 {
    DVec3::new(x, y, z)
}

fn v2(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

#[track_caller]
fn assert_close(got: f64, expected: f64) {
    assert!(
        (got - expected).abs() <= 1e-8 * expected.abs().max(1.0),
        "{got} instead of {expected}"
    );
}

/// The area of a fillet's sliver: the square of side `r` less the quarter disc.
fn sliver(r: f64) -> f64 {
    r * r * (1.0 - PI / 4.0)
}

/// How far the sliver's centroid is from each of its two straight sides... measured from
/// the corner along either side.
fn sliver_centroid(r: f64) -> f64 {
    r * (5.0 / 6.0 - PI / 4.0) / (1.0 - PI / 4.0)
}

/// The edges whose midpoint satisfies `pick`.
fn edges_where(s: &Solid, pick: impl Fn(DVec3) -> bool) -> Vec<EdgeId> {
    s.edge_ids()
        .filter(|&e| pick(s.edge(e).point_at_fraction(0.5)))
        .collect()
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

/// Blends and checks the result: valid, meshable, closed.
#[track_caller]
fn blended(s: &Solid, edges: &[EdgeId], b: Blend) -> Blended {
    let out = match blend(s, edges, b) {
        Ok(out) => out,
        Err(e) => panic!("blend failed: {e}"),
    };
    assert_valid(&out.solid);
    assert_eq!(out.faces.len(), out.solid.faces.len());
    let mesh = tessellate(&out.solid, 0.02).unwrap();
    let volume: f64 = mesh
        .triangles()
        .iter()
        .map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0)
        .sum();
    let exact = measure::volume(&out.solid);
    assert!(
        (volume - exact).abs() <= 0.01 * exact,
        "{volume} vs {exact}"
    );
    out
}

fn count(b: &Blended, what: impl Fn(&BlendFace) -> bool) -> usize {
    b.faces
        .iter()
        .filter(|labels| labels.iter().any(&what))
        .count()
}

fn block() -> Solid {
    cuboid(DVec3::ZERO, DVec3::splat(10.0))
}

#[test]
fn one_edge_of_a_block() {
    let s = block();
    let edge = edges_where(&s, |p| near(p.x, 10.0) && near(p.y, 10.0));
    assert_eq!(edge.len(), 1);
    let f = blended(&s, &edge, Blend::Fillet { radius: 2.0 });
    assert_close(measure::volume(&f.solid), 1000.0 - sliver(2.0) * 10.0);
    assert_eq!(f.solid.faces.len(), 7);
    assert_eq!(count(&f, |l| *l == BlendFace::Blend(0)), 1);
    // The fillet is a cylinder, tangent to both faces.
    let fillet = f
        .faces
        .iter()
        .position(|l| l.contains(&BlendFace::Blend(0)))
        .unwrap();
    assert!(matches!(
        f.solid.faces[fillet].surface,
        Surface::Cylinder(c) if near(c.radius, 2.0)
    ));
    assert!(!f.solid.faces[fillet].reversed);
    let c = blended(&s, &edge, Blend::Chamfer { distance: 2.0 });
    assert_close(measure::volume(&c.solid), 1000.0 - 2.0 * 10.0);
    assert_eq!(c.solid.faces.len(), 7);
    // Every original face is still there under its own label.
    for face in s.face_ids() {
        assert_eq!(count(&c, |l| *l == BlendFace::Original(face)), 1);
    }
}

#[test]
fn parallel_edges() {
    let s = block();
    let uprights = edges_where(&s, |p| near(p.z, 5.0));
    assert_eq!(uprights.len(), 4);
    let f = blended(&s, &uprights, Blend::Fillet { radius: 3.0 });
    assert_close(measure::volume(&f.solid), 1000.0 - 4.0 * sliver(3.0) * 10.0);
    assert_eq!(f.solid.faces.len(), 10);
    // Each edge's fillet is told apart.
    for i in 0..4 {
        assert_eq!(count(&f, |l| *l == BlendFace::Blend(i)), 1);
    }
    // An edge given twice is blended once.
    let twice = [uprights[0], uprights[0]];
    let f = blended(&s, &twice, Blend::Fillet { radius: 3.0 });
    assert_close(measure::volume(&f.solid), 1000.0 - sliver(3.0) * 10.0);
}

/// The volume two fillet slivers at right angles share where they cross at a corner.
fn mitre_overlap(r: f64) -> f64 {
    // At depth b below the face both slivers are w(b) wide.
    let w = |b: f64| r - (2.0 * r * b - b * b).sqrt();
    let n = 20_000;
    let h = r / n as f64;
    (0..n)
        .map(|i| {
            let b = (i as f64 + 0.5) * h;
            w(b) * w(b) * h
        })
        .sum()
}

#[test]
fn edges_meeting_in_mitres() {
    let s = block();
    let top = edges_where(&s, |p| near(p.z, 10.0));
    assert_eq!(top.len(), 4);
    let f = blended(&s, &top, Blend::Fillet { radius: 2.0 });
    // Four slivers, counted once where two cross at each corner.
    let expected = 1000.0 - 4.0 * sliver(2.0) * 10.0 + 4.0 * mitre_overlap(2.0);
    let got = measure::volume(&f.solid);
    assert!((got - expected).abs() < 1e-5, "{got} vs {expected}");
    // The fillets meet in ellipses.
    assert!(
        f.solid
            .edges
            .iter()
            .any(|e| matches!(e.curve, Curve3::Ellipse(_)))
    );
    let c = blended(&s, &top, Blend::Chamfer { distance: 2.0 });
    // Chamfers: a frustum's worth is left of the top 2 mm.
    let frustum = 2.0 / 3.0 * (100.0 + 36.0 + 60.0);
    assert_close(measure::volume(&c.solid), 800.0 + frustum);
    assert_eq!(c.solid.faces.len(), 10);
}

#[test]
fn three_fillets_round_the_corner_with_a_ball() {
    let s = block();
    let at_corner = edges_where(&s, |p| {
        [near(p.x, 10.0), near(p.y, 10.0), near(p.z, 10.0)]
            .iter()
            .filter(|b| **b)
            .count()
            == 2
    });
    assert_eq!(at_corner.len(), 3);
    let r = 2.0;
    let f = blended(&s, &at_corner, Blend::Fillet { radius: r });
    let removed = r * r * r * (1.0 - PI / 6.0) + 3.0 * sliver(r) * (10.0 - r);
    assert_close(measure::volume(&f.solid), 1000.0 - removed);
    assert_eq!(count(&f, |l| matches!(l, BlendFace::Corner(_))), 1);
    let ball = f
        .solid
        .faces
        .iter()
        .filter(|face| matches!(face.surface, Surface::Sphere(_)))
        .count();
    assert_eq!(ball, 1);
    // 6 faces, 3 fillets and the ball.
    assert_eq!(f.solid.faces.len(), 10);
    // Every edge and corner of the block.
    let all: Vec<EdgeId> = s.edge_ids().collect();
    let f = blended(&s, &all, Blend::Fillet { radius: r });
    let removed = 8.0 * r * r * r * (1.0 - PI / 6.0) + 12.0 * sliver(r) * (10.0 - 2.0 * r);
    assert_close(measure::volume(&f.solid), 1000.0 - removed);
    assert_eq!(f.solid.faces.len(), 6 + 12 + 8);
    // All chamfered: three chamfers meet in a point at each corner.
    let c = blended(&s, &all, Blend::Chamfer { distance: 1.0 });
    assert_eq!(c.solid.faces.len(), 6 + 12);
}

fn region_at(s: &Sketch, at: DVec2) -> Vec<Region> {
    let profile = find_regions(s);
    let i = profile.region_at(at).expect("a region at the pick point");
    vec![profile.regions[i].clone()]
}

/// An L-shaped bracket: 10 thick in y, legs along x and z.
fn bracket() -> Solid {
    let mut s = Sketch::new();
    let pts = [
        v2(0.0, 0.0),
        v2(30.0, 0.0),
        v2(30.0, 5.0),
        v2(5.0, 5.0),
        v2(5.0, 20.0),
        v2(0.0, 20.0),
    ];
    for i in 0..pts.len() {
        s.add_line(pts[i], pts[(i + 1) % pts.len()]);
    }
    extrude(&Plane::front(), &region_at(&s, v2(2.0, 2.0)), -10.0, 0.0).unwrap()
}

#[test]
fn concave_edge_gets_material() {
    let s = bracket();
    let volume = measure::volume(&s);
    // The inside corner, along y at x = 5, z = 5.
    let inner = edges_where(&s, |p| near(p.x, 5.0) && near(p.z, 5.0));
    assert_eq!(inner.len(), 1);
    let f = blended(&s, &inner, Blend::Fillet { radius: 3.0 });
    assert_close(measure::volume(&f.solid), volume + sliver(3.0) * 10.0);
    let fillet = f
        .faces
        .iter()
        .position(|l| l.contains(&BlendFace::Blend(0)))
        .unwrap();
    // The fillet faces the hollow side: against the cylinder's own normal.
    assert!(f.solid.faces[fillet].reversed);
    let c = blended(&s, &inner, Blend::Chamfer { distance: 3.0 });
    assert_close(measure::volume(&c.solid), volume + 4.5 * 10.0);
    // Inside and outside corners together.
    let outer = edges_where(&s, |p| near(p.x, 0.0) && near(p.z, 0.0));
    let both = [inner[0], outer[0]];
    let f = blended(&s, &both, Blend::Fillet { radius: 3.0 });
    assert_close(measure::volume(&f.solid), volume);
}

/// A plate with a round boss on it and a round hole through both.
fn bossed_plate() -> Solid {
    let plate = cuboid(v3(-20.0, -20.0, 0.0), v3(20.0, 20.0, 5.0));
    let mut s = Sketch::new();
    s.add_circle(DVec2::ZERO, 8.0);
    let boss = extrude(&Plane::TOP, &region_at(&s, v2(1.0, 1.0)), 5.0, 15.0).unwrap();
    let mut s = Sketch::new();
    s.add_circle(DVec2::ZERO, 3.0);
    let drill = extrude(&Plane::TOP, &region_at(&s, v2(1.0, 1.0)), -1.0, 16.0).unwrap();
    let body = boolean(&plate, &boss, BooleanOp::Union).unwrap();
    boolean(&body, &drill, BooleanOp::Subtract).unwrap()
}

#[test]
fn round_edges() {
    let s = bossed_plate();
    let volume = measure::volume(&s);
    let circle = |radius: f64, z: f64| {
        let found: Vec<EdgeId> = s
            .edge_ids()
            .filter(|&e| {
                matches!(s.edge(e).curve, Curve3::Circle(c)
                    if near(c.radius, radius) && near(c.frame.origin.z, z))
            })
            .collect();
        assert_eq!(found.len(), 1, "circle of radius {radius} at z = {z}");
        found
    };
    let (r, a, x) = (1.0, sliver(1.0), sliver_centroid(1.0));
    // The boss's top rim: convex, the sliver's centroid inside the rim.
    let f = blended(&s, &circle(8.0, 15.0), Blend::Fillet { radius: r });
    assert_close(measure::volume(&f.solid), volume - TAU * (8.0 - x) * a);
    assert!(
        f.solid
            .faces
            .iter()
            .any(|face| matches!(face.surface, Surface::Torus(t) if near(t.major, 7.0)))
    );
    // The hole's rim: convex too, the sliver outside the hole.
    let f = blended(&s, &circle(3.0, 15.0), Blend::Fillet { radius: r });
    assert_close(measure::volume(&f.solid), volume - TAU * (3.0 + x) * a);
    // The foot of the boss: concave, filled in.
    let f = blended(&s, &circle(8.0, 5.0), Blend::Fillet { radius: r });
    assert_close(measure::volume(&f.solid), volume + TAU * (8.0 + x) * a);
    // Chamfers there are cones; a countersink on the hole.
    let c = blended(&s, &circle(3.0, 15.0), Blend::Chamfer { distance: 1.5 });
    let tri = 0.5 * 1.5 * 1.5;
    assert_close(measure::volume(&c.solid), volume - TAU * (3.0 + 0.5) * tri);
    assert!(
        c.solid
            .faces
            .iter()
            .any(|face| matches!(face.surface, Surface::Cone(_)))
    );
    // Everything round at once.
    let mut all = circle(8.0, 15.0);
    all.extend(circle(3.0, 15.0));
    all.extend(circle(8.0, 5.0));
    all.extend(circle(3.0, 0.0));
    let f = blended(&s, &all, Blend::Fillet { radius: r });
    assert_close(
        measure::volume(&f.solid),
        volume - TAU * (8.0 - x) * a - 2.0 * TAU * (3.0 + x) * a + TAU * (8.0 + x) * a,
    );
    // A fillet too big for the rim.
    let err = blend(&s, &circle(8.0, 15.0), Blend::Fillet { radius: 9.0 }).unwrap_err();
    assert!(err.to_string().contains("too big"), "{err}");
}

/// A plate with rounded corners (an outline of lines and arcs).
fn rounded_plate() -> Solid {
    let mut s = Sketch::new();
    let (w, h, r) = (40.0, 20.0, 5.0);
    s.add_line(v2(r, 0.0), v2(w - r, 0.0));
    s.add_arc(v2(w - r, r), v2(w - r, 0.0), v2(w, r));
    s.add_line(v2(w, r), v2(w, h - r));
    s.add_arc(v2(w - r, h - r), v2(w, h - r), v2(w - r, h));
    s.add_line(v2(w - r, h), v2(r, h));
    s.add_arc(v2(r, h - r), v2(r, h), v2(0.0, h - r));
    s.add_line(v2(0.0, h - r), v2(0.0, r));
    s.add_arc(v2(r, r), v2(0.0, r), v2(r, 0.0));
    extrude(&Plane::TOP, &region_at(&s, v2(20.0, 10.0)), 0.0, 6.0).unwrap()
}

#[test]
fn a_smooth_chain_of_lines_and_arcs() {
    let s = rounded_plate();
    let volume = measure::volume(&s);
    let start = edges_where(&s, |p| near(p.z, 6.0) && near(p.y, 0.0));
    assert_eq!(start.len(), 1);
    let chain = tangent_chain(&s, start[0]);
    assert_eq!(chain.len(), 8);
    assert!(
        chain
            .iter()
            .all(|&e| near(s.edge(e).point_at_fraction(0.5).z, 6.0))
    );
    let (r, a, x) = (1.5, sliver(1.5), sliver_centroid(1.5));
    let f = blended(&s, &chain, Blend::Fillet { radius: r });
    // The sliver's centroid runs round a path set in by x from the outline.
    let outline = 2.0 * (30.0 + 10.0) + TAU * 5.0;
    assert_close(measure::volume(&f.solid), volume - a * (outline - TAU * x));
    // No steps are left between the pieces.
    assert_eq!(count(&f, |l| matches!(l, BlendFace::End(_))), 0);
    assert_eq!(f.solid.faces.len(), s.faces.len() + 8);
    // One piece of the chain alone ends in steps.
    let f = blended(&s, &start, Blend::Fillet { radius: r });
    assert_close(measure::volume(&f.solid), volume - a * 30.0);
    assert_eq!(count(&f, |l| matches!(l, BlendFace::End(_))), 2);
    // The upright edges are smooth (a flat side runs into a round corner).
    let smooth = edges_where(&s, |p| near(p.z, 3.0));
    let err = blend(&s, &smooth[..1], Blend::Fillet { radius: 1.0 }).unwrap_err();
    assert!(err.to_string().contains("no corner"), "{err}");
}

#[test]
fn an_edge_ending_on_a_slanted_face() {
    // A block with one end cut off at a slant.
    let s = cuboid(DVec3::ZERO, v3(30.0, 10.0, 10.0));
    let cutter = transform::solid(
        &cuboid(v3(0.0, -20.0, -20.0), v3(40.0, 20.0, 20.0)),
        &Frame {
            origin: v3(22.0, 5.0, 5.0),
            rotation: DQuat::from_rotation_z(0.5) * DQuat::from_rotation_y(-0.3),
        },
    );
    let s = boolean(&s, &cutter, BooleanOp::Subtract).unwrap();
    assert_valid(&s);
    let volume = measure::volume(&s);
    let long = edges_where(&s, |p| near(p.y, 10.0) && near(p.z, 10.0));
    assert_eq!(long.len(), 1);
    let length = measure_edge(&s, long[0]);
    let f = blended(&s, &long, Blend::Fillet { radius: 2.0 });
    let removed = volume - measure::volume(&f.solid);
    // About the sliver along the edge: the slanted end takes or gives a little.
    assert!(
        (removed - sliver(2.0) * length).abs() < sliver(2.0) * 2.0,
        "{removed} for an edge {length} long"
    );
    assert_eq!(f.solid.faces.len(), s.faces.len() + 1);
    let c = blended(&s, &long, Blend::Chamfer { distance: 2.0 });
    assert_eq!(c.solid.faces.len(), s.faces.len() + 1);
}

fn measure_edge(s: &Solid, e: EdgeId) -> f64 {
    crate::query::edge_length(s, e)
}

#[test]
fn refused_edges() {
    let s = block();
    let edge = edges_where(&s, |p| near(p.x, 10.0) && near(p.y, 10.0));
    for b in [
        Blend::Fillet { radius: 0.0 },
        Blend::Fillet { radius: f64::NAN },
        Blend::Chamfer { distance: -1.0 },
    ] {
        assert!(matches!(
            blend(&s, &edge, b),
            Err(KernelError::InvalidInput(_))
        ));
    }
    assert!(blend(&s, &[], Blend::Fillet { radius: 1.0 }).is_err());
    assert!(blend(&s, &[EdgeId(99)], Blend::Fillet { radius: 1.0 }).is_err());
    // A cylinder's seam is not a corner.
    let plate = bossed_plate();
    let seam = plate
        .edge_ids()
        .find(|&e| {
            let edge = plate.edge(e);
            edge.coedges.len() == 2
                && plate.coedge_face(edge.coedges[0]) == plate.coedge_face(edge.coedges[1])
        })
        .unwrap();
    assert!(matches!(
        blend(&plate, &[seam], Blend::Fillet { radius: 1.0 }),
        Err(KernelError::Unsupported(_))
    ));
}
