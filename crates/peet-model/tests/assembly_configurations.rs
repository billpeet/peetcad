//! Configurations in assemblies: a component shows its part in a configuration of its
//! own choosing, and an assembly has configurations of its own that differ in what its
//! components and mates are.

use std::sync::Arc;

use peet_math::{DVec2, DVec3, Frame, Plane};
use peet_model::{
    ComponentState, ConfigId, Engine, Model, Operation, PlaneRef, Scalar, Scope, Slot, StdPlane,
};

fn at(x: f64, y: f64, z: f64) -> Frame {
    Frame {
        origin: DVec3::new(x, y, z),
        ..Frame::WORLD
    }
}

/// A 40 x 30 plate 5 thick, with a second configuration, "Thick", in which it is 20 thick.
fn bracket() -> (Model, ConfigId, ConfigId) {
    let mut m = Model::new();
    m.name = "Plate".to_owned();
    let s = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
    let sketch = &mut m.feature_mut(s).unwrap().sketch_mut().unwrap().sketch;
    peet_sketch::shapes::rectangle(sketch, DVec2::ZERO, DVec2::new(40.0, 30.0));
    let extrude = m.add_extrude(s, Operation::Add);
    m.feature_mut(extrude)
        .unwrap()
        .extrude_mut()
        .unwrap()
        .params
        .depth = Scalar::new(5.0);
    let default = m.active_configuration().id;
    let thick = m.add_configuration("Thick", None).unwrap();
    let depth = m
        .values(extrude)
        .into_iter()
        .find(|v| v.name == "depth")
        .unwrap()
        .slot;
    assert_eq!(depth, Slot::Field("depth".to_owned()));
    m.set_value(
        extrude,
        &depth,
        Scalar::new(20.0),
        &Scope::Only(vec![thick]),
    )
    .unwrap();
    Engine::new().regenerate(&mut m);
    (m, default, thick)
}

fn height(built: &peet_model::Evaluation, index: usize) -> f64 {
    let b = built.instances[index].body.solid.bounds();
    (b.max - b.min).length()
}

/// How big (its box's diagonal) the bracket is in "Thick", built on its own.
fn thick_height() -> f64 {
    let (part, _, thick) = bracket();
    let mut m = part.with_configuration(thick);
    let built = Engine::new().regenerate(&mut m).clone();
    let b = built.bodies[0].solid.bounds();
    (b.max - b.min).length()
}

#[test]
fn components_of_one_part_show_different_configurations() {
    let (part, _, thick) = bracket();
    let part = Arc::new(part);
    let mut model = Model::new_assembly();
    let (a, b) = {
        let asm = model.assembly_mut().unwrap();
        let d = asm.define(part.clone());
        (
            asm.insert(d, Frame::WORLD).unwrap(),
            asm.insert(d, at(200.0, 0.0, 0.0)).unwrap(),
        )
    };
    let mut engine = Engine::new();
    let built = engine.regenerate(&mut model).clone();
    let thin = height(&built, 0);
    assert!(Arc::ptr_eq(
        &built.instances[0].body,
        &built.instances[1].body
    ));

    // The second shows "Thick": built once more, and the definition is not changed.
    model
        .assembly_mut()
        .unwrap()
        .component_mut(b)
        .unwrap()
        .configuration = Some(thick);
    let built = engine.regenerate(&mut model).clone();
    assert!(
        (height(&built, 1) - thick_height()).abs() < 1e-9,
        "{}",
        height(&built, 1)
    );
    assert!((thin - thick_height()).abs() > 1.0);
    assert!((height(&built, 0) - thin).abs() < 1e-9);
    let asm = model.assembly().unwrap();
    assert!(Arc::ptr_eq(&asm.definition_of(a).unwrap().model, &part));
    assert_eq!(
        asm.configuration_name(asm.component(b).unwrap()),
        Some("Thick")
    );
    assert_eq!(
        asm.configuration_name(asm.component(a).unwrap()),
        Some("Default")
    );
    // A third in the same configuration shares its body: one build per configuration.
    {
        let asm = model.assembly_mut().unwrap();
        let d = asm.component(b).unwrap().definition;
        let c = asm.insert(d, at(400.0, 0.0, 0.0)).unwrap();
        asm.component_mut(c).unwrap().configuration = Some(thick);
    }
    let again = engine.regenerate(&mut model).clone();
    assert_eq!(again.stats.rebuilt, 0);
    assert!(Arc::ptr_eq(
        &again.instances[1].body,
        &again.instances[2].body
    ));
}

#[test]
fn an_assemblys_configurations_differ_in_its_components() {
    let (part, default_part, thick) = bracket();
    let mut model = Model::new_assembly();
    let (a, b) = {
        let asm = model.assembly_mut().unwrap();
        let d = asm.define(Arc::new(part));
        (
            asm.insert(d, Frame::WORLD).unwrap(),
            asm.insert(d, at(200.0, 0.0, 0.0)).unwrap(),
        )
    };
    let first = model.active_configuration().id;
    let right = model.add_configuration("Right", None).unwrap();
    assert!(model.activate_configuration(right));
    // In "Right": b suppressed, a thick and somewhere else; the first configuration keeps
    // them as they were.
    model
        .set_component(b, &Scope::This, |s| s.suppressed = true)
        .unwrap();
    model
        .set_component(a, &Scope::This, |s| {
            s.configuration = Some(thick);
            s.placement = at(0.0, 50.0, 0.0);
        })
        .unwrap();
    assert!(model.component_differs(a) && model.component_differs(b));
    let mut engine = Engine::new();
    let built = engine.regenerate(&mut model).clone();
    assert_eq!(built.instances.len(), 1);
    assert!((height(&built, 0) - thick_height()).abs() < 1e-9);
    assert_eq!(built.instances[0].frame.origin.y, 50.0);

    let was = model.component_in(a, first).unwrap();
    assert_eq!(was.configuration, None);
    assert_eq!(was.placement, Frame::WORLD);
    assert!(model.activate_configuration(first));
    let built = engine.regenerate(&mut model).clone();
    assert_eq!(built.instances.len(), 2);
    assert_eq!(built.instances[0].frame.origin, DVec3::ZERO);
    assert!((height(&built, 0) - thick_height()).abs() > 1.0);
    assert_eq!(
        model.component_in(a, right).unwrap().configuration,
        Some(thick)
    );

    // In all of them: no longer differs.
    model
        .set_component(b, &Scope::All, |s| s.suppressed = false)
        .unwrap();
    assert!(!model.component_differs(b));
    // A change to the model made directly is given its scope afterwards.
    let before = ComponentState::of(model.assembly().unwrap().component(b).unwrap());
    model
        .assembly_mut()
        .unwrap()
        .component_mut(b)
        .unwrap()
        .fixed = true;
    assert!(!model.component_differs(b), "changed in all, as made");
    model
        .assembly_mut()
        .unwrap()
        .component_mut(b)
        .unwrap()
        .fixed = before.fixed;
    // A configuration copies another's.
    let copy = model.add_configuration("Right 2", Some(right)).unwrap();
    assert_eq!(model.component_in(a, copy), model.component_in(a, right));
    // Removing the one that differed tidies up.
    model.remove_configuration(right).unwrap();
    model.remove_configuration(copy).unwrap();
    assert!(!model.component_differs(a));
    let _ = default_part;
}

#[test]
fn a_component_keeps_its_place_in_each_configuration() {
    // A solve that moves a component moves it in the active configuration only.
    let (part, ..) = bracket();
    let mut model = Model::new_assembly();
    let b = {
        let asm = model.assembly_mut().unwrap();
        let d = asm.define(Arc::new(part));
        asm.insert(d, Frame::WORLD).unwrap();
        asm.insert(d, at(200.0, 0.0, 0.0)).unwrap()
    };
    let first = model.active_configuration().id;
    let other = model.add_configuration("Other", None).unwrap();
    model
        .set_component(b, &Scope::This, |s| s.placement = at(0.0, 0.0, 99.0))
        .unwrap();
    assert_eq!(
        model.component_in(b, other).unwrap().placement,
        at(200.0, 0.0, 0.0)
    );
    assert_eq!(model.active_configuration().id, first);
    model.activate_configuration(other);
    assert_eq!(
        model.assembly().unwrap().component(b).unwrap().placement,
        at(200.0, 0.0, 0.0)
    );
}
