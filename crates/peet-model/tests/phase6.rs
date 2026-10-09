//! Phase 6 solid modelling features end to end, through the rebuild engine: revolves,
//! fillets and chamfers, shells, draft, the hole wizard and imported bodies.

mod common;

use std::f64::consts::{PI, TAU};

use common::{FRONT, Part, TOP, assert_close, dimensioned_rectangle};
use peet_kernel::Surface;
use peet_kernel::primitive::cuboid;
use peet_kernel::validate::measure;
use peet_math::{DVec2, DVec3, Plane};
use peet_model::{
    AxisRef, BlendKind, EdgeRef, FaceRole, FeatureId, FeatureKind, HoleEnd, HoleFit, HoleKind,
    ImportedSolid, METRIC, Operation, Output, PatternDef, PlaneRef, RevolveAxisRef, Scalar, Status,
    StdAxis, StdPlane,
};
use peet_sketch::Sketch;

fn v2(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

fn v3(x: f64, y: f64, z: f64) -> DVec3 {
    DVec3::new(x, y, z)
}

fn polygon(s: &mut Sketch, points: &[DVec2]) {
    for i in 0..points.len() {
        s.add_line(points[i], points[(i + 1) % points.len()]);
    }
}

/// A W × D × H block from a dimensioned rectangle on the top plane (`d1` is W, `d2` D).
fn block(w: f64, d: f64, h: f64) -> (Part, FeatureId) {
    let mut p = Part::default();
    let sketch = p.sketch(TOP, |s| dimensioned_rectangle(s, w, d));
    p.extrude(sketch, Operation::NewBody, |e| e.depth = Scalar::new(h));
    p.rebuild();
    p.assert_ok();
    (p, sketch)
}

/// The edge of body 0 between two points (either way round).
#[track_caller]
fn edge(p: &Part, a: DVec3, b: DVec3) -> EdgeRef {
    let body = &p.bodies()[0];
    for e in body.solid.edge_ids() {
        let edge = body.solid.edge(e);
        let (s, t) = (
            body.solid.vertex(edge.start).point,
            body.solid.vertex(edge.end).point,
        );
        if (s.distance(a) < 1e-9 && t.distance(b) < 1e-9)
            || (s.distance(b) < 1e-9 && t.distance(a) < 1e-9)
        {
            return body.edge_ref(e).unwrap();
        }
    }
    panic!("no edge from {a} to {b}");
}

/// A sketch on `plane`, drawn in model coordinates.
fn sketch_at(
    p: &mut Part,
    plane: PlaneRef,
    draw: impl FnOnce(&mut Sketch, &dyn Fn(DVec3) -> DVec2),
) -> FeatureId {
    let id = p.sketch(plane, |_| {});
    p.rebuild();
    let Output::Sketch { plane, .. } = p.eval().output(id) else {
        panic!("not a sketch")
    };
    let to = move |q: DVec3| Plane::to_plane_coords(&plane, q);
    draw(p.sketch_mut(id), &to);
    id
}

fn revolve_mut(p: &mut Part, id: FeatureId) -> &mut peet_model::RevolveFeature {
    match &mut p.model.feature_mut(id).unwrap().kind {
        FeatureKind::Revolve(r) => r,
        _ => panic!("not a revolve"),
    }
}

fn hole_mut(p: &mut Part, id: FeatureId) -> &mut peet_model::HoleFeature {
    match &mut p.model.feature_mut(id).unwrap().kind {
        FeatureKind::Hole(h) => h,
        _ => panic!("not a hole"),
    }
}

fn blend_mut(p: &mut Part, id: FeatureId) -> &mut peet_model::BlendFeature {
    match &mut p.model.feature_mut(id).unwrap().kind {
        FeatureKind::Blend(b) => b,
        _ => panic!("not a blend"),
    }
}

#[track_caller]
fn failure(p: &Part, id: FeatureId) -> String {
    match p.status(id) {
        Status::Failed(m) => m.clone(),
        other => panic!("expected a failure, got {other:?}"),
    }
}

// ---- Revolve ----

#[test]
fn revolve_a_stepped_shaft_and_turn_a_groove() {
    let mut p = Part::default();
    // Half the section on the front plane: sketch x is the radius, sketch y the height.
    let profile = p.sketch(FRONT, |s| {
        polygon(
            s,
            &[
                v2(0.0, 0.0),
                v2(10.0, 0.0),
                v2(10.0, 30.0),
                v2(6.0, 30.0),
                v2(6.0, 50.0),
                v2(0.0, 50.0),
            ],
        );
    });
    let shaft = p.model.add_revolve(profile, Operation::NewBody);
    // No centreline in the sketch: about its vertical axis.
    assert_eq!(revolve_mut(&mut p, shaft).axis, RevolveAxisRef::SketchY);
    p.rebuild();
    p.assert_ok();
    let full = PI * (100.0 * 30.0 + 36.0 * 20.0);
    assert_close(p.volume(), full);
    assert_eq!(p.bodies().len(), 1);
    // A full turn has no caps; every face is named after a line of the profile.
    let body = &p.bodies()[0];
    assert!(body.face_names.iter().all(|n| {
        n.origins()
            .iter()
            .all(|o| o.feature == shaft && matches!(o.role, FaceRole::Side(_)))
    }));
    // The end of the shaft is a flat face you can sketch on.
    let top = p.face_ref(DVec3::Z, v3(0.0, 0.0, 50.0));

    // A groove, turned out of it about the same axis.
    let groove_sketch = p.sketch(FRONT, |s| {
        peet_sketch::shapes::rectangle(s, v2(8.0, 10.0), v2(12.0, 14.0));
    });
    let groove = p.model.add_revolve(groove_sketch, Operation::Cut);
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), full - PI * (100.0 - 64.0) * 4.0);
    assert_eq!(p.model.feature(groove).unwrap().name, "Cut-Revolve1");

    // Three quarters of a turn, then half a turn about the sketch plane.
    revolve_mut(&mut p, shaft).angle = Scalar::new(270.0);
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), 0.75 * (full - PI * 36.0 * 4.0));
    let caps = p.bodies()[0]
        .face_names
        .iter()
        .filter(|n| {
            n.origins()
                .iter()
                .any(|o| matches!(o.role, FaceRole::NearCap | FaceRole::FarCap))
        })
        .count();
    assert_eq!(caps, 2);
    revolve_mut(&mut p, shaft).angle = Scalar::new(180.0);
    revolve_mut(&mut p, shaft).symmetric = true;
    p.rebuild();
    p.assert_ok();
    let b = p.bodies()[0].solid.bounds();
    // The front plane's normal is −Y: half a turn either side of it fills x ≥ 0.
    assert!(
        b.min.x.abs() < 1e-9 && (b.max.x - 10.0).abs() < 1e-9,
        "{b:?}"
    );

    // The end face is still found after all that.
    revolve_mut(&mut p, shaft).angle = Scalar::new(360.0);
    p.rebuild();
    p.assert_ok();
    let found = peet_model::naming::find_face(p.bodies(), &top).expect("the end face");
    let Surface::Plane(plane) = p.bodies()[found.body].solid.face(found.id).surface else {
        panic!("the end face is flat");
    };
    assert!((plane.origin().z - 50.0).abs() < 1e-9);
}

#[test]
fn revolve_about_a_centreline_and_its_failures() {
    let mut p = Part::default();
    let profile = p.sketch(TOP, |s| {
        peet_sketch::shapes::rectangle(s, v2(20.0, 0.0), v2(30.0, 8.0));
        // A centreline along the sketch's X axis, 5 below it.
        let line = s.add_line(v2(-10.0, -5.0), v2(40.0, -5.0));
        s.set_construction(line, true);
    });
    let ring = p.model.add_revolve(profile, Operation::NewBody);
    assert!(matches!(
        revolve_mut(&mut p, ring).axis,
        RevolveAxisRef::SketchLine(_)
    ));
    p.rebuild();
    p.assert_ok();
    // Pappus: a 10 × 8 rectangle whose centre is 9 from the axis.
    assert_close(p.volume(), TAU * 9.0 * 80.0);
    // About a standard axis in the sketch plane.
    revolve_mut(&mut p, ring).axis =
        RevolveAxisRef::Axis(peet_model::AxisRef::Standard(peet_model::StdAxis::Y));
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), TAU * 25.0 * 80.0);
    // An axis that sticks out of the sketch plane is refused, with the way out.
    revolve_mut(&mut p, ring).axis =
        RevolveAxisRef::Axis(peet_model::AxisRef::Standard(peet_model::StdAxis::Z));
    p.rebuild();
    let m = failure(&p, ring);
    assert!(m.contains("must lie in the sketch's plane"), "{m}");
    // An axis through the profile.
    revolve_mut(&mut p, ring).axis = RevolveAxisRef::SketchX;
    p.rebuild();
    p.assert_ok(); // the rectangle touches the X axis: allowed
    assert_close(p.volume(), PI * 64.0 * 10.0);
    let s = p.sketch_mut(profile);
    for id in s.entities().map(|(id, _)| id).collect::<Vec<_>>() {
        if let Some(q) = s.try_point(id) {
            s.set_point(id, q - DVec2::new(0.0, 3.0));
        }
    }
    p.rebuild();
    let m = failure(&p, ring);
    assert!(m.contains("crosses the axis"), "{m}");
    // A zero angle.
    revolve_mut(&mut p, ring).axis = RevolveAxisRef::SketchY;
    revolve_mut(&mut p, ring).angle = Scalar::new(0.0);
    p.rebuild();
    assert!(failure(&p, ring).contains("greater than zero"));
}

// ---- Fillet and chamfer ----

#[test]
fn blends_follow_their_edges_through_edits() {
    let (mut p, sketch) = block(40.0, 30.0, 20.0);
    // The four upright edges, picked one by one.
    let corners = [(0.0, 0.0), (40.0, 0.0), (40.0, 30.0), (0.0, 30.0)];
    let uprights: Vec<EdgeRef> = corners
        .iter()
        .map(|&(x, y)| edge(&p, v3(x, y, 0.0), v3(x, y, 20.0)))
        .collect();
    let fillet = p.model.add_blend(BlendKind::Fillet, uprights);
    blend_mut(&mut p, fillet).size = Scalar::new(5.0);
    p.rebuild();
    p.assert_ok();
    let sliver = 25.0 * (1.0 - PI / 4.0);
    assert_close(p.volume(), (1200.0 - 4.0 * sliver) * 20.0);
    assert_eq!(p.bodies()[0].solid.faces.len(), 10);
    let fillets = p.bodies()[0]
        .face_names
        .iter()
        .filter(|n| {
            n.origins()
                .iter()
                .any(|o| o.feature == fillet && matches!(o.role, FaceRole::Blend(_)))
        })
        .count();
    assert_eq!(fillets, 4);

    // The block gets wider: the fillets stay on their edges.
    p.set_dimension(sketch, "d1", "60");
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), (1800.0 - 4.0 * sliver) * 20.0);

    // A chamfer round the top: one edge is picked, the smooth chain carries it on.
    let top_edge = edge(&p, v3(5.0, 0.0, 20.0), v3(55.0, 0.0, 20.0));
    let chamfer = p.model.add_blend(BlendKind::Chamfer, vec![top_edge]);
    blend_mut(&mut p, chamfer).size = Scalar::new(2.0);
    p.rebuild();
    p.assert_ok();
    let outline = 2.0 * (50.0 + 20.0) + TAU * 5.0;
    let removed = 2.0 * (outline - TAU * 2.0 / 3.0);
    assert_close(p.volume(), (1800.0 - 4.0 * sliver) * 20.0 - removed);
    // Without the chain: only the picked piece, ending in steps.
    blend_mut(&mut p, chamfer).chain = false;
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), (1800.0 - 4.0 * sliver) * 20.0 - 2.0 * 50.0);
    assert_eq!(p.model.feature(chamfer).unwrap().name, "Chamfer1");
    assert_eq!(p.model.feature(fillet).unwrap().name, "Fillet1");

    // A radius as an expression.
    p.model.parameters.set("r", "4").unwrap();
    let params = p.model.parameters.clone();
    blend_mut(&mut p, fillet)
        .size
        .set_input("r + 1", peet_model::ScalarKind::Length, &params)
        .unwrap();
    p.rebuild();
    p.assert_ok();
    p.model.parameters.set("r", "2").unwrap();
    p.rebuild();
    p.assert_ok();
    let sliver = 9.0 * (1.0 - PI / 4.0);
    assert_close(p.volume(), (1800.0 - 4.0 * sliver) * 20.0 - 2.0 * 54.0);
}

#[test]
fn blend_failures_say_what_to_do() {
    let (mut p, _) = block(40.0, 30.0, 20.0);
    let upright = edge(&p, v3(0.0, 0.0, 0.0), v3(0.0, 0.0, 20.0));
    let empty = p.model.add_blend(BlendKind::Fillet, Vec::new());
    p.rebuild();
    assert!(failure(&p, empty).contains("Pick the edges to fillet"));
    p.model.remove(empty);
    // Too big: the fillet would swallow the faces next to it.
    let fillet = p.model.add_blend(BlendKind::Fillet, vec![upright]);
    blend_mut(&mut p, fillet).size = Scalar::new(0.0);
    p.rebuild();
    assert!(failure(&p, fillet).contains("greater than zero"));
    // The bodies pass through a failed feature unchanged.
    assert_close(p.volume(), 24000.0);
    blend_mut(&mut p, fillet).size = Scalar::new(3.0);
    p.rebuild();
    p.assert_ok();
    // The edge goes away upstream.
    let first = p.model.features().next().unwrap().id;
    let extrude = p.model.features().nth(1).unwrap().id;
    p.model.remove(extrude);
    p.rebuild();
    let m = failure(&p, fillet);
    assert!(m.contains("no longer exists"), "{m}");
    assert!(p.status(first).is_built());
}

// ---- Shell and draft ----

#[test]
fn draft_then_shell_a_tub() {
    let (mut p, sketch) = block(60.0, 40.0, 30.0);
    let walls = [
        p.face_ref(DVec3::X, v3(60.0, 0.0, 0.0)),
        p.face_ref(-DVec3::X, v3(0.0, 0.0, 0.0)),
        p.face_ref(DVec3::Y, v3(0.0, 40.0, 0.0)),
        p.face_ref(-DVec3::Y, v3(0.0, 0.0, 0.0)),
    ];
    let draft = p
        .model
        .add_draft(walls.to_vec(), Some(PlaneRef::Standard(StdPlane::Top)));
    let FeatureKind::Draft(d) = &mut p.model.feature_mut(draft).unwrap().kind else {
        unreachable!()
    };
    d.angle = Scalar::new(5.0);
    p.rebuild();
    p.assert_ok();
    let tan = 5f64.to_radians().tan();
    // The section at height z, and the volume between two heights (Simpson is exact).
    let section =
        |z: f64, inset: f64| (60.0 - 2.0 * (z * tan + inset)) * (40.0 - 2.0 * (z * tan + inset));
    let between = |a: f64, b: f64, inset: f64| {
        (b - a) / 6.0
            * (section(a, inset) + 4.0 * section(0.5 * (a + b), inset) + section(b, inset))
    };
    assert_close(p.volume(), between(0.0, 30.0, 0.0));

    // Open at the top, walls 2 thick (measured square to the tilted walls).
    let top = p.face_ref(DVec3::Z, v3(10.0, 10.0, 30.0));
    let shell = p.model.add_shell(vec![top]);
    p.rebuild();
    p.assert_ok();
    let inset = 2.0 / 5f64.to_radians().cos();
    assert_close(
        p.volume(),
        between(0.0, 30.0, 0.0) - between(2.0, 30.0, inset),
    );
    // The inside walls are named after the faces they stand behind.
    let body = &p.bodies()[0];
    let inner: Vec<_> = body
        .face_names
        .iter()
        .filter(|n| n.origins().iter().any(|o| o.role == FaceRole::Inner))
        .collect();
    assert_eq!(inner.len(), 5);
    assert!(
        inner
            .iter()
            .all(|n| n.origins().len() == 2 && n.origins().iter().any(|o| o.feature == shell))
    );

    // A drain hole through the floor, sketched on the inside of the tub.
    let floor = p.on_face(DVec3::Z, v3(30.0, 20.0, 2.0));
    let drain = sketch_at(&mut p, floor, |s, to| {
        s.add_circle(to(v3(30.0, 20.0, 2.0)), 4.0);
    });
    p.extrude(drain, Operation::Cut, |e| e.depth = Scalar::new(2.0));
    p.rebuild();
    p.assert_ok();
    let with_hole = between(0.0, 30.0, 0.0) - between(2.0, 30.0, inset) - PI * 16.0 * 2.0;
    assert_close(p.volume(), with_hole);

    // The tub gets longer; everything follows, the drain included.
    p.set_dimension(sketch, "d1", "80");
    p.rebuild();
    p.assert_ok();
    assert!(p.volume() > with_hole);

    // Walls thicker than the tub can hold.
    let FeatureKind::Shell(s) = &mut p.model.feature_mut(shell).unwrap().kind else {
        unreachable!()
    };
    s.thickness = Scalar::new(25.0);
    p.rebuild();
    let m = failure(&p, shell);
    assert!(m.contains("too thick"), "{m}");
    // No neutral plane yet.
    let FeatureKind::Draft(d) = &mut p.model.feature_mut(draft).unwrap().kind else {
        unreachable!()
    };
    d.neutral = None;
    p.rebuild();
    assert!(failure(&p, draft).contains("Pick the neutral plane"));
}

// ---- Holes ----

/// A 60 × 40 × 10 plate with a hole sketch of three points on its top face.
fn drilled_plate() -> (Part, FeatureId, FeatureId) {
    let (mut p, _) = block(60.0, 40.0, 10.0);
    let top = p.on_face(DVec3::Z, v3(1.0, 1.0, 10.0));
    let sketch = sketch_at(&mut p, top, |s, to| {
        for x in [15.0, 30.0, 45.0] {
            s.add_point(to(v3(x, 20.0, 10.0)));
        }
    });
    let hole = p.model.add_hole(sketch);
    p.rebuild();
    p.assert_ok();
    (p, sketch, hole)
}

#[test]
fn hole_wizard_sizes_and_shapes() {
    let (mut p, sketch, hole) = drilled_plate();
    let plate = 60.0 * 40.0 * 10.0;
    // The default: M6, normal clearance, through.
    assert_close(p.volume(), plate - 3.0 * PI * 3.3 * 3.3 * 10.0);
    assert_eq!(hole_mut(&mut p, hole).summary(), "Ø6.6 hole");
    assert!(!p.model.feature(sketch).unwrap().visible);

    // Counterbored for M8 cap screws.
    let m8 = METRIC.iter().find(|s| s.name == "M8").unwrap();
    hole_mut(&mut p, hole).set_standard(m8, HoleFit::Normal);
    hole_mut(&mut p, hole).kind = HoleKind::Counterbore;
    p.rebuild();
    p.assert_ok();
    let one = PI * 7.5 * 7.5 * 8.6 + PI * 4.5 * 4.5 * 1.4;
    assert_close(p.volume(), plate - 3.0 * one);

    // Countersunk for M5: a 90° cone from Ø11.2 down to the Ø5.5 hole.
    let m5 = METRIC.iter().find(|s| s.name == "M5").unwrap();
    hole_mut(&mut p, hole).set_standard(m5, HoleFit::Normal);
    hole_mut(&mut p, hole).kind = HoleKind::Countersink;
    p.rebuild();
    p.assert_ok();
    let (r1, r2) = (5.6_f64, 2.75_f64);
    let h = r1 - r2;
    let cone = PI * h / 3.0 * (r1 * r1 + r1 * r2 + r2 * r2);
    assert_close(p.volume(), plate - 3.0 * (cone + PI * r2 * r2 * (10.0 - h)));
    assert!(
        p.bodies()[0]
            .solid
            .faces
            .iter()
            .any(|f| matches!(f.surface, Surface::Cone(_)))
    );

    // Tapped M6, blind, with a drill point.
    let m6 = METRIC.iter().find(|s| s.name == "M6").unwrap();
    {
        let h = hole_mut(&mut p, hole);
        h.set_standard(m6, HoleFit::Tapped);
        h.kind = HoleKind::Simple;
        h.end = HoleEnd::Blind;
        h.depth = Scalar::new(6.0);
        assert_eq!(h.thread.as_deref(), Some("M6x1"));
        assert_eq!(h.summary(), "M6x1 tapped hole");
    }
    p.rebuild();
    p.assert_ok();
    let point = 2.5 / 59f64.to_radians().tan();
    assert_close(
        p.volume(),
        plate - 3.0 * (PI * 6.25 * 6.0 + PI * 6.25 * point / 3.0),
    );
    // Each hole's faces are told apart, so one of them can be referred to.
    let names = &p.bodies()[0].face_names;
    let bores: Vec<_> = names
        .iter()
        .filter(|n| n.origins().iter().any(|o| o.feature == hole))
        .collect();
    assert_eq!(bores.len(), 6); // a wall and a point each
    let mut distinct = bores.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), 6);

    // The other way: out of the plate, where there is nothing to drill.
    hole_mut(&mut p, hole).reverse = true;
    p.rebuild();
    let m = failure(&p, hole);
    assert!(m.contains("None of the holes could be made"), "{m}");
    hole_mut(&mut p, hole).reverse = false;

    // A point off the plate is left out with a warning.
    let s = p.sketch_mut(sketch);
    s.add_point(v2(500.0, 500.0));
    p.rebuild();
    p.assert_ok();
    let Status::Warning(w) = p.status(hole) else {
        panic!("expected a warning");
    };
    assert!(w.contains("hole 4"), "{w}");

    // A counterbore that is no wider than the hole.
    {
        let h = hole_mut(&mut p, hole);
        h.kind = HoleKind::Counterbore;
        h.counterbore_diameter = Scalar::new(4.0);
    }
    p.rebuild();
    assert!(failure(&p, hole).contains("wider than the hole"));
}

#[test]
fn holes_at_circle_centres_and_empty_sketches() {
    let (mut p, _) = block(60.0, 40.0, 10.0);
    let top = p.on_face(DVec3::Z, v3(1.0, 1.0, 10.0));
    let sketch = sketch_at(&mut p, top, |s, to| {
        s.add_circle(to(v3(20.0, 20.0, 10.0)), 3.0);
        s.add_circle(to(v3(40.0, 20.0, 10.0)), 3.0);
    });
    let hole = p.model.add_hole(sketch);
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), 24000.0 - 2.0 * PI * 3.3 * 3.3 * 10.0);
    let empty = p.sketch(TOP, |_| {});
    let none = p.model.add_hole(empty);
    p.rebuild();
    assert!(failure(&p, none).contains("no points to drill at"));
    assert!(p.status(hole).is_built());
}

#[test]
fn standard_sizes_are_consistent() {
    for (i, s) in METRIC.iter().enumerate() {
        assert!(s.tap_drill < s.diameter, "{}", s.name);
        assert!(
            (s.tap_drill - (s.diameter - s.pitch)).abs() < 0.11,
            "{}",
            s.name
        );
        assert!(s.diameter < s.clearance[0], "{}", s.name);
        assert!(s.clearance[0] < s.clearance[1] && s.clearance[1] <= s.clearance[2]);
        assert!(s.counterbore_diameter > s.clearance[2], "{}", s.name);
        assert!(s.counterbore_depth > s.diameter, "{}", s.name);
        assert!(s.countersink_diameter > s.clearance[2], "{}", s.name);
        if i > 0 {
            assert!(METRIC[i - 1].diameter < s.diameter);
        }
    }
    assert_eq!(METRIC[5].thread(), "M6x1");
    assert_eq!(METRIC[6].thread(), "M8x1.25");
}

// ---- Imported bodies ----

#[test]
fn imported_bodies_take_features() {
    let mut p = Part::default();
    let solids = vec![
        ImportedSolid {
            name: "block".to_owned(),
            solid: cuboid(DVec3::ZERO, v3(30.0, 20.0, 10.0)),
        },
        ImportedSolid {
            name: "pin".to_owned(),
            solid: cuboid(v3(50.0, 0.0, 0.0), v3(55.0, 5.0, 40.0)),
        },
    ];
    let import = p.model.add_import("bracket.step".to_owned(), solids);
    p.rebuild();
    p.assert_ok();
    assert_eq!(p.bodies().len(), 2);
    assert_close(p.volume(), 6000.0 + 1000.0);
    assert_eq!(p.model.feature(import).unwrap().name, "Imported1");
    // Its faces can be sketched on, drilled and rounded like any other.
    let top = p.on_face(DVec3::Z, v3(1.0, 1.0, 10.0));
    let sketch = sketch_at(&mut p, top, |s, to| {
        s.add_point(to(v3(15.0, 10.0, 10.0)));
    });
    let hole = p.model.add_hole(sketch);
    let upright = edge(&p, v3(0.0, 0.0, 0.0), v3(0.0, 0.0, 10.0));
    p.model.add_blend(BlendKind::Chamfer, vec![upright]);
    p.rebuild();
    p.assert_ok();
    assert!(p.status(hole).is_built());
    assert_close(p.volume(), 7000.0 - PI * 3.3 * 3.3 * 10.0 - 0.5 * 10.0);
    // The second solid's faces are told apart from the first's.
    let (a, b) = (&p.bodies()[0], &p.bodies()[1]);
    assert!(a.face_names.iter().all(|n| !b.face_names.contains(n)));
    // An import with nothing in it.
    let empty = p.model.add_import("empty.step".to_owned(), Vec::new());
    p.rebuild();
    assert!(failure(&p, empty).contains("no solid bodies"));
}

// ---- Rebuilds ----

#[test]
fn only_what_changed_is_rebuilt() {
    let (mut p, _, hole) = drilled_plate();
    let upright = edge(&p, v3(0.0, 0.0, 0.0), v3(0.0, 0.0, 10.0));
    let fillet = p.model.add_blend(BlendKind::Fillet, vec![upright]);
    p.rebuild();
    p.assert_ok();
    // Nothing changed: nothing is recomputed.
    assert_eq!(p.rebuild().stats.rebuilt, 0);
    // The fillet's size: only the fillet.
    blend_mut(&mut p, fillet).size = Scalar::new(3.0);
    let stats = p.rebuild().stats;
    assert_eq!(stats.rebuilt, 1);
    // The hole: the hole and the fillet after it.
    hole_mut(&mut p, hole).diameter = Scalar::new(5.0);
    let stats = p.rebuild().stats;
    assert_eq!(stats.rebuilt, 2);
    p.assert_ok();
    let mass = peet_kernel::query::mass_properties(&p.bodies()[0].solid).unwrap();
    assert_close(mass.volume, measure::volume(&p.bodies()[0].solid));
    assert!((mass.centroid.z - 5.0).abs() < 1e-3);
}

// ---- Copies ----

#[test]
fn bolt_circle_and_mirrored_revolve() {
    // A flange: a disc turned about the Z axis.
    let mut p = Part::default();
    let profile = p.sketch(FRONT, |s| {
        polygon(
            s,
            &[v2(0.0, 0.0), v2(40.0, 0.0), v2(40.0, 10.0), v2(0.0, 10.0)],
        );
    });
    p.model.add_revolve(profile, Operation::NewBody);
    p.rebuild();
    p.assert_ok();
    let disc = PI * 1600.0 * 10.0;
    // One hole, then five more round the axis.
    let top = p.on_face(DVec3::Z, v3(0.0, 0.0, 10.0));
    let sketch = sketch_at(&mut p, top, |s, to| {
        s.add_point(to(v3(28.0, 0.0, 10.0)));
    });
    let hole = p.model.add_hole(sketch);
    let circle = p.model.add_pattern(
        vec![hole],
        PatternDef::Circular {
            axis: AxisRef::Standard(StdAxis::Z),
            count: 6,
            angle: Scalar::new(360.0),
            flip: false,
        },
    );
    p.rebuild();
    p.assert_ok();
    let bore = PI * 3.3 * 3.3 * 10.0;
    assert_close(p.volume(), disc - 6.0 * bore);
    // Every hole's wall has a name of its own.
    let mut walls: Vec<_> = p.bodies()[0]
        .face_names
        .iter()
        .filter(|n| {
            n.origins()
                .iter()
                .any(|o| o.feature == hole || o.feature == circle)
        })
        .collect();
    assert_eq!(walls.len(), 6);
    walls.sort();
    walls.dedup();
    assert_eq!(walls.len(), 6);
    // The holes follow their seed: counterbored, all six.
    hole_mut(&mut p, hole).kind = HoleKind::Counterbore;
    p.rebuild();
    p.assert_ok();
    let counterbored = PI * 5.5 * 5.5 * 6.4 + PI * 3.3 * 3.3 * 3.6;
    assert_close(p.volume(), disc - 6.0 * counterbored);

    // A boss turned about a line of its own sketch, and its mirror image.
    let boss_sketch = p.sketch(FRONT, |s| {
        peet_sketch::shapes::rectangle(s, v2(10.0, 10.0), v2(16.0, 18.0));
        let axis = s.add_line(v2(13.0, 0.0), v2(13.0, 30.0));
        s.set_construction(axis, true);
    });
    let boss = p.model.add_revolve(boss_sketch, Operation::Add);
    p.rebuild();
    // The axis runs through the middle of the rectangle: only one side of it may be
    // revolved. The message is a sentence that says so.
    let m = failure(&p, boss);
    assert!(m.starts_with("The profile crosses the axis"), "{m}");
    assert!(m.ends_with('.'), "{m}");
    let s = p.sketch_mut(boss_sketch);
    *s = Sketch::new();
    peet_sketch::shapes::rectangle(s, v2(13.0, 10.0), v2(16.0, 18.0));
    let axis = s.add_line(v2(13.0, 0.0), v2(13.0, 30.0));
    s.set_construction(axis, true);
    let axis = peet_model::default_axis(&p.model.sketch(boss_sketch).unwrap().sketch);
    revolve_mut(&mut p, boss).axis = axis;
    p.rebuild();
    p.assert_ok();
    let rod = PI * 9.0 * 8.0;
    assert_close(p.volume(), disc - 6.0 * counterbored + rod);
    p.model
        .add_mirror(vec![boss], PlaneRef::Standard(StdPlane::Right));
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), disc - 6.0 * counterbored + 2.0 * rod);
    let b = p.bodies()[0].solid.bounds();
    assert!((b.max.z - 18.0).abs() < 1e-9, "{b:?}");
}

// ---- The sample part ----

#[test]
fn housing_sample_matches_its_hand_calculation() {
    let (mut model, mut engine) = peet_model::samples::housing();
    let eval = engine.regenerate(&mut model);
    for f in model.features() {
        assert!(
            eval.status(f.id).is_some_and(Status::is_built),
            "{}: {:?}",
            f.name,
            eval.status(f.id)
        );
    }
    assert_eq!(eval.bodies.len(), 1);
    let solid = &eval.bodies[0].solid;
    assert!(peet_kernel::validate::validate(solid).is_ok());
    assert_close(
        measure::volume(solid),
        peet_model::samples::housing_volume(),
    );
    // Turned parts have their centre of gravity on the axis.
    let mass = peet_kernel::query::mass_properties(solid).unwrap();
    assert!(mass.centroid.x.abs() < 1e-3 && mass.centroid.y.abs() < 1e-3);
    // Planes, cylinders, a torus (the fillet) and cones (the chamfers).
    let has = |f: fn(&Surface) -> bool| solid.faces.iter().any(|face| f(&face.surface));
    assert!(has(|s| matches!(s, Surface::Torus(_))));
    assert!(has(|s| matches!(s, Surface::Cone(_))));
    assert!(has(|s| matches!(s, Surface::Cylinder(_))));
    // Nothing is recomputed on a second rebuild.
    assert_eq!(engine.regenerate(&mut model).stats.rebuilt, 0);
    // From scratch.
    engine.clear();
    let stats = engine.regenerate(&mut model).stats;
    assert_eq!(stats.rebuilt, model.len());
    eprintln!(
        "housing: {} features rebuilt in {:.1} ms",
        stats.rebuilt, stats.ms
    );
}

// ---- Sweep ----

fn sweep_mut(p: &mut Part, id: FeatureId) -> &mut peet_model::SweepFeature {
    match &mut p.model.feature_mut(id).unwrap().kind {
        FeatureKind::Sweep(s) => s,
        _ => panic!("not a sweep"),
    }
}

#[test]
fn sweep_a_pipe_along_lines_and_bends() {
    let mut p = Part::default();
    // A tube section on the right plane (the YZ plane), centred on the origin.
    let section = p.sketch(PlaneRef::Standard(StdPlane::Right), |s| {
        s.add_circle(DVec2::ZERO, 5.0);
        s.add_circle(DVec2::ZERO, 3.0);
    });
    // The path on the top plane: out along X, a quarter turn of radius 20, on along Y.
    let path = p.sketch(TOP, |s| {
        s.add_line(v2(0.0, 0.0), v2(30.0, 0.0));
        s.add_arc(v2(30.0, 20.0), v2(30.0, 0.0), v2(50.0, 20.0));
        s.add_line(v2(50.0, 20.0), v2(50.0, 45.0));
    });
    let sweep = p.model.add_sweep(section, Some(path), Operation::NewBody);
    p.rebuild();
    p.assert_ok();
    let ring = PI * (25.0 - 9.0);
    let length = 30.0 + 0.5 * PI * 20.0 + 25.0;
    assert_close(p.volume(), ring * length);
    assert_eq!(p.bodies().len(), 1);
    let b = p.bodies()[0].solid.bounds();
    // Curved faces' bounds are a hair generous.
    assert!(
        (b.max.y - 45.0).abs() < 1e-9 && (b.max.x - 55.0).abs() < 0.05,
        "{b:?}"
    );
    // The bend is a torus, inside and out; the ends are the profile.
    let tori = p.bodies()[0]
        .solid
        .faces
        .iter()
        .filter(|f| matches!(f.surface, Surface::Torus(_)))
        .count();
    assert_eq!(tori, 2);
    assert!(!p.model.feature(path).unwrap().visible);
    // Its faces are told apart, piece by piece.
    let mut names = p.bodies()[0].face_names.clone();
    let faces = names.len();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), faces);

    // A sharper bend than the pipe is wide.
    let s = p.sketch_mut(path);
    *s = Sketch::new();
    s.add_line(v2(0.0, 0.0), v2(30.0, 0.0));
    s.add_arc(v2(30.0, 4.0), v2(30.0, 0.0), v2(34.0, 4.0));
    p.rebuild();
    let m = failure(&p, sweep);
    assert!(m.contains("doesn't fit round a bend"), "{m}");
    // A square corner between straight pieces is mitred: the pipe's centreline is 50
    // long, and a mitre neither adds nor removes volume.
    let s = p.sketch_mut(path);
    *s = Sketch::new();
    s.add_line(v2(0.0, 0.0), v2(30.0, 0.0));
    s.add_line(v2(30.0, 0.0), v2(30.0, 20.0));
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), ring * 50.0);
    // The two legs' walls meet in ellipses.
    assert!(
        p.bodies()[0]
            .solid
            .edges
            .iter()
            .any(|e| matches!(e.curve, peet_kernel::Curve3::Ellipse(_)))
    );
    // Three legs at other angles.
    let s = p.sketch_mut(path);
    *s = Sketch::new();
    s.add_line(v2(0.0, 0.0), v2(30.0, 0.0));
    s.add_line(v2(30.0, 0.0), v2(50.0, 20.0));
    s.add_line(v2(50.0, 20.0), v2(50.0, 60.0));
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), ring * (30.0 + 800f64.sqrt() + 40.0));
    // A corner at an arc can't be mitred.
    let s = p.sketch_mut(path);
    *s = Sketch::new();
    s.add_line(v2(0.0, 0.0), v2(30.0, 0.0));
    s.add_arc(v2(50.0, 0.0), v2(30.0, 0.0), v2(50.0, 20.0));
    p.rebuild();
    assert!(failure(&p, sweep).contains("corner where an arc meets it"));
    // A path that starts somewhere else.
    let s = p.sketch_mut(path);
    *s = Sketch::new();
    s.add_line(v2(5.0, 0.0), v2(30.0, 0.0));
    p.rebuild();
    assert!(failure(&p, sweep).contains("must start on the profile's plane"));
    // No path yet.
    sweep_mut(&mut p, sweep).path = None;
    p.rebuild();
    assert!(failure(&p, sweep).contains("Pick the path"));
}

#[test]
fn sweep_round_a_circle_and_cut_a_groove() {
    // A block with a channel swept through its top: a rectangular section along a line.
    let (mut p, _) = block(60.0, 40.0, 20.0);
    let end = p.on_face(-DVec3::Y, v3(1.0, 0.0, 1.0));
    let section = sketch_at(&mut p, end, |s, to| {
        let (a, b) = (to(v3(25.0, 0.0, 14.0)), to(v3(35.0, 0.0, 20.0)));
        peet_sketch::shapes::rectangle(s, a.min(b), a.max(b));
    });
    let path = p.sketch(TOP, |s| {
        s.add_line(v2(30.0, 0.0), v2(30.0, 40.0));
    });
    p.model.add_sweep(section, Some(path), Operation::Cut);
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), 48000.0 - 10.0 * 6.0 * 40.0);

    // An O-ring: a circle swept round a circle.
    let mut p = Part::default();
    let section = p.sketch(FRONT, |s| {
        s.add_circle(v2(20.0, 0.0), 2.0);
    });
    let path = p.sketch(TOP, |s| {
        s.add_circle(DVec2::ZERO, 20.0);
    });
    p.model.add_sweep(section, Some(path), Operation::NewBody);
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), 2.0 * PI * PI * 20.0 * 4.0);
}

// ---- Loft ----

#[test]
fn loft_between_sketches_on_offset_planes() {
    let mut p = Part::default();
    let base = p.sketch(TOP, |s| {
        peet_sketch::shapes::rectangle(s, v2(-20.0, -15.0), v2(20.0, 15.0));
    });
    let plane = p
        .model
        .add(FeatureKind::Plane(peet_model::PlaneDef::Offset {
            from: PlaneRef::Standard(StdPlane::Top),
            distance: Scalar::new(30.0),
            flip: false,
        }));
    let top = p.sketch(PlaneRef::Feature(plane), |s| {
        s.add_circle(DVec2::ZERO, 8.0);
    });
    let loft = p.model.add_loft(vec![base, top], Operation::NewBody);
    p.rebuild();
    p.assert_ok();
    assert_eq!(p.bodies().len(), 1);
    let solid = &p.bodies()[0].solid;
    // Four freeform sides between the rectangle's edges and the circle's quarters.
    let sides = solid
        .faces
        .iter()
        .filter(|f| matches!(f.surface, Surface::Nurbs(_)))
        .count();
    assert_eq!(sides, 4);
    let volume = p.volume();
    assert!(volume > 30.0 * 128.0 && volume < 30.0 * 1200.0, "{volume}");
    assert!(!p.model.feature(top).unwrap().visible);
    // Named after the first profile's curves, and the caps.
    let names = &p.bodies()[0].face_names;
    assert!(
        names
            .iter()
            .any(|n| n.origins()[0].role == FaceRole::NearCap)
    );
    assert!(
        names
            .iter()
            .any(|n| n.origins()[0].role == FaceRole::FarCap)
    );
    assert_eq!(
        names
            .iter()
            .filter(|n| matches!(n.origins()[0].role, FaceRole::Side(_)))
            .count(),
        4
    );
    // The plane moves: the loft follows, and its volume with it.
    let FeatureKind::Plane(peet_model::PlaneDef::Offset { distance, .. }) =
        &mut p.model.feature_mut(plane).unwrap().kind
    else {
        unreachable!()
    };
    *distance = Scalar::new(60.0);
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), 2.0 * volume);

    // A third profile makes it smooth through all three.
    let plane2 = p
        .model
        .add(FeatureKind::Plane(peet_model::PlaneDef::Offset {
            from: PlaneRef::Standard(StdPlane::Top),
            distance: Scalar::new(90.0),
            flip: false,
        }));
    let end = p.sketch(PlaneRef::Feature(plane2), |s| {
        peet_sketch::shapes::rectangle(s, v2(-10.0, -10.0), v2(10.0, 10.0));
    });
    // The loft comes before the new sketch in the tree: move it to the end.
    let last = p.model.len() - 1;
    let FeatureKind::Loft(l) = &mut p.model.feature_mut(loft).unwrap().kind else {
        unreachable!()
    };
    l.sections.push(end);
    p.model.move_to(loft, last).unwrap();
    p.rebuild();
    p.assert_ok();
    let b = p.bodies()[0].solid.bounds();
    // Freeform faces' bounds are a hair generous.
    assert!(
        (b.max.z - 90.0).abs() < 0.05 && b.min.z.abs() < 0.05,
        "{b:?}"
    );

    // Failures say what to do.
    let FeatureKind::Loft(l) = &mut p.model.feature_mut(loft).unwrap().kind else {
        unreachable!()
    };
    l.sections.truncate(1);
    p.rebuild();
    assert!(failure(&p, loft).contains("at least two profile sketches"));
    let tri = p.sketch(PlaneRef::Feature(plane), |s| {
        polygon(s, &[v2(0.0, 0.0), v2(10.0, 0.0), v2(0.0, 10.0)]);
    });
    let last = p.model.len() - 1;
    let FeatureKind::Loft(l) = &mut p.model.feature_mut(loft).unwrap().kind else {
        unreachable!()
    };
    l.sections = vec![base, tri];
    p.model.move_to(loft, last).unwrap();
    p.rebuild();
    let m = failure(&p, loft);
    assert!(m.contains("same number of edges"), "{m}");
}

/// A loft from a rectangle on `low` up to a circle `rise` above it.
fn funnel(p: &mut Part, low: f64, rise: f64, operation: Operation) -> FeatureId {
    let plane_at = |p: &mut Part, distance: f64| {
        p.model
            .add(FeatureKind::Plane(peet_model::PlaneDef::Offset {
                from: PlaneRef::Standard(StdPlane::Top),
                distance: Scalar::new(distance),
                flip: false,
            }))
    };
    let a = plane_at(p, low);
    let b = plane_at(p, low + rise);
    let base = p.sketch(PlaneRef::Feature(a), |s| {
        peet_sketch::shapes::rectangle(s, v2(18.0, 11.0), v2(42.0, 29.0));
    });
    let top = p.sketch(PlaneRef::Feature(b), |s| {
        s.add_circle(v2(30.0, 20.0), 6.0);
    });
    p.model.add_loft(vec![base, top], operation)
}

#[test]
fn lofts_join_and_cut_bodies_and_take_features() {
    // The funnel alone.
    let mut alone = Part::default();
    funnel(&mut alone, 10.0, 20.0, Operation::NewBody);
    alone.rebuild();
    alone.assert_ok();
    let funnel_volume = alone.volume();

    // On top of a block it joins: one body, the two volumes together.
    let (mut p, _) = block(60.0, 40.0, 10.0);
    let block_volume = p.volume();
    let boss = funnel(&mut p, 10.0, 20.0, Operation::Add);
    p.rebuild();
    p.assert_ok();
    assert_eq!(p.bodies().len(), 1);
    assert!(
        (p.volume() - block_volume - funnel_volume).abs() < 1e-3 * funnel_volume,
        "{} vs {}",
        p.volume(),
        block_volume + funnel_volume
    );
    // A hole drilled down through the boss's flat top and the block.
    let (_, top) = p.face(DVec3::Z, v3(30.0, 20.0, 30.0));
    assert_eq!(
        p.bodies()[0].face_names[top.index()].origins()[0].feature,
        boss
    );
    let top = p.on_face(DVec3::Z, v3(30.0, 20.0, 30.0));
    let bore = sketch_at(&mut p, top, |s, to| {
        s.add_circle(to(v3(30.0, 20.0, 30.0)), 3.0);
    });
    p.extrude(bore, Operation::Cut, |e| {
        e.end = peet_model::EndCondition::ThroughAll;
    });
    p.rebuild();
    p.assert_ok();
    let expected = block_volume + funnel_volume - PI * 9.0 * 30.0;
    assert!((p.volume() - expected).abs() < 1e-3 * funnel_volume);

    // Sunk into a thick block it cuts a pocket of the funnel's shape.
    let (mut p, _) = block(60.0, 40.0, 30.0);
    let block_volume = p.volume();
    funnel(&mut p, 10.0, 20.0, Operation::Cut);
    p.rebuild();
    p.assert_ok();
    assert_eq!(p.bodies().len(), 1);
    assert!(
        (p.volume() - block_volume + funnel_volume).abs() < 1e-3 * funnel_volume,
        "{} vs {}",
        p.volume(),
        block_volume - funnel_volume
    );
}

#[test]
fn a_hole_drilled_across_a_hole_of_another_size() {
    // The two bores cross in curves with no closed form, which the kernel traces. What
    // they share is counted once: 8 ∫₀⁴ √(16 − t²) √(36 − t²) dt.
    let (mut p, _) = block(60.0, 40.0, 30.0);
    let top = p.on_face(DVec3::Z, v3(30.0, 20.0, 30.0));
    let down = sketch_at(&mut p, top, |s, to| {
        s.add_circle(to(v3(30.0, 20.0, 30.0)), 6.0);
    });
    p.extrude(down, Operation::Cut, |e| {
        e.end = peet_model::EndCondition::ThroughAll;
    });
    p.rebuild();
    p.assert_ok();
    let side = p.on_face(DVec3::X, v3(60.0, 20.0, 15.0));
    let across = sketch_at(&mut p, side, |s, to| {
        s.add_circle(to(v3(60.0, 20.0, 15.0)), 4.0);
    });
    p.extrude(across, Operation::Cut, |e| {
        e.end = peet_model::EndCondition::ThroughAll;
    });
    p.rebuild();
    p.assert_ok();
    let n = 200_000;
    let shared: f64 = (0..n)
        .map(|k| {
            let t = 4.0 * (f64::from(k) + 0.5) / f64::from(n);
            8.0 * ((16.0 - t * t) * (36.0 - t * t)).sqrt() * 4.0 / f64::from(n)
        })
        .sum();
    let expected = 60.0 * 40.0 * 30.0 - PI * 36.0 * 30.0 - PI * 16.0 * 60.0 + shared;
    let volume = p.volume();
    assert!(
        (volume - expected).abs() < 1e-5 * expected,
        "{volume} instead of {expected}"
    );
    // The rims where they cross are freeform curves on round faces.
    assert!(
        p.bodies()[0]
            .solid
            .edges
            .iter()
            .any(|e| matches!(e.curve, peet_kernel::Curve3::Nurbs(_)))
    );
}
