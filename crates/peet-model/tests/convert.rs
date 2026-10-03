//! Convert to sheet metal, end to end: a solid modelled as a solid, and a sheet metal part
//! that came back as a plain solid (an import), become sheet metal bodies with the right
//! flat pattern, and take sheet metal features afterwards.

mod common;

use std::f64::consts::FRAC_PI_2;

use common::{FRONT, Part, assert_close};
use peet_kernel::EdgeId;
use peet_kernel::validate::measure;
use peet_math::{DVec2, DVec3, Plane};
use peet_model::{
    BendModelDef, ConvertToSheetFeature, EdgeRef, FeatureId, FeatureKind, ImportedSolid, Operation,
    Output, PlaneRef, Scalar, Status,
};
use peet_sheetmetal::{BendModel, BendValues, FaceTag};
use peet_sketch::Sketch;

const T: f64 = 2.0;
const R: f64 = 3.0;
const K: f64 = 0.44;
/// The L profile: a base `BASE` long and a leg `LEG` high (outside sizes), extruded
/// `DEPTH`.
const BASE: f64 = 40.0;
const LEG: f64 = 30.0;
const DEPTH: f64 = 50.0;

fn convert_mut(p: &mut Part, id: FeatureId) -> &mut ConvertToSheetFeature {
    match &mut p.model.feature_mut(id).unwrap().kind {
        FeatureKind::ConvertToSheet(c) => c,
        _ => panic!("not a conversion"),
    }
}

#[track_caller]
fn failure(p: &Part, id: FeatureId) -> String {
    match p.status(id) {
        Status::Failed(m) => m.clone(),
        other => panic!("expected a failure, got {other:?}"),
    }
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

/// The edge of body 0 between two points, in either direction.
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
            return body.edge_ref(EdgeId(e.0)).unwrap();
        }
    }
    panic!("no edge from {a} to {b}");
}

/// A thin L extruded from the front plane: the base lies on the top plane (`z` from 0 to
/// `T`), the leg stands at `x = BASE`, the corner is rounded inside (`R`) and outside
/// (`R + T`) about one axis. With `round` off the corner is sharp.
fn extruded_l(round: bool) -> Part {
    let mut p = Part::default();
    let sketch = p.sketch(FRONT, |s| {
        let v = DVec2::new;
        let c = v(BASE - R - T, R + T);
        if round {
            s.add_line(v(0.0, 0.0), v(c.x, 0.0));
            s.add_arc(c, v(c.x, 0.0), v(BASE, c.y));
            s.add_line(v(BASE, c.y), v(BASE, LEG));
            s.add_line(v(BASE, LEG), v(BASE - T, LEG));
            s.add_line(v(BASE - T, LEG), v(BASE - T, c.y));
            s.add_arc(c, v(c.x, T), v(BASE - T, c.y));
            s.add_line(v(c.x, T), v(0.0, T));
        } else {
            s.add_line(v(0.0, 0.0), v(BASE, 0.0));
            s.add_line(v(BASE, 0.0), v(BASE, LEG));
            s.add_line(v(BASE, LEG), v(BASE - T, LEG));
            s.add_line(v(BASE - T, LEG), v(BASE - T, T));
            s.add_line(v(BASE - T, T), v(0.0, T));
        }
        s.add_line(v(0.0, T), v(0.0, 0.0));
    });
    p.extrude(sketch, Operation::NewBody, |e| {
        e.depth = Scalar::new(DEPTH);
    });
    p.rebuild();
    p.assert_ok();
    p
}

/// The bend allowance and deduction of a right-angle bend of the test parts.
fn right_angle() -> BendValues {
    BendValues::new(BendModel::KFactor(K), FRAC_PI_2, R, T).unwrap()
}

#[test]
fn extruded_l_becomes_sheet_metal_and_takes_flanges_and_cuts() {
    let mut p = extruded_l(true);
    let solid_volume = p.volume();
    assert!(p.bodies()[0].sheet.is_none());
    // Where the extrusion went: the front plane's normal is −Y.
    let y = p.bodies()[0].solid.bounds();
    let (y0, y1) = (y.min.y, y.max.y);
    assert_close(y1 - y0, DEPTH);
    let mid_y = (y0 + y1) / 2.0;

    // Fix the inside of the base (the face the leg stands on).
    let inside = p.face_ref(DVec3::Z, DVec3::new(10.0, mid_y, T));
    let convert = p.model.add_convert_to_sheet(Some(inside));
    assert_eq!(p.model.feature(convert).unwrap().name, "Convert-To-Sheet1");
    assert_eq!(
        p.model.feature(convert).unwrap().kind.type_name(),
        "Convert to sheet metal"
    );
    assert!(p.model.feature(convert).unwrap().kind.is_sheet_metal());
    p.rebuild();
    p.assert_ok();
    assert!(matches!(p.status(convert), Status::Ok));
    assert_eq!(p.bodies().len(), 1);
    assert_close(p.volume(), solid_volume);

    let body = &p.bodies()[0];
    let sheet = body.sheet.as_ref().expect("a sheet metal body");
    let report = sheet.report();
    assert_close(report.thickness, T);
    assert_close(sheet.layout.settings.radius, R);
    assert_eq!(report.bends.len(), 1);
    assert_close(report.bends[0].angle, 90.0);
    assert_close(report.bends[0].radius, R);
    assert_close(report.bends[0].k_factor, K);
    assert!(report.bends[0].up, "the leg stands on the fixed side");
    // Hand calculation: the two outside lengths less one bend deduction, by the depth.
    let v = right_angle();
    let flat = BASE + LEG - v.deduction();
    assert_close(flat, (BASE - R - T) + v.allowance + (LEG - R - T));
    let (short, long) = (
        report.flat_size.x.min(report.flat_size.y),
        report.flat_size.x.max(report.flat_size.y),
    );
    assert_close(short, DEPTH);
    assert_close(long, flat);
    assert_close(report.flat_area, DEPTH * flat);
    // The body is still the extrusion's, and its faces are named after the conversion.
    assert_eq!(body.origin, p.model.features().nth(1).unwrap().id);
    assert!(
        body.face_names
            .iter()
            .all(|n| n.features().all(|f| f == convert))
    );
    let mut names = body.face_names.clone();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), body.face_names.len(), "names are unique");

    // An edge flange on the base's free end, on the fixed side.
    let end = edge(&p, DVec3::new(0.0, y0, T), DVec3::new(0.0, y1, T));
    let flange = p.model.add_edge_flange(Some(end));
    p.rebuild();
    p.assert_ok();
    assert!(
        matches!(p.status(flange), Status::Ok),
        "{:?}",
        p.status(flange)
    );
    let sheet = p.bodies()[0].sheet.as_ref().unwrap();
    let report = sheet.report();
    assert_eq!(report.bends.len(), 2);
    // The default flange is 20 mm, material inside: the blank grows by 20 − BD.
    let long = report.flat_size.x.max(report.flat_size.y);
    assert_close(long, flat + 20.0 - v.deduction());
    // The flange uses the radius the conversion found.
    assert!(report.bends.iter().all(|b| (b.radius - R).abs() < 1e-9));

    // A sheet metal cut on the fixed face: a hole through the base.
    let top = p.on_face(DVec3::Z, DVec3::new(20.0, mid_y, T));
    let hole = sketch_at(&mut p, top, |s, to| {
        s.add_circle(to(DVec3::new(20.0, mid_y, T)), 4.0);
    });
    let cut = p.model.add_sheet_cut(hole);
    let before = p.volume();
    p.rebuild();
    p.assert_ok();
    assert!(matches!(p.status(cut), Status::Ok), "{:?}", p.status(cut));
    assert_close(before - p.volume(), std::f64::consts::PI * 16.0 * T);
    let sheet = p.bodies()[0].sheet.as_ref().unwrap();
    assert_eq!(sheet.report().cutouts, 1);

    // Another K-factor changes the flat pattern, not the part.
    let folded = p.volume();
    convert_mut(&mut p, convert).model = BendModelDef::KFactor(Scalar::new(0.3));
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), folded);
    let report = p.bodies()[0].sheet.as_ref().unwrap().report();
    let long2 = report.flat_size.x.max(report.flat_size.y);
    assert_close(long - long2, 2.0 * FRAC_PI_2 * (K - 0.3) * T);
}

#[test]
fn conversion_follows_the_solid() {
    // No face picked: the only body, unfolded from its largest flat face.
    let mut p = extruded_l(true);
    let convert = p.model.add_convert_to_sheet(None);
    p.rebuild();
    p.assert_ok();
    let flat = |p: &Part| {
        let r = p.bodies()[0].sheet.as_ref().unwrap().report();
        r.flat_size.x.max(r.flat_size.y)
    };
    let v = right_angle();
    assert_close(flat(&p), BASE + LEG - v.deduction());
    // A deeper extrusion: the conversion is rebuilt, and the blank is wider.
    let extrude = p.model.features().nth(1).unwrap().id;
    p.extrude_mut(extrude).params.depth = Scalar::new(120.0);
    p.rebuild();
    p.assert_ok();
    assert!(p.eval().state(convert).unwrap().rebuilt);
    assert_close(flat(&p), 120.0);
    let r = p.bodies()[0].sheet.as_ref().unwrap().report();
    assert_close(r.flat_size.x.min(r.flat_size.y), BASE + LEG - v.deduction());
    // Rebuilding with nothing changed reuses it.
    p.rebuild();
    assert!(!p.eval().state(convert).unwrap().rebuilt);
}

#[test]
fn conversion_failures_say_what_to_do() {
    // Sharp corners: not a sheet with bends.
    let mut p = extruded_l(false);
    let volume = p.volume();
    let convert = p.model.add_convert_to_sheet(None);
    p.rebuild();
    let m = failure(&p, convert);
    assert!(m.contains("round them first"), "{m}");
    // The body passes through unchanged.
    assert_close(p.volume(), volume);
    assert!(p.bodies()[0].sheet.is_none());

    // A curved fixed face.
    let mut p = extruded_l(true);
    let body = &p.bodies()[0];
    let bend = body
        .solid
        .face_ids()
        .find(|&f| {
            matches!(
                body.solid.face(f).surface,
                peet_kernel::Surface::Cylinder(_)
            )
        })
        .unwrap();
    let bend = body.face_ref(bend);
    let convert = p.model.add_convert_to_sheet(Some(bend));
    p.rebuild();
    assert!(failure(&p, convert).contains("must be flat"));

    // Converting twice.
    convert_mut(&mut p, convert).face = None;
    let again = p.model.add_convert_to_sheet(None);
    p.rebuild();
    assert!(matches!(p.status(convert), Status::Ok));
    assert!(failure(&p, again).contains("already sheet metal"));

    // Nothing to convert, a bad K-factor, and two bodies with no face picked.
    let mut p = Part::default();
    let convert = p.model.add_convert_to_sheet(None);
    p.rebuild();
    assert!(failure(&p, convert).contains("no body to convert"));
    convert_mut(&mut p, convert).model = BendModelDef::KFactor(Scalar::new(1.5));
    p.rebuild();
    assert!(failure(&p, convert).contains("K-factor"));

    let mut p = Part::default();
    let block = |x: f64, t: f64| ImportedSolid {
        name: "plate".to_owned(),
        solid: peet_kernel::primitive::cuboid(
            DVec3::new(x, 0.0, 0.0),
            DVec3::new(x + 30.0, 20.0, t),
        ),
    };
    p.model.add_import(
        "plates.step".to_owned(),
        vec![block(0.0, 2.0), block(50.0, 3.0)],
    );
    let convert = p.model.add_convert_to_sheet(None);
    p.rebuild();
    assert!(failure(&p, convert).contains("2 bodies"));
    // With a face picked, that body is converted and the other left alone.
    let face = p.face_ref(DVec3::Z, DVec3::new(60.0, 10.0, 3.0));
    convert_mut(&mut p, convert).face = Some(face);
    p.rebuild();
    p.assert_ok();
    assert!(p.bodies()[0].sheet.is_none());
    let sheet = p.bodies()[1].sheet.as_ref().expect("the second plate");
    assert_close(sheet.report().thickness, 3.0);
}

#[test]
fn the_enclosure_comes_back_from_a_plain_solid() {
    // The Phase 4 sample, as another CAD system would hand it over: only its folded
    // solid.
    let (sample, engine) = peet_model::samples::enclosure();
    let original = &engine.evaluation().bodies[0];
    let original_sheet = original.sheet.as_ref().expect("the sample is sheet metal");
    let want = original_sheet.report();
    // Documented in the roadmap: 244.356636 × 194.356636 mm at K = 0.44, R 2, t 1.5.
    assert!((want.flat_size.x - 244.356636).abs() < 1e-6);
    assert!((want.flat_size.y - 194.356636).abs() < 1e-6);
    let _ = sample;

    let mut p = Part::default();
    p.model.add_import(
        "enclosure.step".to_owned(),
        vec![ImportedSolid {
            name: "Enclosure Panel".to_owned(),
            solid: original.solid.clone(),
        }],
    );
    p.rebuild();
    p.assert_ok();
    assert!(p.bodies()[0].sheet.is_none());
    let volume = p.volume();

    // The panel's top face stays fixed.
    let top = p.face_ref(DVec3::Z, DVec3::new(50.0, 50.0, 1.5));
    let convert = p.model.add_convert_to_sheet(Some(top));
    p.rebuild();
    p.assert_ok();
    assert!(matches!(p.status(convert), Status::Ok));
    assert_close(p.volume(), volume);

    let body = &p.bodies()[0];
    let sheet = body.sheet.as_ref().expect("a sheet metal body");
    let got = sheet.report();
    assert_close(got.thickness, 1.5);
    assert_close(sheet.layout.settings.radius, 2.0);
    assert!(
        (got.flat_size.x - 244.356636).abs() < 1e-6,
        "{}",
        got.flat_size
    );
    assert!(
        (got.flat_size.y - 194.356636).abs() < 1e-6,
        "{}",
        got.flat_size
    );
    assert_close(got.flat_area, want.flat_area);
    assert_eq!(got.cutouts, want.cutouts);
    assert_eq!(got.pieces, 1);
    // Four bends, though the slot across the right-hand one splits its faces in two.
    assert_eq!(got.bends.len(), 4);
    let lengths = |r: &peet_sheetmetal::Report| {
        let mut l: Vec<f64> = r.bends.iter().map(|b| b.length).collect();
        l.sort_by(f64::total_cmp);
        l
    };
    for (g, w) in lengths(&got).iter().zip(lengths(&want)) {
        assert_close(*g, w);
    }
    for b in &got.bends {
        assert_close(b.angle, 90.0);
        assert_close(b.radius, 2.0);
        assert!(b.up);
    }
    // The flat pattern lies where the original's does: both keep the panel in place.
    let (a, b) = (sheet.flat.bounds(), original_sheet.flat.bounds());
    assert!(a.min.abs_diff_eq(b.min, 1e-6) && a.max.abs_diff_eq(b.max, 1e-6));
    assert_close(
        measure::volume(&sheet.flat),
        measure::volume(&original_sheet.flat),
    );
    // Sides of flanges and of bends are told apart, as in any sheet metal body.
    let tops = sheet
        .faces
        .iter()
        .filter(|f| matches!(f, FaceTag::Top { .. }))
        .count();
    assert_eq!(
        tops,
        5 + 4 + 1,
        "five walls, four bends, one of them in two faces"
    );
}
