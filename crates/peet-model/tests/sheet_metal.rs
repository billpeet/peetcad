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
fn round_hole_across_a_bend() {
    let (mut p, _, _) = plate(100.0, 60.0);
    let (_, e) = edge(&p, DVec3::new(0.0, 60.0, T), DVec3::new(100.0, 60.0, T));
    let flange = edge_flange(&mut p, e, |f| f.length = Scalar::new(30.0));
    p.rebuild();
    let top = p.on_face(DVec3::Z, DVec3::new(10.0, 10.0, T));
    // Drawn on the plate, reaching past its edge: in the flat pattern that is over the
    // bend and into the flange.
    let hole = p.sketch(top, |s| {
        s.add_circle(DVec2::new(30.0, 56.0), 5.0);
    });
    let cut = p.model.add_sheet_cut(hole);
    p.rebuild();
    p.assert_ok();
    assert!(matches!(p.status(cut), Status::Ok));
    assert!(matches!(p.status(flange), Status::Ok));
    // The body is still sheet metal, with the hole whole in its blank.
    let body = &p.bodies()[0];
    let sheet = body.sheet.as_ref().expect("still sheet metal");
    assert_eq!(sheet.outline.iter().filter(|l| !l.outer).count(), 1);
}

#[test]
fn a_cut_goes_through_every_flange_in_its_plane() {
    // A channel with a lip turned in from the top of each wall. The two lips lie in one
    // plane and are far apart in the blank. A sketch on one lip with a hole over each
    // cuts both.
    let (mut p, _, _) = plate(100.0, 60.0);
    for (y, inner) in [(60.0, 60.0 - T), (0.0, T)] {
        let (_, e) = edge(&p, DVec3::new(0.0, y, T), DVec3::new(100.0, y, T));
        edge_flange(&mut p, e, |f| f.length = Scalar::new(30.0));
        p.rebuild();
        let (_, e) = edge(&p, DVec3::new(0.0, inner, 30.0), DVec3::new(100.0, inner, 30.0));
        edge_flange(&mut p, e, |f| f.length = Scalar::new(15.0));
        p.rebuild();
    }
    p.assert_ok();
    let lip = p.on_face(DVec3::Z, DVec3::new(50.0, 55.0, 30.0));
    let holes = p.sketch(lip, |s| {
        s.add_circle(DVec2::new(50.0, 53.0), 2.0);
        s.add_circle(DVec2::new(50.0, 7.0), 2.0);
    });
    let cut = p.model.add_sheet_cut(holes);
    p.rebuild();
    p.assert_ok();
    assert!(matches!(p.status(cut), Status::Ok));
    let sheet = p.bodies()[0].sheet.as_ref().expect("still sheet metal");
    assert_eq!(sheet.outline.iter().filter(|l| !l.outer).count(), 2);
}

#[test]
fn a_shape_over_the_far_flange_cuts_only_that_flange() {
    // A hat section: walls up from the plate and a lip turned out from the top of each.
    // Holes over both lips, sketched on one, go through the lips and nowhere else: the far
    // lip's holes, taken into the blank through the near lip, land on the plate (Top Hat).
    let (mut p, _, _) = plate(100.0, 60.0);
    for y in [60.0, 0.0] {
        let (_, e) = edge(&p, DVec3::new(0.0, y, T), DVec3::new(100.0, y, T));
        edge_flange(&mut p, e, |f| f.length = Scalar::new(30.0));
        p.rebuild();
        let (_, e) = edge(&p, DVec3::new(0.0, y, 30.0), DVec3::new(100.0, y, 30.0));
        edge_flange(&mut p, e, |f| f.length = Scalar::new(20.0));
        p.rebuild();
    }
    p.assert_ok();
    let (lo, hi) = bounds(&p);
    assert!(lo.y < -10.0 && hi.y > 70.0, "the lips turn out: {lo} {hi}");
    let before = p.volume();
    let lip = p.on_face(DVec3::Z, DVec3::new(50.0, 70.0, 30.0));
    let holes = p.sketch(lip, |s| {
        for (x, y) in [(20.0, 70.0), (80.0, 70.0), (20.0, -10.0), (80.0, -10.0)] {
            s.add_circle(DVec2::new(x, y), 2.0);
        }
    });
    let cut = p.model.add_sheet_cut(holes);
    p.rebuild();
    p.assert_ok();
    assert!(matches!(p.status(cut), Status::Ok));
    assert_close(before - p.volume(), 4.0 * std::f64::consts::PI * 4.0 * T);
}

#[test]
fn corners_are_broken_and_follow_the_part() {
    // A plate with a wall. The wall's two top corners are rounded and one corner of the
    // plate is chamfered, each by the edge through the sheet at the corner.
    let (mut p, _, _) = plate(100.0, 60.0);
    let (_, e) = edge(&p, DVec3::new(0.0, 60.0, T), DVec3::new(100.0, 60.0, T));
    let wall = edge_flange(&mut p, e, |f| f.length = Scalar::new(30.0));
    p.rebuild();
    p.assert_ok();
    let before = p.volume();
    let corners: Vec<_> = [0.0, 100.0]
        .into_iter()
        .map(|x| edge(&p, DVec3::new(x, 60.0 - T, 30.0), DVec3::new(x, 60.0, 30.0)).1)
        .collect();
    let rounds = p.model.add_break_corner(corners);
    let FeatureKind::BreakCorner(def) = &mut p.model.feature_mut(rounds).unwrap().kind else {
        unreachable!()
    };
    def.size = Scalar::new(5.0);
    p.rebuild();
    p.assert_ok();
    let round = 25.0 - 25.0 * std::f64::consts::FRAC_PI_4;
    assert_close(before - p.volume(), 2.0 * round * T);
    let (_, e) = edge(&p, DVec3::new(0.0, 0.0, 0.0), DVec3::new(0.0, 0.0, T));
    let chamfer = p.model.add_break_corner(vec![e]);
    let FeatureKind::BreakCorner(def) = &mut p.model.feature_mut(chamfer).unwrap().kind else {
        unreachable!()
    };
    def.kind = peet_sheetmetal::BreakKind::Chamfer;
    def.size = Scalar::new(3.0);
    p.rebuild();
    p.assert_ok();
    assert_close(before - p.volume(), (2.0 * round + 4.5) * T);
    // Still sheet metal, with the rounds in its blank.
    assert!(p.bodies()[0].sheet.is_some());
    // The wall made taller: the rounds go up with it.
    let FeatureKind::EdgeFlange(def) = &mut p.model.feature_mut(wall).unwrap().kind else {
        unreachable!()
    };
    def.length = Scalar::new(45.0);
    p.rebuild();
    p.assert_ok();
    assert!(matches!(p.status(rounds), Status::Ok));
    let (_, hi) = bounds(&p);
    assert_close(hi.z, 45.0);
    // Two rounds that would overlap along the wall's top are refused.
    let FeatureKind::BreakCorner(def) = &mut p.model.feature_mut(rounds).unwrap().kind else {
        unreachable!()
    };
    def.size = Scalar::new(80.0);
    p.rebuild();
    assert!(matches!(p.status(rounds), Status::Failed(m) if m.contains("too short for both")));
}

#[test]
fn an_inside_corner_of_a_cut_is_rounded() {
    // A notch cut into the plate's edge has two inside corners. Rounding them gives the
    // sheet back between each corner and its round, as SolidWorks' fillet does there,
    // and a later cut over a round still takes it.
    let (mut p, _, _) = plate(100.0, 60.0);
    let top = p.on_face(DVec3::Z, DVec3::new(10.0, 10.0, T));
    let notch = p.sketch(top.clone(), |s| {
        shapes::rectangle(s, DVec2::new(40.0, 40.0), DVec2::new(60.0, 70.0));
    });
    p.model.add_sheet_cut(notch);
    p.rebuild();
    p.assert_ok();
    let before = p.volume();
    let corners: Vec<_> = [40.0, 60.0]
        .into_iter()
        .map(|x| edge(&p, DVec3::new(x, 40.0, 0.0), DVec3::new(x, 40.0, T)).1)
        .collect();
    let rounds = p.model.add_break_corner(corners);
    let FeatureKind::BreakCorner(def) = &mut p.model.feature_mut(rounds).unwrap().kind else {
        unreachable!()
    };
    def.size = Scalar::new(5.0);
    p.rebuild();
    p.assert_ok();
    assert!(matches!(p.status(rounds), Status::Ok));
    let round = 25.0 - 25.0 * std::f64::consts::FRAC_PI_4;
    assert_close(p.volume() - before, 2.0 * round * T);
    assert!(p.bodies()[0].sheet.is_some());
    let over = p.sketch(top, |s| {
        shapes::rectangle(s, DVec2::new(30.0, 30.0), DVec2::new(50.0, 50.0));
    });
    p.model.add_sheet_cut(over);
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume() - before, (round - 300.0) * T);
}

#[test]
fn a_corner_between_a_side_and_an_arc_is_rounded() {
    // A slot cut across the plate's edge: where its round end meets the edge the corner
    // is between a straight side and an arc. A fillet rounds it, touching both.
    let (mut p, _, _) = plate(100.0, 60.0);
    let top = p.on_face(DVec3::Z, DVec3::new(10.0, 10.0, T));
    let slot = p.sketch(top, |s| {
        s.add_circle(DVec2::new(50.0, 55.0), 10.0);
    });
    p.model.add_sheet_cut(slot);
    p.rebuild();
    p.assert_ok();
    let before = p.volume();
    let x = 50.0 - 75f64.sqrt();
    let (_, e) = edge(&p, DVec3::new(x, 60.0, 0.0), DVec3::new(x, 60.0, T));
    let round = p.model.add_break_corner(vec![e]);
    let FeatureKind::BreakCorner(def) = &mut p.model.feature_mut(round).unwrap().kind else {
        unreachable!()
    };
    def.size = Scalar::new(3.0);
    p.rebuild();
    p.assert_ok();
    assert!(matches!(p.status(round), Status::Ok));
    let body = &p.bodies()[0];
    assert!(body.sheet.is_some());
    let rounds = body
        .solid
        .faces
        .iter()
        .filter(|f| matches!(&f.surface, Surface::Cylinder(c) if (c.radius - 3.0).abs() < 1e-9))
        .count();
    assert_eq!(rounds, 1);
    // A 60° outside corner loses sheet.
    assert!(p.volume() < before - 1e-3);
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
            FaceTag::Wall { .. } | FaceTag::Form { .. } => {}
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

#[test]
fn a_cut_through_the_flanges_in_its_way() {
    // A channel: flanges up from the plate's two long edges. A notch sketched on the plate
    // at one end, going up through everything (as SolidWorks' cut-extrude does), trims
    // the ends of both sides as well as the plate's.
    let notched = |end: peet_model::SheetCutEnd, depth: f64| {
        let (mut p, _, _) = plate(100.0, 60.0);
        for (a, b) in [
            (DVec3::new(0.0, 0.0, T), DVec3::new(100.0, 0.0, T)),
            (DVec3::new(100.0, 60.0, T), DVec3::new(0.0, 60.0, T)),
        ] {
            let (_, e) = edge(&p, a, b);
            edge_flange(&mut p, e, |f| f.length = Scalar::new(20.0));
            p.rebuild();
            p.assert_ok();
        }
        let top = p.on_face(DVec3::Z, DVec3::new(50.0, 30.0, T));
        let notch = p.sketch(top.clone(), |s| {
            shapes::rectangle(s, DVec2::new(-5.0, -5.0), DVec2::new(10.0, 65.0));
        });
        let cut = p.model.add_sheet_cut(notch);
        let FeatureKind::SheetCut(def) = &mut p.model.feature_mut(cut).unwrap().kind else {
            unreachable!()
        };
        def.end = end;
        def.depth = Scalar::new(depth);
        def.reverse = true;
        p.rebuild();
        p.assert_ok();
        bounds(&p)
    };
    let (lo, _) = notched(peet_model::SheetCutEnd::ThroughAll, 0.0);
    assert_close(lo.x, 10.0);
    // Only as far as a depth: the sides' tops are left at the end.
    let (lo, _) = notched(peet_model::SheetCutEnd::Blind, 8.0);
    assert!(lo.x < 1.0, "{lo}");
    // Through its own sheet only, as before.
    let (lo, _) = notched(peet_model::SheetCutEnd::Sheet, 0.0);
    assert!(lo.x < 1.0, "{lo}");
}
