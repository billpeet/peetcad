//! Sketch splines in files: the native format, and DXF import.

use peet_io::dxf_import::{ImportOptions, import};
use peet_math::{DVec2, Plane};
use peet_model::{Engine, Model, Operation, PlaneRef, Status, StdPlane};
use peet_sketch::region::find_regions;
use peet_sketch::{ConstraintKind, EntityKind, Sketch};

fn v(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

/// A DXF from `(code, value)` pairs, one per line.
fn dxf(pairs: &[(i32, &str)]) -> String {
    pairs.iter().map(|(c, v)| format!("{c}\n{v}\n")).collect()
}

fn entities(body: &[(i32, &str)]) -> String {
    let mut p: Vec<(i32, &str)> = vec![
        (0, "SECTION"),
        (2, "HEADER"),
        (9, "$INSUNITS"),
        (70, "4"),
        (0, "ENDSEC"),
        (0, "SECTION"),
        (2, "ENTITIES"),
    ];
    p.extend_from_slice(body);
    p.extend([(0, "ENDSEC"), (0, "EOF")]);
    dxf(&p)
}

fn splines(s: &Sketch) -> Vec<peet_sketch::EntityId> {
    s.entities()
        .filter(|(_, e)| e.kind() == EntityKind::Spline)
        .map(|(id, _)| id)
        .collect()
}

#[test]
fn a_part_with_splines_survives_save_and_reopen() {
    let mut model = Model::new();
    let sketch = model.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
    {
        let s = &mut model
            .feature_mut(sketch)
            .and_then(|f| f.sketch_mut())
            .unwrap()
            .sketch;
        let line = s.add_line(v(0.0, 0.0), v(30.0, 0.0));
        let open = s
            .add_spline(
                &[v(30.0, 0.0), v(22.0, 12.0), v(8.0, 9.0), v(0.0, 0.0)],
                false,
            )
            .unwrap();
        let (a, b) = s.endpoints(line).unwrap();
        let (c, d) = s.endpoints(open).unwrap();
        s.add_constraint(ConstraintKind::Coincident(b, c)).unwrap();
        s.add_constraint(ConstraintKind::Coincident(d, a)).unwrap();
        let closed = s
            .add_spline(&[v(40.0, 0.0), v(50.0, 5.0), v(45.0, 12.0)], true)
            .unwrap();
        s.set_construction(closed, true);
    }
    model.add_extrude(sketch, Operation::NewBody);
    let mut engine = Engine::new();
    engine.regenerate(&mut model);
    let bodies = engine.evaluation().bodies.clone();
    assert_eq!(bodies.len(), 1);

    let caches = peet_io::document::Caches {
        bodies: &bodies,
        meshes: Vec::new(),
    };
    let bytes = peet_io::document::save(
        &model,
        &peet_io::document::Metadata::default(),
        Some(caches),
    )
    .expect("the part saves");
    let opened = peet_io::document::open(&bytes).expect("the part opens");
    assert_eq!(opened.model, model);
    assert!(opened.warnings.is_empty(), "{:?}", opened.warnings);
    // The cached body is the one a rebuild gives, freeform wall and all.
    let cached = opened.bodies.expect("the cache matches the model");
    assert_eq!(cached[0].solid, bodies[0].solid);
    let mut again = opened.model;
    let mut engine = Engine::new();
    let eval = engine.regenerate(&mut again);
    assert!(
        again
            .features()
            .all(|f| !matches!(eval.status(f.id), Some(Status::Failed(_))))
    );
    assert_eq!(eval.bodies[0].solid, bodies[0].solid);

    // The text form round-trips too.
    let text = peet_io::document::to_text(&bytes).expect("as text");
    let back = peet_io::document::from_text(&text).expect("from text");
    assert_eq!(peet_io::document::open(&back).unwrap().model, model);
}

#[test]
fn dxf_splines_with_fit_points_become_sketch_splines() {
    let text = entities(&[
        // An open spline through four fit points, with control points it also carries.
        (0, "SPLINE"),
        (8, "PROFILE"),
        (70, "8"),
        (71, "3"),
        (74, "4"),
        (10, "0.0"),
        (20, "0.0"),
        (30, "0.0"),
        (10, "3.0"),
        (20, "9.0"),
        (30, "0.0"),
        (11, "0.0"),
        (21, "0.0"),
        (31, "0.0"),
        (11, "10.0"),
        (21, "8.0"),
        (31, "0.0"),
        (11, "20.0"),
        (21, "3.0"),
        (31, "0.0"),
        (11, "30.0"),
        (21, "0.0"),
        (31, "0.0"),
        // A line from the spline's end back to its start: one closed profile.
        (0, "LINE"),
        (8, "PROFILE"),
        (10, "30.0"),
        (20, "0.0"),
        (11, "0.0"),
        (21, "0.0"),
        // A closed spline (flag 1) through three points.
        (0, "SPLINE"),
        (8, "PROFILE"),
        (70, "9"),
        (11, "50.0"),
        (21, "0.0"),
        (11, "60.0"),
        (21, "5.0"),
        (11, "55.0"),
        (21, "12.0"),
        // Control points only: skipped.
        (0, "SPLINE"),
        (8, "PROFILE"),
        (70, "8"),
        (71, "3"),
        (10, "0.0"),
        (20, "0.0"),
        (10, "5.0"),
        (20, "5.0"),
    ]);
    let done = import(text.as_bytes(), &ImportOptions::default()).expect("the file imports");
    let s = &done.sketch;
    let found = splines(s);
    assert_eq!(found.len(), 2);
    assert_eq!(done.report.entities_imported, 3);
    let (open, closed) = (found[0], found[1]);
    let (points, is_closed) = s.spline_points(open).unwrap();
    assert!(!is_closed);
    let at: Vec<DVec2> = points.iter().map(|p| s.point(*p)).collect();
    assert_eq!(
        at,
        vec![v(0.0, 0.0), v(10.0, 8.0), v(20.0, 3.0), v(30.0, 0.0)]
    );
    assert_eq!(s.spline_points(closed).unwrap().0.len(), 3);
    assert!(s.spline_points(closed).unwrap().1);
    // The line's ends are joined to the spline's.
    let joined = s
        .constraints()
        .filter(|(_, c)| matches!(c.kind, ConstraintKind::Coincident(..)))
        .count();
    assert_eq!(
        joined,
        2,
        "{:?}",
        s.constraints().map(|(_, c)| c.kind).collect::<Vec<_>>()
    );
    let profile = find_regions(s);
    assert_eq!(profile.regions.len(), 2);
    assert!(profile.open_ends.is_empty());
    // The report says what happened to each.
    assert_eq!(done.report.skipped, vec![("SPLINE".to_owned(), 1)]);
    let all = done.report.warnings.join("\n");
    assert!(
        all.contains("2 splines were drawn again through their fit points"),
        "{all}"
    );
    assert!(
        all.contains("1 spline skipped: only splines saved with fit points"),
        "{all}"
    );
}

#[test]
fn dxf_spline_whose_ends_meet_is_closed_and_units_scale_it() {
    let text = dxf(&[
        (0, "SECTION"),
        (2, "HEADER"),
        (9, "$INSUNITS"),
        (70, "1"),
        (0, "ENDSEC"),
        (0, "SECTION"),
        (2, "ENTITIES"),
        (0, "SPLINE"),
        (8, "0"),
        (70, "8"),
        (11, "1.0"),
        (21, "0.0"),
        (11, "2.0"),
        (21, "1.0"),
        (11, "1.0"),
        (21, "2.0"),
        (11, "1.0"),
        (21, "0.0"),
        // Too few different points to be a curve.
        (0, "SPLINE"),
        (8, "0"),
        (11, "5.0"),
        (21, "5.0"),
        (11, "5.0"),
        (21, "5.0"),
        (0, "ENDSEC"),
        (0, "EOF"),
    ]);
    let done = import(text.as_bytes(), &ImportOptions::default()).expect("the file imports");
    let s = &done.sketch;
    let found = splines(s);
    assert_eq!(found.len(), 1);
    let (points, closed) = s.spline_points(found[0]).unwrap();
    assert!(closed, "the repeated first point closes it");
    assert_eq!(points.len(), 3);
    // Inches.
    assert_eq!(s.point(points[1]), v(50.8, 25.4));
    assert_eq!(find_regions(s).regions.len(), 1);
    assert!(
        done.report
            .skipped
            .iter()
            .any(|(k, n)| k.contains("zero size") && *n == 1),
        "{:?}",
        done.report.skipped
    );
}
