//! Phase 5 sheet metal and copy features end to end, through the rebuild engine: hems,
//! sketched bends and jogs on faces, mitre flanges from a sketched profile, corner
//! treatments, forms, and patterns and mirrors of cuts.

mod common;

use std::f64::consts::PI;

use common::{Part, TOP, assert_close, dimensioned_rectangle};
use peet_math::{DVec2, DVec3, Plane};
use peet_model::{
    AxisRef, BendModelDef, EdgeRef, FeatureId, FeatureKind, LinearDirection, Operation, Output,
    PatternDef, PlaneDef, PlaneRef, Scalar, Status, StdAxis, StdPlane,
};
use peet_sheetmetal::corner::CornerKind;
use peet_sheetmetal::{FormKind, HemKind};
use peet_sketch::{Sketch, shapes};

const W: f64 = 100.0;
const H: f64 = 60.0;
const T: f64 = 2.0;
const R: f64 = 3.0;

/// A W × H plate (t = 2, R = 3, K = 0.5) on the top plane.
fn plate() -> (Part, FeatureId) {
    let mut p = Part::default();
    let sketch = p.sketch(TOP, |s| dimensioned_rectangle(s, W, H));
    let flange = p.model.add_base_flange(sketch);
    let FeatureKind::BaseFlange(b) = &mut p.model.feature_mut(flange).unwrap().kind else {
        unreachable!()
    };
    b.settings.thickness = Scalar::new(T);
    b.settings.radius = Scalar::new(R);
    b.settings.model = BendModelDef::KFactor(Scalar::new(0.5));
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

/// A sketch on `plane`, drawn in model coordinates: `draw` gets the sketch and a map from
/// model points to sketch points.
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

fn bounds(p: &Part) -> (DVec3, DVec3) {
    let mut lo = DVec3::splat(f64::INFINITY);
    let mut hi = DVec3::splat(f64::NEG_INFINITY);
    for b in p.bodies() {
        for e in &b.solid.edges {
            for i in 0..=128 {
                let q = e.point_at_fraction(f64::from(i) / 128.0);
                lo = lo.min(q);
                hi = hi.max(q);
            }
        }
    }
    (lo, hi)
}

fn flat_area(p: &Part) -> f64 {
    let sheet = p.bodies()[0].sheet.clone().unwrap();
    peet_kernel::validate::measure::volume(&sheet.flat) / T
}

#[test]
fn hem_follows_its_edge() {
    let (mut p, sketch) = plate();
    let e = edge(&p, DVec3::new(0.0, H, T), DVec3::new(W, H, T));
    let hem = p.model.add_hem(Some(e));
    p.rebuild();
    p.assert_ok();
    // Closed, inside: the outline keeps its size; the hem lies on top.
    let (lo, hi) = bounds(&p);
    assert!(lo.abs_diff_eq(DVec3::ZERO, 1e-9), "{lo}");
    assert_close(hi.y, H);
    assert!(hi.z > 2.0 * T && hi.z < 2.0 * T + 0.1, "{hi}");
    // An open rolled hem, after the plate gets wider.
    let FeatureKind::Hem(h) = &mut p.model.feature_mut(hem).unwrap().kind else {
        unreachable!()
    };
    h.kind = HemKind::Rolled;
    h.radius = Scalar::new(4.0);
    h.angle = Scalar::new(250.0);
    p.set_dimension(sketch, "d1", "140");
    p.rebuild();
    p.assert_ok();
    let (_, hi) = bounds(&p);
    assert_close(hi.x, 140.0);
    assert_close(p.volume(), flat_area(&p) * T);
}

#[test]
fn sketched_bend_and_jog_on_a_face() {
    let (mut p, _) = plate();
    let top = p.on_face(DVec3::Z, DVec3::new(50.0, 30.0, T));
    let line = sketch_at(&mut p, top.clone(), |s, to| {
        s.add_line(
            to(DVec3::new(70.0, -5.0, T)),
            to(DVec3::new(70.0, H + 5.0, T)),
        );
    });
    let bend = p.model.add_sketched_bend(line);
    p.rebuild();
    p.assert_ok();
    // The right 30 stand up (towards the sketched face); the volume is kept (K = 0.5).
    assert_close(p.volume(), W * H * T);
    let (_, hi) = bounds(&p);
    assert!(hi.z > 20.0, "{hi}");
    assert!(hi.x < W - 20.0, "{hi}");
    // Flip it: down instead.
    let FeatureKind::SketchedBend(b) = &mut p.model.feature_mut(bend).unwrap().kind else {
        unreachable!()
    };
    b.flip = true;
    p.rebuild();
    p.assert_ok();
    let (lo, _) = bounds(&p);
    assert!(lo.z < -20.0, "{lo}");

    // A jog on a fresh plate.
    let (mut p, _) = plate();
    let top = p.on_face(DVec3::Z, DVec3::new(50.0, 30.0, T));
    let line = sketch_at(&mut p, top, |s, to| {
        s.add_line(
            to(DVec3::new(60.0, -5.0, T)),
            to(DVec3::new(60.0, H + 5.0, T)),
        );
    });
    let jog = p.model.add_jog(line);
    let FeatureKind::Jog(j) = &mut p.model.feature_mut(jog).unwrap().kind else {
        unreachable!()
    };
    j.offset = Scalar::new(15.0);
    p.rebuild();
    p.assert_ok();
    let (_, hi) = bounds(&p);
    // The far side's top face, 15 up.
    assert_close(hi.z, 15.0 + T);
    assert_close(p.volume(), W * H * T);
}

#[test]
fn miter_flange_around_the_plate() {
    let (mut p, _) = plate();
    // The four top edges.
    let c = [
        DVec3::new(0.0, 0.0, T),
        DVec3::new(W, 0.0, T),
        DVec3::new(W, H, T),
        DVec3::new(0.0, H, T),
    ];
    let edges: Vec<EdgeRef> = (0..4).map(|i| edge(&p, c[i], c[(i + 1) % 4])).collect();
    // The profile, on the plate's left end face (x = 0), square to the front edge
    // (y = 0): up 20 from the bottom corner, then a lip 8 back over the plate.
    let end = p.on_face(-DVec3::X, DVec3::new(0.0, 30.0, 1.0));
    let profile = sketch_at(&mut p, end, |s, to| {
        let a = to(DVec3::new(0.0, 0.0, 0.0));
        let b = to(DVec3::new(0.0, 0.0, 20.0));
        let c = to(DVec3::new(0.0, 8.0, 20.0));
        s.add_line(a, b);
        s.add_line(b, c);
    });
    let m = p.model.add_miter_flange(profile, edges);
    p.rebuild();
    p.assert_ok();
    assert!(matches!(p.status(m), Status::Ok), "{:?}", p.status(m));
    let (lo, hi) = bounds(&p);
    assert!(lo.abs_diff_eq(DVec3::ZERO, 1e-9), "{lo}");
    assert!(hi.abs_diff_eq(DVec3::new(W, H, 20.0), 1e-9), "{hi}");
    let sheet = p.bodies()[0].sheet.clone().unwrap();
    assert_eq!(sheet.bend_lines.len(), 8);
}

#[test]
fn corner_treatment_changes_every_corner() {
    let (mut p, _) = plate();
    let c = [
        DVec3::new(0.0, 0.0, T),
        DVec3::new(W, 0.0, T),
        DVec3::new(W, H, T),
        DVec3::new(0.0, H, T),
    ];
    for i in 0..4 {
        let e = edge(&p, c[i], c[(i + 1) % 4]);
        p.model.add_edge_flange(Some(e));
    }
    p.rebuild();
    p.assert_ok();
    let butt = p.volume();
    let corner = p.model.add_corner(Vec::new());
    let FeatureKind::Corner(k) = &mut p.model.feature_mut(corner).unwrap().kind else {
        unreachable!()
    };
    k.kind = CornerKind::Open;
    k.gap = Scalar::new(1.0);
    p.rebuild();
    p.assert_ok();
    // Open corners: every flange gives up its ends, so there is less material.
    assert!(p.volume() < butt - 1.0, "{} {butt}", p.volume());
    let sheet = p.bodies()[0].sheet.clone().unwrap();
    assert!(
        sheet
            .layout
            .corners
            .iter()
            .all(|c| c.spec.kind == CornerKind::Open)
    );
}

#[test]
fn dimples_from_a_sketch() {
    let (mut p, _) = plate();
    let top = p.on_face(DVec3::Z, DVec3::new(50.0, 30.0, T));
    let circles = sketch_at(&mut p, top, |s, to| {
        s.add_circle(to(DVec3::new(25.0, 30.0, T)), 8.0);
        s.add_circle(to(DVec3::new(75.0, 30.0, T)), 8.0);
    });
    let form = p.model.add_form(circles, FormKind::Dimple);
    p.rebuild();
    p.assert_ok();
    let h = 3.0;
    let ring = PI * (64.0 - 36.0);
    assert_close(p.volume(), W * H * T + 2.0 * ring * h);
    let (_, hi) = bounds(&p);
    // The plateau's outer face stands the height above the sheet's top face.
    assert_close(hi.z, T + h);
    assert!(matches!(p.status(form), Status::Ok));
    let sheet = p.bodies()[0].sheet.clone().unwrap();
    assert_eq!(sheet.forms.len(), 2);
}

#[test]
fn pattern_and_mirror_of_sheet_cuts() {
    let (mut p, _) = plate();
    let top = p.on_face(DVec3::Z, DVec3::new(50.0, 30.0, T));
    let hole = sketch_at(&mut p, top, |s, to| {
        s.add_circle(to(DVec3::new(15.0, 15.0, T)), 3.0);
    });
    let cut = p.model.add_sheet_cut(hole);
    p.rebuild();
    p.assert_ok();
    let one = PI * 9.0 * T;
    assert_close(p.volume(), W * H * T - one);
    // A 4 × 2 grid.
    let pattern = p.model.add_pattern(
        vec![cut],
        PatternDef::Linear {
            first: LinearDirection {
                direction: AxisRef::Standard(StdAxis::X),
                spacing: Scalar::new(20.0),
                count: Scalar::new(4.0),
                flip: false,
            },
            second: Some(LinearDirection {
                direction: AxisRef::Standard(StdAxis::Y),
                spacing: Scalar::new(30.0),
                count: Scalar::new(2.0),
                flip: false,
            }),
        },
    );
    p.rebuild();
    p.assert_ok();
    assert!(
        matches!(p.status(pattern), Status::Ok),
        "{:?}",
        p.status(pattern)
    );
    assert_close(p.volume(), W * H * T - 8.0 * one);
    // Mirror the first hole across the plate's middle (x = 50): a ninth hole at
    // (85, 15), between the grid's rows.
    let mid = p.model.add(FeatureKind::Plane(PlaneDef::Offset {
        from: PlaneRef::Standard(StdPlane::Right),
        distance: Scalar::new(50.0),
        flip: false,
    }));
    let mirror = p.model.add_mirror(vec![cut], PlaneRef::Feature(mid));
    p.rebuild();
    p.assert_ok();
    assert!(
        matches!(p.status(mirror), Status::Ok),
        "{:?}",
        p.status(mirror)
    );
    assert_close(p.volume(), W * H * T - 9.0 * one);
    let sheet = p.bodies()[0].sheet.clone().unwrap();
    assert_eq!(sheet.outline.iter().filter(|l| !l.outer).count(), 9);
    // A mirror can't copy a pattern, and says so.
    let bad = p.model.add_mirror(vec![pattern], PlaneRef::Feature(mid));
    p.rebuild();
    let Status::Failed(m) = p.status(bad) else {
        panic!("{:?}", p.status(bad))
    };
    assert!(m.contains("can't be copied"), "{m}");
}

#[test]
fn mirror_and_circular_pattern_of_solid_cuts() {
    let mut p = Part::default();
    let disc = p.sketch(TOP, |s| {
        s.add_circle(DVec2::ZERO, 50.0);
    });
    p.extrude(disc, Operation::NewBody, |e| e.depth = Scalar::new(10.0));
    let hole = p.sketch(TOP, |s| {
        s.add_circle(DVec2::new(35.0, 0.0), 4.0);
    });
    let cut = p.extrude(hole, Operation::Cut, |e| {
        e.end = peet_model::EndCondition::ThroughAll;
        e.reverse = true;
    });
    p.rebuild();
    p.assert_ok();
    let disc_volume = PI * 2500.0 * 10.0;
    let hole_volume = PI * 16.0 * 10.0;
    assert_close(p.volume(), disc_volume - hole_volume);
    let pattern = p.model.add_pattern(
        vec![cut],
        PatternDef::Circular {
            axis: AxisRef::Standard(StdAxis::Z),
            count: Scalar::new(6.0),
            angle: Scalar::new(360.0),
            flip: false,
        },
    );
    p.rebuild();
    p.assert_ok();
    assert!(
        matches!(p.status(pattern), Status::Ok),
        "{:?}",
        p.status(pattern)
    );
    assert_close(p.volume(), disc_volume - 6.0 * hole_volume);
    // Every face has its own name.
    let names = &p.bodies()[0].face_names;
    for i in 0..names.len() {
        for j in i + 1..names.len() {
            assert_ne!(names[i], names[j], "faces {i} and {j}");
        }
    }
    // Mirrored across the right plane (x = 0), the first hole lands on the copy at 180°:
    // nothing to cut, and the mirror says so.
    let mirror = p
        .model
        .add_mirror(vec![cut], PlaneRef::Standard(StdPlane::Right));
    p.rebuild();
    let Status::Failed(m) = p.status(mirror) else {
        panic!("{:?}", p.status(mirror))
    };
    assert!(m.contains("None of the copies"), "{m}");
}

#[test]
fn mirror_of_a_cut_with_an_arc() {
    // A plate from x = -50 to 50, with a half-round notch cut near its right end: the
    // flat side down, the round side up. Its mirror image must bulge the same way.
    let mut p = Part::default();
    let plate = p.sketch(TOP, |s| {
        shapes::rectangle(s, DVec2::new(-50.0, -20.0), DVec2::new(50.0, 30.0));
    });
    p.extrude(plate, Operation::NewBody, |e| e.depth = Scalar::new(5.0));
    let half = p.sketch(TOP, |s| {
        let (c, a, b) = (
            DVec2::new(30.0, 0.0),
            DVec2::new(38.0, 0.0),
            DVec2::new(22.0, 0.0),
        );
        s.add_arc(c, a, b);
        s.add_line(b, a);
    });
    let cut = p.extrude(half, Operation::Cut, |e| {
        e.end = peet_model::EndCondition::ThroughAll;
        e.reverse = true;
    });
    p.rebuild();
    p.assert_ok();
    let block = 100.0 * 50.0 * 5.0;
    let notch = PI * 64.0 / 2.0 * 5.0;
    assert_close(p.volume(), block - notch);
    let mirror = p
        .model
        .add_mirror(vec![cut], PlaneRef::Standard(StdPlane::Right));
    p.rebuild();
    p.assert_ok();
    assert!(
        matches!(p.status(mirror), Status::Ok),
        "{:?}",
        p.status(mirror)
    );
    assert_close(p.volume(), block - 2.0 * notch);
    // The mirrored arc's top is at (-30, 8).
    let top = p.bodies()[0]
        .solid
        .edges
        .iter()
        .flat_map(|e| (0..=64).map(move |i| e.point_at_fraction(f64::from(i) / 64.0)))
        .filter(|q| (q.x + 30.0).abs() < 9.0 && q.y > 0.5 && q.y < 20.0)
        .map(|q| q.y)
        .fold(f64::NEG_INFINITY, f64::max);
    assert!((top - 8.0).abs() < 1e-3, "{top}");
    // Every face has its own name.
    let names = &p.bodies()[0].face_names;
    for i in 0..names.len() {
        for j in i + 1..names.len() {
            assert_ne!(names[i], names[j], "faces {i} and {j}");
        }
    }
}

#[test]
fn copies_of_unsupported_features_fail_clearly() {
    let (mut p, _) = plate();
    let e = edge(&p, DVec3::new(0.0, H, T), DVec3::new(W, H, T));
    let f = p.model.add_edge_flange(Some(e));
    let pattern = p.model.add_pattern(
        vec![f],
        PatternDef::Linear {
            first: LinearDirection {
                direction: AxisRef::Standard(StdAxis::X),
                spacing: Scalar::new(20.0),
                count: Scalar::new(2.0),
                flip: false,
            },
            second: None,
        },
    );
    p.rebuild();
    let Status::Failed(m) = p.status(pattern) else {
        panic!("{:?}", p.status(pattern))
    };
    assert!(m.contains("can't be copied"), "{m}");
}
