//! Convert to sheet metal through the file formats: a sheet metal part leaves as a STEP
//! file (a plain solid, as any other CAD system would send it), comes back through the
//! STEP importer, is converted, and gives the flat pattern it had before, as a DXF. The
//! converted part then survives a save and reopen in the native format.

use peet_io::dxf::{self, layer, read::Raw};
use peet_io::step::{StepOptions, write};
use peet_io::step_import::read;
use peet_kernel::validate::{measure, validate};
use peet_math::{DVec2, DVec3};
use peet_model::{Engine, FaceRef, FeatureId, ImportedSolid, Model, Status};

/// The roadmap's hand-calculated blank of the Phase 4 enclosure panel (K = 0.44, R 2,
/// t 1.5).
const FLAT: (f64, f64) = (244.356636, 194.356636);

/// A model holding the solids of a STEP file written from `solid`.
fn through_step(name: &str, solid: &peet_kernel::Solid) -> (Model, Engine) {
    let text = write(&[(name, solid)], &StepOptions::default());
    let imported = read(&text).expect("PeetCAD reads its own STEP files");
    assert_eq!(imported.bodies.len(), 1);
    let mut model = Model::new();
    model.add_import(
        format!("{name}.step"),
        imported
            .bodies
            .into_iter()
            .map(|b| ImportedSolid {
                name: b.name,
                solid: b.solid,
            })
            .collect(),
    );
    let mut engine = Engine::new();
    engine.regenerate(&mut model);
    (model, engine)
}

/// A reference to the flat face of body 0 with outward normal `n` through `p`.
#[track_caller]
fn face(engine: &Engine, n: DVec3, p: DVec3) -> FaceRef {
    let body = &engine.evaluation().bodies[0];
    for f in body.solid.face_ids() {
        if let peet_kernel::Surface::Plane(plane) = &body.solid.face(f).surface
            && body.solid.face_normal_at(f, p).dot(n) > 0.999
            && plane.signed_distance(p).abs() < 1e-6
        {
            return body.face_ref(f);
        }
    }
    panic!("no face with normal {n} through {p}");
}

#[track_caller]
fn status(engine: &Engine, id: FeatureId) -> &Status {
    engine.evaluation().status(id).expect("a status")
}

fn extent(entities: &[&Raw]) -> (DVec2, DVec2) {
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for e in entities {
        let pts = match e.kind.as_str() {
            "LINE" => vec![
                DVec2::new(e.num(10), e.num(20)),
                DVec2::new(e.num(11), e.num(21)),
            ],
            "ARC" | "CIRCLE" => {
                let (c, r) = (DVec2::new(e.num(10), e.num(20)), e.num(40));
                vec![c - DVec2::splat(r), c + DVec2::splat(r)]
            }
            _ => vec![],
        };
        for p in pts {
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    (lo, hi)
}

#[test]
fn enclosure_round_trips_through_step_and_unfolds_again() {
    let (_, sample) = peet_model::samples::enclosure();
    let original = sample.evaluation().bodies[0].clone();
    let want = original.sheet.as_ref().expect("sheet metal").report();
    assert!((want.flat_size.x - FLAT.0).abs() < 1e-6);
    assert!((want.flat_size.y - FLAT.1).abs() < 1e-6);

    let (mut model, mut engine) = through_step("Enclosure Panel", &original.solid);
    let imported = engine.evaluation().bodies[0].clone();
    assert!(
        imported.sheet.is_none(),
        "a STEP file carries no flat pattern"
    );
    validate(&imported.solid).expect("a valid imported solid");

    let top = face(&engine, DVec3::Z, DVec3::new(50.0, 50.0, 1.5));
    let convert = model.add_convert_to_sheet(Some(top));
    engine.regenerate(&mut model);
    assert_eq!(status(&engine, convert), &Status::Ok);
    let body = engine.evaluation().bodies[0].clone();
    validate(&body.solid).expect("a valid converted solid");
    let sheet = body.sheet.clone().expect("a sheet metal body");
    validate(&sheet.flat).expect("a valid flat pattern");
    let volume = measure::volume(&original.solid);
    assert!((measure::volume(&body.solid) - volume).abs() < 1e-6 * volume);

    // The same blank, bends and cutouts as the part had before it left.
    let got = sheet.report();
    assert!((got.flat_size.x - FLAT.0).abs() < 1e-6, "{}", got.flat_size);
    assert!((got.flat_size.y - FLAT.1).abs() < 1e-6, "{}", got.flat_size);
    assert!((got.flat_area - want.flat_area).abs() < 1e-6 * want.flat_area);
    assert_eq!(got.bends.len(), 4);
    assert_eq!(got.cutouts, want.cutouts);
    assert!((got.thickness - 1.5).abs() < 1e-9);
    assert!(got.bends.iter().all(|b| (b.radius - 2.0).abs() < 1e-9));
    assert!(got.bends.iter().all(|b| (b.angle - 90.0).abs() < 1e-7));

    // The DXF for the laser: the same outline size, four bend lines, the same holes.
    let text = dxf::flat_pattern(&sheet);
    let entities = dxf::read::entities(&text).expect("a well-formed DXF");
    let on = |l: &str| -> Vec<&Raw> { entities.iter().filter(|e| e.layer() == l).collect() };
    let (lo, hi) = extent(&on(layer::OUTLINE));
    assert!((hi.x - lo.x - FLAT.0).abs() < 1e-6);
    assert!((hi.y - lo.y - FLAT.1).abs() < 1e-6);
    let before = dxf::flat_pattern(original.sheet.as_ref().unwrap());
    let before = dxf::read::entities(&before).expect("a well-formed DXF");
    let count = |all: &[Raw], kind: &str| all.iter().filter(|e| e.kind == kind).count();
    assert_eq!(count(&entities, "CIRCLE"), count(&before, "CIRCLE"));
    assert_eq!(count(&entities, "CIRCLE"), 5);

    // Saved and reopened, the converted part rebuilds to the same body.
    let bodies = engine.evaluation().bodies.clone();
    let bytes = peet_io::document::save(
        &model,
        &peet_io::document::Metadata::default(),
        Some(peet_io::document::Caches {
            bodies: &bodies,
            meshes: Vec::new(),
        }),
    )
    .expect("the part saves");
    let opened = peet_io::document::open(&bytes).expect("the part opens");
    assert_eq!(opened.model, model);
    assert!(opened.warnings.is_empty(), "{:?}", opened.warnings);
    let mut reopened = opened.model;
    let mut engine = Engine::new();
    let eval = engine.regenerate(&mut reopened);
    assert_eq!(eval.bodies[0].solid, bodies[0].solid);
    assert_eq!(eval.bodies[0].face_names, bodies[0].face_names);
    assert!(eval.bodies[0].sheet.is_some());
}

#[test]
fn parts_with_forms_are_refused_with_a_reason() {
    // The Phase 5 chassis has louvers and dimples: pressed forms are not flat walls, so
    // the conversion says so instead of making a wrong part.
    let (_, sample) = peet_model::samples::chassis();
    let original = sample.evaluation().bodies[0].clone();
    let (mut model, mut engine) = through_step("Chassis", &original.solid);
    let volume = measure::volume(&engine.evaluation().bodies[0].solid);
    let convert = model.add_convert_to_sheet(None);
    engine.regenerate(&mut model);
    let Status::Failed(message) = status(&engine, convert) else {
        panic!("{:?}", status(&engine, convert));
    };
    assert!(message.len() > 40 && message.ends_with('.'), "{message}");
    // The imported body is still there, untouched.
    let body = &engine.evaluation().bodies[0];
    assert!(body.sheet.is_none());
    assert!((measure::volume(&body.solid) - volume).abs() < 1e-9 * volume);
}
