//! Sheet metal features end to end: base flanges, edge flanges that follow their edge
//! through edits, sheet metal cuts across bends, open profiles and failures.

mod common;

use std::f64::consts::FRAC_PI_2;

use common::{Part, TOP, assert_close, dimensioned_rectangle};
use peet_kernel::{EdgeId, Surface};
use peet_math::{DVec2, DVec3};
use peet_model::{
    BendModelDef, EdgeRef, FeatureId, FeatureKind, Operation, PlaneRef, Scalar, Status,
};
use peet_sheetmetal::{BendModel, BendValues, FaceTag, FlangePosition};
use peet_sketch::shapes;

const T: f64 = 2.0;
const R: f64 = 3.0;

/// A `w` × `h` plate (t = 2, R = 3, K = 0.5) on the top plane.
fn plate(w: f64, h: f64) -> (Part, FeatureId, FeatureId) {
    let mut p = Part::default();
    let sketch = p.sketch(TOP, |s| dimensioned_rectangle(s, w, h));
    let flange = p.model.add_base_flange(sketch);
    let FeatureKind::BaseFlange(b) = &mut p.model.feature_mut(flange).unwrap().kind else {
        unreachable!()
    };
    b.settings.thickness = Scalar::new(T);
    b.settings.radius = Scalar::new(R);
    b.settings.model = BendModelDef::KFactor(Scalar::new(0.5));
    p.rebuild();
    p.assert_ok();
    (p, sketch, flange)
}

/// The edge of body 0 between the points (in either direction) on the top or bottom side.
#[track_caller]
fn edge(p: &Part, a: DVec3, b: DVec3) -> (EdgeId, EdgeRef) {
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
            return (e, body.edge_ref(e).unwrap());
        }
    }
    panic!("no edge from {a} to {b}");
}

fn edge_flange(
    p: &mut Part,
    e: EdgeRef,
    edit: impl FnOnce(&mut peet_model::EdgeFlangeFeature),
) -> FeatureId {
    let id = p.model.add_edge_flange(Some(e));
    let FeatureKind::EdgeFlange(f) = &mut p.model.feature_mut(id).unwrap().kind else {
        unreachable!()
    };
    edit(f);
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

#[test]
fn base_flange_plate() {
    let (p, _, _) = plate(100.0, 60.0);
    let body = &p.bodies()[0];
    let sheet = body.sheet.as_ref().expect("a sheet metal body");
    assert_close(p.volume(), 100.0 * 60.0 * T);
    assert_eq!(sheet.flat, body.solid);
    let (lo, hi) = bounds(&p);
    assert!(lo.abs_diff_eq(DVec3::ZERO, 1e-9) && hi.abs_diff_eq(DVec3::new(100.0, 60.0, T), 1e-9));
}

#[test]
fn edge_flange_follows_its_edge() {
    let (mut p, sketch, _) = plate(100.0, 60.0);
    // The far edge (y = 60) on the top side.
    let (_, e) = edge(&p, DVec3::new(0.0, 60.0, T), DVec3::new(100.0, 60.0, T));
    let f = edge_flange(&mut p, e, |f| f.length = Scalar::new(25.0));
    p.rebuild();
    p.assert_ok();
    let (lo, hi) = bounds(&p);
    assert!(lo.abs_diff_eq(DVec3::ZERO, 1e-9), "{lo}");
    assert!(hi.abs_diff_eq(DVec3::new(100.0, 60.0, 25.0), 1e-9), "{hi}");
    assert!(matches!(p.status(f), Status::Ok));
    // Widen and deepen the plate: the flange stays on the far edge, full width.
    p.set_dimension(sketch, "d1", "140");
    p.set_dimension(sketch, "d2", "80");
    p.rebuild();
    p.assert_ok();
    let (lo, hi) = bounds(&p);
    assert!(lo.abs_diff_eq(DVec3::ZERO, 1e-9), "{lo}");
    assert!(hi.abs_diff_eq(DVec3::new(140.0, 80.0, 25.0), 1e-9), "{hi}");
    // Flat length along y: 80 + 25 - BD.
    let v = BendValues::new(BendModel::KFactor(0.5), FRAC_PI_2, R, T).unwrap();
    let sheet = p.bodies()[0].sheet.clone().unwrap();
    let (flo, fhi) = sheet.flat_bounds();
    assert_close(fhi.y - flo.y, 80.0 + 25.0 - v.deduction());
    // K = 0.5 keeps the volume.
    assert_close(
        p.volume(),
        peet_kernel::validate::measure::volume(&sheet.flat),
    );
}

#[test]
fn flange_edits_rebuild_incrementally() {
    let (mut p, _, base) = plate(100.0, 60.0);
    let (_, e) = edge(&p, DVec3::new(0.0, 60.0, T), DVec3::new(100.0, 60.0, T));
    let f = edge_flange(&mut p, e, |_| {});
    p.rebuild();
    let FeatureKind::EdgeFlange(def) = &mut p.model.feature_mut(f).unwrap().kind else {
        unreachable!()
    };
    def.angle = Scalar::new(45.0);
    def.position = FlangePosition::BendOutside;
    let stats = p.rebuild().stats;
    p.assert_ok();
    assert_eq!(stats.rebuilt, 1, "only the flange is rebuilt");
    assert!(p.eval().state(base).is_some_and(|s| !s.rebuilt));
    let (_, hi) = bounds(&p);
    assert!(hi.y > 60.0 + R, "bend outside goes past the edge: {hi}");
}

#[test]
fn flange_on_a_flange_and_a_cut_across_the_bend() {
    let (mut p, _, _) = plate(100.0, 60.0);
    let (_, e) = edge(&p, DVec3::new(0.0, 60.0, T), DVec3::new(100.0, 60.0, T));
    let f1 = edge_flange(&mut p, e, |f| f.length = Scalar::new(30.0));
    p.rebuild();
    p.assert_ok();
    // The flange's tip edge on its inner side (towards the plate).
    let inner = 60.0 - T;
    let (_, tip) = edge(
        &p,
        DVec3::new(0.0, inner, 30.0),
        DVec3::new(100.0, inner, 30.0),
    );
    let f2 = edge_flange(&mut p, tip, |f| f.length = Scalar::new(12.0));
    p.rebuild();
    p.assert_ok();
    let (lo, hi) = bounds(&p);
    assert!(hi.abs_diff_eq(DVec3::new(100.0, 60.0, 30.0), 1e-9), "{hi}");
    assert!(lo.abs_diff_eq(DVec3::ZERO, 1e-9), "{lo}");

    // A slot on the plate's top face, running across the first bend into the flange.
    let top = p.on_face(DVec3::Z, DVec3::new(10.0, 10.0, T));
    let slot = p.sketch(top, |s| {
        shapes::rectangle(s, DVec2::new(40.0, 40.0), DVec2::new(50.0, 70.0));
    });
    let cut = p.model.add_sheet_cut(slot);
    let before = p.volume();
    p.rebuild();
    p.assert_ok();
    let removed = before - p.volume();
    // 10 mm wide, 30 mm of flat pattern long, t thick (K = 0.5 keeps volumes).
    assert_close(removed, 10.0 * 30.0 * T);
    // The slot's walls are named after the cut's sketch curves.
    let body = &p.bodies()[0];
    let named = body
        .face_names
        .iter()
        .filter(|n| n.features().any(|f| f == cut))
        .count();
    assert!(named >= 4, "{named}");
    // Making the first flange longer moves everything, and the cut follows in the flat.
    let FeatureKind::EdgeFlange(def) = &mut p.model.feature_mut(f1).unwrap().kind else {
        unreachable!()
    };
    def.length = Scalar::new(40.0);
    p.rebuild();
    p.assert_ok();
    assert!(matches!(p.status(f2), Status::Ok));
    let (_, hi) = bounds(&p);
    assert_close(hi.z, 40.0);
}

#[test]
fn round_hole_across_a_bend_fails_with_a_message() {
    let (mut p, _, _) = plate(100.0, 60.0);
    let (_, e) = edge(&p, DVec3::new(0.0, 60.0, T), DVec3::new(100.0, 60.0, T));
    let flange = edge_flange(&mut p, e, |f| f.length = Scalar::new(30.0));
    p.rebuild();
    let top = p.on_face(DVec3::Z, DVec3::new(10.0, 10.0, T));
    let hole = p.sketch(top, |s| {
        s.add_circle(DVec2::new(30.0, 56.0), 5.0);
    });
    let cut = p.model.add_sheet_cut(hole);
    p.rebuild();
    let Status::Failed(m) = p.status(cut) else {
        panic!("{:?}", p.status(cut));
    };
    let name = p.model.feature(flange).unwrap().name.clone();
    assert!(m.contains("curved") && m.contains(&name), "{m}");
    // The rest of the part still builds.
    assert!(matches!(p.status(flange), Status::Ok));
}

#[test]
fn open_profile_channel() {
    let mut p = Part::default();
    let sketch = p.sketch(TOP, |s| {
        let pts = [
            DVec2::new(0.0, 40.0),
            DVec2::ZERO,
            DVec2::new(60.0, 0.0),
            DVec2::new(60.0, 40.0),
        ];
        let lines: Vec<_> = (0..3).map(|i| s.add_line(pts[i], pts[i + 1])).collect();
        for w in lines.windows(2) {
            let end = s.endpoints(w[0]).unwrap().1;
            let start = s.endpoints(w[1]).unwrap().0;
            s.add_constraint(peet_sketch::ConstraintKind::Coincident(end, start))
                .unwrap();
        }
    });
    let flange = p.model.add_base_flange(sketch);
    let FeatureKind::BaseFlange(b) = &mut p.model.feature_mut(flange).unwrap().kind else {
        unreachable!()
    };
    b.depth = Scalar::new(80.0);
    b.settings.thickness = Scalar::new(T);
    b.settings.radius = Scalar::new(R);
    p.rebuild();
    p.assert_ok();
    let body = &p.bodies()[0];
    let sheet = body.sheet.as_ref().unwrap();
    assert_eq!(sheet.layout.bends().count(), 2);
    let cylinders = body
        .solid
        .faces
        .iter()
        .filter(|f| matches!(f.surface, Surface::Cylinder(_)))
        .count();
    assert_eq!(cylinders, 4);
    let (lo, hi) = bounds(&p);
    // The lines are one face; the material is to their left, inside the U.
    assert!(lo.abs_diff_eq(DVec3::ZERO, 1e-9), "{lo}");
    assert!(hi.abs_diff_eq(DVec3::new(60.0, 40.0, 80.0), 1e-9), "{hi}");
}

#[test]
fn solid_cut_on_a_sheet_body_warns() {
    let (mut p, _, _) = plate(100.0, 60.0);
    let top = p.on_face(DVec3::Z, DVec3::new(10.0, 10.0, T));
    let hole = p.sketch(top, |s| {
        s.add_circle(DVec2::new(30.0, 30.0), 5.0);
    });
    let cut = p.extrude(hole, Operation::Cut, |e| {
        e.end = peet_model::EndCondition::ThroughAll;
    });
    p.rebuild();
    let Status::Warning(m) = p.status(cut) else {
        panic!("{:?}", p.status(cut));
    };
    assert!(m.contains("flat pattern"), "{m}");
    assert!(p.bodies()[0].sheet.is_none());
}

#[test]
fn edge_flange_needs_a_sheet_edge() {
    let (mut p, _, _) = plate(100.0, 60.0);
    let f = p.model.add_edge_flange(None);
    p.rebuild();
    let Status::Failed(m) = p.status(f) else {
        panic!()
    };
    assert!(m.contains("Pick"), "{m}");
    // A vertical edge through the thickness is not a flange edge.
    let (_, corner) = edge(&p, DVec3::new(0.0, 0.0, 0.0), DVec3::new(0.0, 0.0, T));
    let FeatureKind::EdgeFlange(def) = &mut p.model.feature_mut(f).unwrap().kind else {
        unreachable!()
    };
    def.edge = Some(corner);
    p.rebuild();
    let Status::Failed(m) = p.status(f) else {
        panic!()
    };
    assert!(m.contains("top or bottom face"), "{m}");
}

#[test]
fn faces_are_named_by_role() {
    let (mut p, _, base) = plate(100.0, 60.0);
    let (_, e) = edge(&p, DVec3::new(0.0, 60.0, T), DVec3::new(100.0, 60.0, T));
    let f = edge_flange(&mut p, e, |_| {});
    p.rebuild();
    let body = &p.bodies()[0];
    let sheet = body.sheet.as_ref().unwrap();
    for (i, tag) in sheet.faces.iter().enumerate() {
        let name = &body.face_names[i];
        let owner = name.origins()[0].feature;
        match tag {
            FaceTag::Top { piece } | FaceTag::Bottom { piece } => {
                let expect = if *piece == 0 { base } else { f };
                assert_eq!(owner, expect);
            }
            FaceTag::Wall { .. } => {}
        }
    }
    // A sketch on the flange's outer face follows the flange when the plate changes.
    let outer = PlaneRef::Face(p.face_ref(DVec3::Y, DVec3::new(50.0, 60.0, 10.0)));
    let s = p.sketch(outer, |s| {
        s.add_circle(DVec2::new(50.0, 10.0), 3.0);
    });
    p.rebuild();
    p.assert_ok();
    assert!(matches!(p.status(s), Status::Ok));
    let description = p.model.describe_face(
        &p.bodies()[0]
            .face_name(p.face(DVec3::Y, DVec3::new(50.0, 60.0, 10.0)).1)
            .clone(),
    );
    assert!(description.contains("bottom face"), "{description}");
}

#[test]
fn sample_enclosure_builds() {
    let (mut model, mut engine) = peet_model::samples::enclosure();
    let eval = engine.regenerate(&mut model);
    for f in model.features() {
        assert!(
            eval.status(f.id).is_some_and(|s| matches!(s, Status::Ok)),
            "{}: {:?}",
            f.name,
            eval.status(f.id)
        );
    }
    let body = &eval.bodies[0];
    let sheet = body.sheet.as_ref().unwrap();
    assert_eq!(sheet.layout.bends().count(), 4);
    let report = sheet.report();
    assert_eq!(report.cutouts, 7);
    // Changing the thickness parameter rebuilds the whole panel.
    model.parameters.set("thickness", "2mm").unwrap();
    let eval = engine.regenerate(&mut model);
    let sheet = eval.bodies[0].sheet.as_ref().unwrap();
    assert_close(sheet.layout.settings.thickness, 2.0);
    let b = eval.bodies[0].solid.bounds();
    assert!(
        b.max.abs_diff_eq(DVec3::new(200.0, 150.0, 25.0), 1e-9),
        "{:?}",
        b.max
    );
}
