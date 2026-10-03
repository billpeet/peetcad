//! The Phase 6 exit part: a turned bearing housing, checked against a hand calculation
//! and carried through every file format.
//!
//! The housing (`peet_model::samples::housing`): a flange of radius 50 and thickness 12
//! with a hub of radius 28 up to a height of 40 and a bore of radius 15, revolved from one
//! half section; a 4 mm fillet at the foot of the hub; 1.5 mm chamfers on the hub's and
//! the bore's top rims; six M8 counterbored holes on a bolt circle of radius 40, from a
//! circular pattern of one hole.
//!
//! By hand (Pappus's theorem for everything turned about the axis):
//!
//! - turned body: π·((50² − 15²)·12 + (28² − 15²)·28) = 134 937.69
//! - fillet: a sliver of area 4²·(1 − π/4) whose centroid is 0.8935 from its corner,
//!   swept at radius 28 + 0.8935: + 623.35
//! - chamfers: triangles of area 1.125 swept at radii 27.5 and 15.5: − 303.95
//! - holes: 6 × π·(7.5²·8.6 + 4.5²·3.4) = − 10 416.26
//!
//! 124 840.83 mm³ in all: 0.98 kg of steel. The model's volume must agree to 1 part in 10⁶, the STEP file must read back as the
//! same solid, and the part must survive a save and reopen with its caches.

use std::f64::consts::PI;

use peet_io::step::{StepOptions, StepSchema, write};
use peet_io::step_import;
use peet_kernel::Surface;
use peet_kernel::tessellate::tessellate;
use peet_kernel::validate::{measure, validate};
use peet_model::samples::{housing, housing_size as hs, housing_volume};
use peet_model::{FaceRole, Status};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * b.abs().max(1.0)
}

#[test]
fn housing_volume_matches_the_hand_calculation() {
    let (model, engine) = housing();
    let eval = engine.evaluation();
    for f in model.features() {
        assert!(
            matches!(eval.status(f.id), Some(Status::Ok)),
            "{}: {:?}",
            f.name,
            eval.status(f.id)
        );
    }
    assert_eq!(eval.bodies.len(), 1);
    let solid = &eval.bodies[0].solid;
    assert!(validate(solid).is_ok());
    // The numbers in the header.
    let turned = PI * ((2500.0 - 225.0) * 12.0 + (784.0 - 225.0) * 28.0);
    let fillet = 2.0
        * PI
        * (28.0 + 4.0 * (5.0 / 6.0 - PI / 4.0) / (1.0 - PI / 4.0))
        * 16.0
        * (1.0 - PI / 4.0);
    let chamfers = 2.0 * PI * (27.5 + 15.5) * 1.125;
    let holes = 6.0 * PI * (56.25 * 8.6 + 20.25 * 3.4);
    let expected = turned + fillet - chamfers - holes;
    assert!(close(expected, 124_840.825_472_832), "{expected}");
    assert!(close(expected, housing_volume()), "{expected}");
    let volume = measure::volume(solid);
    assert!(close(volume, expected), "{volume} vs {expected}");

    // What it is made of: flat faces, cylinders, the fillet's torus, the chamfers' cones.
    let count = |f: fn(&Surface) -> bool| solid.faces.iter().filter(|x| f(&x.surface)).count();
    assert_eq!(count(|s| matches!(s, Surface::Torus(_))), 1);
    assert_eq!(count(|s| matches!(s, Surface::Cone(_))), 2);
    // Bore, hub, rim, and a bore and a counterbore wall for each of the six holes.
    assert_eq!(count(|s| matches!(s, Surface::Cylinder(_))), 3 + 12);
    // Every face has a name of its own, so any of them can be referred to.
    let mut names = eval.bodies[0].face_names.clone();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), solid.faces.len());
    assert!(names.iter().any(|n| {
        n.origins()
            .iter()
            .any(|o| matches!(o.role, FaceRole::Blend(_)))
    }));

    // Mass properties: the centre of gravity is on the axis, below half height.
    let mass = peet_kernel::query::mass_properties(solid).unwrap();
    assert!(mass.centroid.x.abs() < 1e-3 && mass.centroid.y.abs() < 1e-3);
    assert!(mass.centroid.z > 5.0 && mass.centroid.z < hs::HEIGHT / 2.0);
    let size = mass.bounds.size();
    assert!(close(size.x, 100.0) && close(size.y, 100.0) && close(size.z, hs::HEIGHT));
    // At steel's density: just under a kilogram.
    let kg = mass.volume * 7850.0e-9;
    assert!((kg - 0.98).abs() < 1e-4, "{kg} kg");

    // It meshes without cracks, to the volume.
    let mesh = tessellate(solid, 0.02).unwrap();
    let mesh_volume: f64 = mesh
        .triangles()
        .iter()
        .map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0)
        .sum();
    assert!((mesh_volume - volume).abs() < 0.005 * volume);
}

#[test]
fn housing_goes_out_and_comes_back_as_step() {
    let (_, engine) = housing();
    let solid = &engine.evaluation().bodies[0].solid;
    let volume = measure::volume(solid);
    let area: f64 = solid.face_ids().map(|f| measure::face_area(solid, f)).sum();
    for schema in [StepSchema::Ap214, StepSchema::Ap242] {
        let text = write(
            &[("Housing", solid)],
            &StepOptions {
                schema,
                product_name: "Housing".to_owned(),
                ..StepOptions::default()
            },
        );
        for entity in [
            "TOROIDAL_SURFACE",
            "CONICAL_SURFACE",
            "CYLINDRICAL_SURFACE",
            "PLANE",
        ] {
            assert!(text.contains(entity), "{entity} is missing");
        }
        let back = step_import::read(&text).expect("the STEP file reads back");
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        assert_eq!(back.bodies.len(), 1);
        assert_eq!(back.bodies[0].name, "Housing");
        let read = &back.bodies[0].solid;
        assert!(validate(read).is_ok());
        assert_eq!(read.faces.len(), solid.faces.len());
        assert_eq!(read.edges.len(), solid.edges.len());
        assert!(close(measure::volume(read), volume));
        let read_area: f64 = read.face_ids().map(|f| measure::face_area(read, f)).sum();
        assert!(close(read_area, area));
    }
}

#[test]
fn imported_housing_takes_more_features() {
    // What a user does with someone else's part: open the STEP file and keep working.
    let (_, engine) = housing();
    let solid = &engine.evaluation().bodies[0].solid;
    let text = write(&[("Housing", solid)], &StepOptions::default());
    let back = step_import::read(&text).unwrap();
    let mut model = peet_model::Model::new();
    let solids = back
        .bodies
        .into_iter()
        .map(|b| peet_model::ImportedSolid {
            name: b.name,
            solid: b.solid,
        })
        .collect();
    let import = model.add_import("housing.step".to_owned(), solids);
    let mut engine = peet_model::Engine::new();
    let eval = engine.regenerate(&mut model);
    assert!(matches!(eval.status(import), Some(Status::Ok)));
    let body = eval.bodies[0].clone();
    // Round the outer rim of the flange's underside.
    let rim = body
        .solid
        .edge_ids()
        .find(|&e| {
            matches!(body.solid.edge(e).curve, peet_kernel::Curve3::Circle(c)
                if (c.radius - hs::FLANGE_R).abs() < 1e-9 && c.frame.origin.z.abs() < 1e-9)
        })
        .and_then(|e| body.edge_ref(e))
        .expect("the flange's lower rim");
    let fillet = model.add_blend(peet_model::BlendKind::Fillet, vec![rim]);
    let eval = engine.regenerate(&mut model);
    assert!(
        matches!(eval.status(fillet), Some(Status::Ok)),
        "{:?}",
        eval.status(fillet)
    );
    let r = 2.0_f64;
    let sliver = r * r * (1.0 - PI / 4.0);
    let centroid = r * (5.0 / 6.0 - PI / 4.0) / (1.0 - PI / 4.0);
    let expected = housing_volume() - 2.0 * PI * (hs::FLANGE_R - centroid) * sliver;
    assert!(close(measure::volume(&eval.bodies[0].solid), expected));
}

#[test]
fn housing_survives_save_and_reopen() {
    let (model, engine) = housing();
    let bodies = engine.evaluation().bodies.clone();
    let caches = peet_io::document::Caches {
        bodies: &bodies,
        meshes: Vec::new(),
    };
    let bytes = peet_io::document::save(
        &model,
        &peet_io::document::Metadata::default(),
        Some(caches),
    )
    .expect("the housing saves");
    let opened = peet_io::document::open(&bytes).expect("the housing opens");
    assert!(opened.warnings.is_empty(), "{:?}", opened.warnings);
    assert_eq!(opened.model, model);
    // The B-rep cache holds the curved faces as they were.
    let cached = opened.bodies.expect("the cache matches the model");
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].solid, bodies[0].solid);
    assert_eq!(cached[0].face_names, bodies[0].face_names);
    // And without caches the model rebuilds to the same part.
    let bytes = peet_io::document::save(&model, &peet_io::document::Metadata::default(), None)
        .expect("the housing saves");
    let mut reopened = peet_io::document::open(&bytes).unwrap().model;
    let mut engine = peet_model::Engine::new();
    let eval = engine.regenerate(&mut reopened);
    assert!(close(
        measure::volume(&eval.bodies[0].solid),
        housing_volume()
    ));
}
