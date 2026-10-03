//! The Phase 7 exit assembly as the model builds it: every part built, every mate held,
//! nothing left free, the patterns' copies in their places.

use peet_math::DVec3;
use peet_model::Status;
use peet_model::samples::{self, chassis_size as cs, enclosure_size as es, housing_size as hs};

#[test]
fn the_enclosure_is_built_and_fully_mated() {
    let (mut model, mut engine) = samples::enclosure_assembly();
    let built = engine.regenerate(&mut model).clone();
    let a = model.assembly().unwrap();
    assert_eq!(a.components().len(), 13);
    assert_eq!(a.definitions().len(), 5);
    for c in a.components() {
        assert_eq!(
            built.component_status(c.id),
            Some(&Status::Ok),
            "{}",
            c.name
        );
    }
    for m in a.mates() {
        assert_eq!(built.mate_status(m.id), Some(&Status::Ok), "{}", m.name);
    }
    for p in a.patterns() {
        assert_eq!(built.pattern_status(p.id), Some(&Status::Ok), "{}", p.name);
    }
    assert_eq!(built.freedom, 0, "nothing is left free");
    for c in a.components() {
        assert_eq!(built.component_freedom(c.id), Some(0), "{}", c.name);
    }

    let at = |name: &str| {
        let c = a.components().find(|c| c.name == name);
        c.unwrap_or_else(|| panic!("no component {name}"))
            .placement
            .origin
    };
    let near = |name: &str, to: DVec3| {
        let p = at(name);
        assert!(p.distance(to) < 1e-6, "{name} is at {p}, not {to}");
    };
    let (hx, hy) = es::HOUSING_AT;
    let top = cs::WALL + es::COVER_T;
    near("Chassis", DVec3::ZERO);
    near("Cover", DVec3::new(0.0, 0.0, cs::WALL));
    near("Housing", DVec3::new(hx, hy, top));
    // The bolts: on the bolt circle, their heads on the floors of the counterbores.
    let seat = top + hs::FLANGE_T - 8.6;
    let bolts = a.patterns().find(|p| p.name == "Bolts").unwrap();
    assert_eq!(bolts.instances.len(), 5);
    near("Bolt-1", DVec3::new(hx + hs::BOLT_CIRCLE_R, hy, seat));
    for i in &bolts.instances {
        let angle = std::f64::consts::TAU * f64::from(i.place[0]) / 6.0;
        let expected = DVec3::new(
            hx + hs::BOLT_CIRCLE_R * angle.cos(),
            hy + hs::BOLT_CIRCLE_R * angle.sin(),
            seat,
        );
        let p = a.component(i.component).unwrap().placement.origin;
        assert!(p.distance(expected) < 1e-6, "{:?}: {p}", i.place);
    }
    // The screws: in the four mounting holes, their heads on the base.
    let screws = a.patterns().find(|p| p.name == "Screws").unwrap();
    assert_eq!(screws.instances.len(), 3);
    let (mx, my) = es::MOUNT_AT;
    near("Screw-1", DVec3::new(mx, my, cs::THICKNESS));
    for i in &screws.instances {
        let expected = DVec3::new(
            mx + es::MOUNT_STEP.0 * f64::from(i.place[0]),
            my + es::MOUNT_STEP.1 * f64::from(i.place[1]),
            cs::THICKNESS,
        );
        let p = a.component(i.component).unwrap().placement.origin;
        assert!(p.distance(expected) < 1e-6, "{:?}: {p}", i.place);
    }
    assert_eq!(a.explode_steps().len(), 4);

    // Solved as it is built: building it again changes nothing.
    let before = model.clone();
    engine.regenerate(&mut model);
    assert_eq!(model, before);
}
