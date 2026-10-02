//! Phase 3 exit criterion: change a dimension in the first sketch, and a 20-feature part
//! rebuilds correctly in under 100 ms.

use peet_kernel::validate::{measure, validate};
use peet_model::samples::{bracket, bracket_volume};
use peet_model::{Model, Status};

fn assert_built(model: &Model, engine: &peet_model::Engine) {
    let eval = engine.evaluation();
    for f in model.features() {
        match eval.status(f.id) {
            Some(Status::Ok) => {}
            other => panic!("{}: {other:?}", f.name),
        }
    }
    assert_eq!(eval.bodies.len(), 1);
    validate(&eval.bodies[0].solid).expect("a valid body");
}

fn volume(engine: &peet_model::Engine) -> f64 {
    measure::volume(&engine.evaluation().bodies[0].solid)
}

fn set_width(model: &mut Model, width: f64) {
    let first = model.features().next().unwrap().id;
    let params = model.parameters.clone();
    let sketch = &mut model
        .feature_mut(first)
        .and_then(|f| f.sketch_mut())
        .unwrap()
        .sketch;
    let d1 = sketch.dimension_by_name("d1").unwrap();
    peet_sketch::expr::set_dimension_input(sketch, &params, d1, &width.to_string()).unwrap();
}

#[test]
fn twenty_feature_part_rebuilds_after_a_first_sketch_change() {
    let (mut model, mut engine) = bracket();
    assert_eq!(model.len(), 20);
    assert_built(&model, &engine);
    let v = volume(&engine);
    assert!((v - bracket_volume(120.0)).abs() < 1e-6, "{v}");

    let mut best = f64::MAX;
    for (i, width) in [150.0, 135.0, 170.0, 120.0, 160.0].into_iter().enumerate() {
        set_width(&mut model, width);
        let stats = engine.regenerate(&mut model).stats;
        best = best.min(stats.ms);
        assert_built(&model, &engine);
        let v = volume(&engine);
        assert!(
            (v - bracket_volume(width)).abs() < 1e-6,
            "width {width}: volume {v}, expected {}",
            bracket_volume(width)
        );
        if i == 0 {
            // Every solid feature is downstream of the plate; sketches on faces that
            // didn't move are reused.
            assert!(stats.rebuilt >= 10 && stats.reused >= 5, "{stats:?}");
        }
    }
    println!("20-feature rebuild after a first-sketch change: {best:.1} ms (best of 5)");
    // Debug builds don't optimise the kernel, so only optimised builds are held to the
    // budget (`cargo test --release`, and `cargo bench -p peet-model`).
    if !cfg!(debug_assertions) {
        assert!(best < 100.0, "{best:.1} ms is over the 100 ms budget");
    }
}

#[test]
fn the_slot_follows_the_end_face() {
    let (mut model, mut engine) = bracket();
    set_width(&mut model, 200.0);
    engine.regenerate(&mut model);
    let body = &engine.evaluation().bodies[0];
    // The slot's floor is 15 mm in from the new end face.
    let floor = body
        .solid
        .face_ids()
        .find(|f| model.describe_face(body.face_name(*f)) == "the end face of Cut-Extrude5");
    let floor = floor.expect("the slot floor");
    let c = body.face_center(floor);
    assert!((c.x - 185.0).abs() < 1e-9, "{c}");
}
