//! Assemblies through the rebuild engine: parts are built once and placed many times.

use std::sync::Arc;

use peet_math::{DQuat, DVec3, Frame};
use peet_model::{Engine, Model, Scalar, Status, samples};

fn at(x: f64, y: f64, z: f64) -> Frame {
    Frame {
        origin: DVec3::new(x, y, z),
        ..Frame::WORLD
    }
}

#[test]
fn instances_share_the_bodies_of_their_part() {
    let bracket = Arc::new(samples::bracket().0);
    let housing = Arc::new(samples::housing().0);
    let mut model = Model::new_assembly();
    assert!(model.is_assembly() && model.is_empty());
    let (b1, b2, h) = {
        let a = model.assembly_mut().unwrap();
        let bracket = a.define(bracket.clone());
        let housing = a.define(housing);
        (
            a.insert(bracket, Frame::WORLD).unwrap(),
            a.insert(bracket, at(200.0, 0.0, 0.0)).unwrap(),
            a.insert(
                housing,
                Frame {
                    origin: DVec3::new(0.0, 0.0, 100.0),
                    rotation: DQuat::from_rotation_x(std::f64::consts::FRAC_PI_2),
                },
            )
            .unwrap(),
        )
    };

    let mut engine = Engine::new();
    let built = engine.regenerate(&mut model).clone();
    assert!(
        built.bodies.is_empty(),
        "an assembly has no bodies of its own"
    );
    assert_eq!(built.instances.len(), 3);
    assert!(built.stats.rebuilt > 20, "both parts were built");
    for id in [b1, b2, h] {
        assert_eq!(built.component_status(id), Some(&Status::Ok));
    }
    // Two instances of the bracket: one body, two places.
    let (i1, i2) = (&built.instances[0], &built.instances[1]);
    assert_eq!((&i1.path[..], &i2.path[..]), (&[b1][..], &[b2][..]));
    assert!(Arc::ptr_eq(&i1.body, &i2.body));
    assert_eq!(i2.frame.origin.x, 200.0);
    assert!(!Arc::ptr_eq(&i1.body, &built.instances[2].body));
    // The parts were already built when they were inserted: nothing was written back.
    assert!(Arc::ptr_eq(
        &model.assembly().unwrap().definition_of(b1).unwrap().model,
        &bracket
    ));

    // Moving a component rebuilds nothing.
    model
        .assembly_mut()
        .unwrap()
        .component_mut(b2)
        .unwrap()
        .placement = at(300.0, 0.0, 0.0);
    let moved = engine.regenerate(&mut model).clone();
    assert_eq!((moved.stats.rebuilt, moved.stats.reused), (0, 0));
    assert_eq!(moved.instances[1].frame.origin.x, 300.0);
    assert!(Arc::ptr_eq(&moved.instances[1].body, &i1.body));

    // An edit to the part reaches both instances, and only that part is rebuilt.
    let mut wider = (*bracket).clone();
    let extrude = wider
        .features()
        .find(|f| f.extrude().is_some())
        .map(|f| f.id)
        .unwrap();
    wider
        .feature_mut(extrude)
        .unwrap()
        .extrude_mut()
        .unwrap()
        .params
        .depth = Scalar::new(12.0);
    let def = model.assembly().unwrap().component(b1).unwrap().definition;
    assert!(
        model
            .assembly_mut()
            .unwrap()
            .set_model(def, Arc::new(wider))
    );
    let edited = engine.regenerate(&mut model).clone();
    assert!(edited.stats.rebuilt > 0 && edited.stats.reused > 0);
    assert!(Arc::ptr_eq(
        &edited.instances[0].body,
        &edited.instances[1].body
    ));
    assert_ne!(edited.instances[0].body.stamp, i1.body.stamp);
    assert_eq!(
        edited.instances[2].body.stamp,
        built.instances[2].body.stamp
    );

    // Suppressed components are left out; deleted ones take their part's engine along.
    let a = model.assembly_mut().unwrap();
    a.component_mut(b1).unwrap().suppressed = true;
    a.remove(h);
    let fewer = engine.regenerate(&mut model).clone();
    assert_eq!(fewer.instances.len(), 1);
    assert_eq!(fewer.component_status(b1), Some(&Status::Suppressed));
    assert_eq!(fewer.component_status(h), None);
}

#[test]
fn a_sub_assembly_is_placed_as_one_thing() {
    let mut pair = Model::new_assembly();
    pair.name = "Pair".to_owned();
    {
        let a = pair.assembly_mut().unwrap();
        let bracket = a.define(Arc::new(samples::bracket().0));
        a.insert(bracket, Frame::WORLD).unwrap();
        a.insert(bracket, at(0.0, 100.0, 0.0)).unwrap();
    }
    let mut top = Model::new_assembly();
    let quarter = DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2);
    let sub = {
        let a = top.assembly_mut().unwrap();
        let pair = a.define(Arc::new(pair));
        a.insert(
            pair,
            Frame {
                origin: DVec3::new(10.0, 0.0, 0.0),
                rotation: quarter,
            },
        )
        .unwrap()
    };
    let mut engine = Engine::new();
    let built = engine.regenerate(&mut top).clone();
    assert_eq!(built.instances.len(), 2);
    assert_eq!(built.component_status(sub), Some(&Status::Ok));
    let second = &built.instances[1];
    assert_eq!(second.path.len(), 2);
    assert_eq!(second.path[0], sub);
    // (0, 100, 0) in the pair, turned a quarter about Z and moved 10 along X.
    assert!(
        second
            .frame
            .origin
            .abs_diff_eq(DVec3::new(-90.0, 0.0, 0.0), 1e-9),
        "{:?}",
        second.frame.origin
    );
    // Building the sub-assembly solved nothing new, so the model is as it was given.
    let again = engine.regenerate(&mut top).clone();
    assert_eq!(again.stats.rebuilt, 0);
}

#[test]
fn problems_in_a_part_show_on_its_components() {
    let mut broken = samples::bracket().0;
    let sketch = broken.features().next().map(|f| f.id).unwrap();
    broken.remove(sketch);
    let mut model = Model::new_assembly();
    let (bad, empty) = {
        let a = model.assembly_mut().unwrap();
        let broken = a.define(Arc::new(broken));
        let nothing = a.define(Arc::new(Model::new()));
        (
            a.insert(broken, Frame::WORLD).unwrap(),
            a.insert(nothing, Frame::WORLD).unwrap(),
        )
    };
    let mut engine = Engine::new();
    let built = engine.regenerate(&mut model);
    let message = |id| {
        built
            .component_status(id)
            .and_then(Status::message)
            .unwrap()
    };
    assert!(message(bad).contains("can't be built"), "{}", message(bad));
    assert!(message(empty).contains("no bodies"), "{}", message(empty));
    assert_eq!(built.component_problems().count(), 2);

    // A part is not an assembly.
    let mut part = samples::bracket().0;
    assert!(!part.is_assembly() && part.assembly_mut().is_none());
}
