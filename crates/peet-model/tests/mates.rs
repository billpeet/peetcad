//! Mates through the rebuild engine: components are moved to where their mates hold, as
//! little as possible, and stay there.

use std::sync::Arc;

use peet_kernel::Surface;
use peet_math::{DQuat, DVec2, DVec3, Frame, Plane};
use peet_model::{
    CompId, Engine, Evaluation, MateEnd, MateGeom, MateId, MateKind, Model, Operation, PlaneRef,
    Scalar, Status, StdPlane,
};

/// A plate from (0, 0, 0) to (w, h, t), with a hole of radius `hole` through its middle
/// if that is not zero.
fn plate(name: &str, w: f64, h: f64, t: f64, hole: f64) -> Model {
    let mut m = Model::new();
    name.clone_into(&mut m.name);
    let s = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
    let sketch = &mut m.feature_mut(s).unwrap().sketch_mut().unwrap().sketch;
    peet_sketch::shapes::rectangle(sketch, DVec2::ZERO, DVec2::new(w, h));
    if hole > 0.0 {
        sketch.add_circle(DVec2::new(w / 2.0, h / 2.0), hole);
    }
    let e = m.add_extrude(s, Operation::Add);
    m.feature_mut(e)
        .unwrap()
        .extrude_mut()
        .unwrap()
        .params
        .depth = Scalar::new(t);
    Engine::new().regenerate(&mut m);
    m
}

/// A pin of radius `r` and length `l` standing on the top plane at the origin.
fn pin(r: f64, l: f64) -> Model {
    let mut m = Model::new();
    m.name = "Pin".to_owned();
    let s = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
    let sketch = &mut m.feature_mut(s).unwrap().sketch_mut().unwrap().sketch;
    sketch.add_circle(DVec2::ZERO, r);
    let e = m.add_extrude(s, Operation::Add);
    m.feature_mut(e)
        .unwrap()
        .extrude_mut()
        .unwrap()
        .params
        .depth = Scalar::new(l);
    Engine::new().regenerate(&mut m);
    m
}

/// The flat face of a part whose outward normal is `normal`, or its round face if
/// `normal` is zero.
fn face(part: &Model, normal: DVec3) -> MateGeom {
    let mut model = part.clone();
    let mut engine = Engine::new();
    let body = engine.regenerate(&mut model).bodies[0].clone();
    let found = body
        .solid
        .face_ids()
        .find(|f| {
            let face = body.solid.face(*f);
            match &face.surface {
                Surface::Plane(p) => {
                    let n = if face.reversed {
                        -p.normal()
                    } else {
                        p.normal()
                    };
                    n.abs_diff_eq(normal, 1e-9)
                }
                Surface::Cylinder(_) => normal == DVec3::ZERO,
                _ => false,
            }
        })
        .unwrap_or_else(|| panic!("{} has no face {normal}", part.name));
    MateGeom::Face(body.face_ref(found))
}

fn end(component: CompId, geom: MateGeom) -> MateEnd {
    MateEnd {
        path: vec![component],
        geom: Some(geom),
    }
}

fn at(x: f64, y: f64, z: f64) -> Frame {
    Frame {
        origin: DVec3::new(x, y, z),
        ..Frame::WORLD
    }
}

/// An assembly of two 40 x 30 x 5 plates: the first fixed at the origin, the second
/// somewhere else and a little askew.
fn two_plates() -> (Model, Model, CompId, CompId) {
    let part = plate("Plate", 40.0, 30.0, 5.0, 0.0);
    let mut model = Model::new_assembly();
    let a = model.assembly_mut().unwrap();
    let d = a.define(Arc::new(part.clone()));
    let base = a.insert(d, Frame::WORLD).unwrap();
    let askew = Frame {
        origin: DVec3::new(70.0, -20.0, 33.0),
        rotation: DQuat::from_euler(peet_math::EulerRot::XYZ, 0.2, -0.3, 0.4),
    };
    let top = a.insert(d, askew).unwrap();
    (model, part, base, top)
}

fn frame(model: &Model, id: CompId) -> Frame {
    model.assembly().unwrap().component(id).unwrap().placement
}

fn mate(model: &mut Model, kind: MateKind, a: MateEnd, b: MateEnd) -> MateId {
    model.assembly_mut().unwrap().add_mate(kind, a, b)
}

fn ok(built: &Evaluation, id: MateId) {
    assert_eq!(built.mate_status(id), Some(&Status::Ok));
}

const UP: DVec3 = DVec3::Z;
const DOWN: DVec3 = DVec3::NEG_Z;

#[test]
fn faces_are_brought_together_one_freedom_at_a_time() {
    let (mut model, part, base, top) = two_plates();
    let mut engine = Engine::new();
    assert_eq!(engine.regenerate(&mut model).freedom, 6);
    let before = frame(&model, top);

    // The top of the first against the bottom of the second.
    let m1 = mate(
        &mut model,
        MateKind::Coincident,
        end(base, face(&part, UP)),
        end(top, face(&part, DOWN)),
    );
    let built = engine.regenerate(&mut model).clone();
    ok(&built, m1);
    assert_eq!(built.freedom, 3, "it slides and turns on the face");
    let f = frame(&model, top);
    assert!((f.to_world(DVec3::ZERO).z - 5.0).abs() < 1e-9, "{f:?}");
    assert!(f.vector_to_world(UP).abs_diff_eq(UP, 1e-9));
    // It went the short way: about where it was, not somewhere else on the plane.
    assert!((f.origin - before.origin).length() < 40.0, "{f:?}");
    assert_eq!(frame(&model, base), Frame::WORLD, "the fixed one stays");
    assert_eq!(built.instances[1].frame, f, "drawn where the mates put it");

    // Then two side faces, the same way round.
    let m2 = mate(
        &mut model,
        MateKind::Coincident,
        end(base, face(&part, DVec3::NEG_X)),
        end(top, face(&part, DVec3::NEG_X)),
    );
    model.assembly_mut().unwrap().mate_mut(m2).unwrap().flip = true;
    let built = engine.regenerate(&mut model).clone();
    ok(&built, m2);
    assert_eq!(built.freedom, 1, "it slides along the edge");
    let m3 = mate(
        &mut model,
        MateKind::Distance(Scalar::new(12.0)),
        end(base, face(&part, DVec3::NEG_Y)),
        end(top, face(&part, DVec3::Y)),
    );
    let built = engine.regenerate(&mut model).clone();
    ok(&built, m3);
    assert_eq!(built.freedom, 0);
    // The second plate's far side is 12 before the first's near side.
    let f = frame(&model, top);
    assert!(
        f.origin.abs_diff_eq(DVec3::new(0.0, -42.0, 5.0), 1e-8),
        "{f:?}"
    );
    assert!(
        f.rotation.abs_diff_eq(DQuat::IDENTITY, 1e-9)
            || f.rotation.abs_diff_eq(-DQuat::IDENTITY, 1e-9)
    );

    // Solved, it stays exactly as it is: nothing is written, however often it is rebuilt.
    let solved = model.clone();
    engine.regenerate(&mut model);
    Engine::new().regenerate(&mut model);
    assert_eq!(model, solved);

    // A distance changed moves it; a redundant mate changes nothing.
    let a = model.assembly_mut().unwrap();
    a.mate_mut(m3).unwrap().kind = MateKind::Distance(Scalar::new(2.0));
    let again = mate(
        &mut model,
        MateKind::Parallel,
        end(base, face(&part, UP)),
        end(top, face(&part, DOWN)),
    );
    let built = engine.regenerate(&mut model).clone();
    ok(&built, again);
    assert_eq!(built.freedom, 0);
    assert!(
        frame(&model, top)
            .origin
            .abs_diff_eq(DVec3::new(0.0, -32.0, 5.0), 1e-8)
    );
}

#[test]
fn a_part_the_wrong_way_round_is_turned_over() {
    let (mut model, part, base, top) = two_plates();
    // Top against top: the second plate has to end up upside down, on the first.
    let m = mate(
        &mut model,
        MateKind::Coincident,
        end(base, face(&part, UP)),
        end(top, face(&part, UP)),
    );
    let mut engine = Engine::new();
    ok(&engine.regenerate(&mut model).clone(), m);
    let f = frame(&model, top);
    assert!(f.vector_to_world(UP).abs_diff_eq(DOWN, 1e-9), "{f:?}");
    assert!((f.to_world(DVec3::new(0.0, 0.0, 5.0)).z - 5.0).abs() < 1e-9);
    // Flipped, the faces look the same way: it is turned back, into the first.
    model.assembly_mut().unwrap().mate_mut(m).unwrap().flip = true;
    ok(&engine.regenerate(&mut model).clone(), m);
    let f = frame(&model, top);
    assert!(f.vector_to_world(UP).abs_diff_eq(UP, 1e-9), "{f:?}");
    assert!(f.origin.z.abs() < 1e-9);
}

#[test]
fn a_pin_goes_in_a_hole() {
    let holed = plate("Holed", 40.0, 30.0, 5.0, 4.0);
    let pin = pin(4.0, 20.0);
    let mut model = Model::new_assembly();
    let (base, p) = {
        let a = model.assembly_mut().unwrap();
        let dh = a.define(Arc::new(holed.clone()));
        let dp = a.define(Arc::new(pin.clone()));
        let tilted = Frame {
            origin: DVec3::new(-30.0, 10.0, 40.0),
            rotation: DQuat::from_rotation_y(0.5),
        };
        (
            a.insert(dh, Frame::WORLD).unwrap(),
            a.insert(dp, tilted).unwrap(),
        )
    };
    let round = DVec3::ZERO;
    let axes = mate(
        &mut model,
        MateKind::Concentric,
        end(base, face(&holed, round)),
        end(p, face(&pin, round)),
    );
    let mut engine = Engine::new();
    let built = engine.regenerate(&mut model).clone();
    ok(&built, axes);
    assert_eq!(built.freedom, 2, "it slides along the hole and turns in it");
    let f = frame(&model, p);
    assert!(f.vector_to_world(UP).cross(UP).length() < 1e-9);
    assert!(
        (f.origin.truncate() - DVec2::new(20.0, 15.0)).length() < 1e-8,
        "{f:?}"
    );

    // Its end flush with the underside: one freedom left, the turn about its axis.
    let flush = mate(
        &mut model,
        MateKind::Coincident,
        end(base, face(&holed, DOWN)),
        end(p, face(&pin, DOWN)),
    );
    model.assembly_mut().unwrap().mate_mut(flush).unwrap().flip = true;
    let built = engine.regenerate(&mut model).clone();
    ok(&built, flush);
    assert_eq!(built.freedom, 1);
    let f = frame(&model, p);
    assert!(
        f.origin.abs_diff_eq(DVec3::new(20.0, 15.0, 0.0), 1e-8),
        "{f:?}"
    );
    assert!(f.vector_to_world(UP).abs_diff_eq(UP, 1e-9));

    // A flat face can't be concentric with anything.
    let wrong = mate(
        &mut model,
        MateKind::Concentric,
        end(base, face(&holed, UP)),
        end(p, face(&pin, round)),
    );
    let built = engine.regenerate(&mut model).clone();
    let message = built.mate_status(wrong).unwrap().message().unwrap();
    assert!(message.contains("two axes"), "{message}");
    ok(&built, flush);
}

#[test]
fn angles_and_fastened_components() {
    let (mut model, part, base, top) = two_plates();
    let lean = mate(
        &mut model,
        MateKind::Angle(Scalar::new(30.0)),
        end(base, face(&part, UP)),
        end(top, face(&part, UP)),
    );
    let mut engine = Engine::new();
    let built = engine.regenerate(&mut model).clone();
    ok(&built, lean);
    assert_eq!(built.freedom, 5);
    let up = frame(&model, top).vector_to_world(UP);
    assert!((up.dot(UP).acos().to_degrees() - 30.0).abs() < 1e-7);
    for bad in [0.0, 180.0, 200.0] {
        model.assembly_mut().unwrap().mate_mut(lean).unwrap().kind =
            MateKind::Angle(Scalar::new(bad));
        let built = engine.regenerate(&mut model);
        let m = built.mate_status(lean).unwrap().message().unwrap();
        assert!(m.contains("parallel mate"), "{m}");
    }
    model.assembly_mut().unwrap().remove_mate(lean).unwrap();

    // Fastened where it is: when the first is moved, the second goes with it.
    let relative = frame(&model, base).inverse().compose(&frame(&model, top));
    let whole = |c| MateEnd {
        path: vec![c],
        geom: None,
    };
    let held = mate(
        &mut model,
        MateKind::Fasten(relative),
        whole(base),
        whole(top),
    );
    let built = engine.regenerate(&mut model).clone();
    ok(&built, held);
    assert_eq!(built.freedom, 0);
    let moved = Frame {
        origin: DVec3::new(100.0, 50.0, -20.0),
        rotation: DQuat::from_rotation_z(1.0),
    };
    model
        .assembly_mut()
        .unwrap()
        .component_mut(base)
        .unwrap()
        .placement = moved;
    ok(&engine.regenerate(&mut model).clone(), held);
    let now = frame(&model, base).inverse().compose(&frame(&model, top));
    assert!(now.origin.abs_diff_eq(relative.origin, 1e-8), "{now:?}");
    assert!(now.rotation.angle_between(relative.rotation) < 1e-9);
    assert_eq!(frame(&model, base), moved);

    // With neither fixed, they are still one thing with six freedoms.
    model
        .assembly_mut()
        .unwrap()
        .component_mut(base)
        .unwrap()
        .fixed = false;
    assert_eq!(engine.regenerate(&mut model).freedom, 6);
}

#[test]
fn mates_that_cant_all_hold_are_told_apart() {
    let (mut model, part, base, top) = two_plates();
    let touching = mate(
        &mut model,
        MateKind::Coincident,
        end(base, face(&part, UP)),
        end(top, face(&part, DOWN)),
    );
    let apart = mate(
        &mut model,
        MateKind::Distance(Scalar::new(10.0)),
        end(base, face(&part, UP)),
        end(top, face(&part, DOWN)),
    );
    let side = mate(
        &mut model,
        MateKind::Coincident,
        end(base, face(&part, DVec3::X)),
        end(top, face(&part, DVec3::X)),
    );
    model.assembly_mut().unwrap().mate_mut(side).unwrap().flip = true;
    let mut engine = Engine::new();
    let built = engine.regenerate(&mut model).clone();
    // The earlier one wins; the one that contradicts it is flagged; the rest still hold.
    ok(&built, touching);
    ok(&built, side);
    let m = built.mate_status(apart).unwrap().message().unwrap();
    assert!(m.contains("can't hold together"), "{m}");
    assert_eq!(built.mate_failures().count(), 1);
    assert_eq!(built.freedom, 1);
    assert!((frame(&model, top).origin.z - 5.0).abs() < 1e-8);

    // Suppressed, it is no longer in the way; a mate on one component is refused.
    let a = model.assembly_mut().unwrap();
    a.mate_mut(apart).unwrap().suppressed = true;
    let own = a.add_mate(
        MateKind::Parallel,
        end(top, face(&part, UP)),
        end(top, face(&part, DOWN)),
    );
    let built = engine.regenerate(&mut model).clone();
    assert_eq!(built.mate_status(apart), Some(&Status::Suppressed));
    let m = built.mate_status(own).unwrap().message().unwrap();
    assert!(m.contains("Both ends are on Plate-2"), "{m}");

    // Two fixed components that are not where a mate wants them.
    let a = model.assembly_mut().unwrap();
    a.remove_mate(own);
    a.component_mut(top).unwrap().fixed = true;
    a.component_mut(top).unwrap().placement = at(0.0, 0.0, 50.0);
    let built = engine.regenerate(&mut model).clone();
    assert!(built.mate_status(touching).unwrap().is_failed());
    assert_eq!(
        frame(&model, top),
        at(0.0, 0.0, 50.0),
        "a fixed component is not moved"
    );

    // Deleting a component takes its mates along; a suppressed one's wait for it.
    let a = model.assembly_mut().unwrap();
    a.component_mut(top).unwrap().fixed = false;
    a.component_mut(top).unwrap().suppressed = true;
    let built = engine.regenerate(&mut model).clone();
    assert_eq!(built.mate_status(touching), Some(&Status::Suppressed));
    let a = model.assembly_mut().unwrap();
    assert_eq!(a.mates_of(top).count(), 3);
    a.remove(top);
    assert_eq!(a.mates().count(), 0);
}

#[test]
fn mates_follow_their_faces_through_edits_to_the_part() {
    let (mut model, part, base, top) = two_plates();
    let m = mate(
        &mut model,
        MateKind::Coincident,
        end(base, face(&part, UP)),
        end(top, face(&part, DOWN)),
    );
    let mut engine = Engine::new();
    engine.regenerate(&mut model);
    assert!((frame(&model, top).origin.z - 5.0).abs() < 1e-9);

    // The plate made thicker: the second one rides up on the first.
    let mut thicker = part.clone();
    let extrude = thicker
        .features()
        .find(|f| f.extrude().is_some())
        .unwrap()
        .id;
    thicker
        .feature_mut(extrude)
        .unwrap()
        .extrude_mut()
        .unwrap()
        .params
        .depth = Scalar::new(8.0);
    let def = model
        .assembly()
        .unwrap()
        .component(base)
        .unwrap()
        .definition;
    model
        .assembly_mut()
        .unwrap()
        .set_model(def, Arc::new(thicker));
    ok(&engine.regenerate(&mut model).clone(), m);
    assert!((frame(&model, top).origin.z - 8.0).abs() < 1e-9);

    // The part left without that face: the mate says so, and nothing moves.
    let before = frame(&model, top);
    model
        .assembly_mut()
        .unwrap()
        .set_model(def, Arc::new(Model::new()));
    let built = engine.regenerate(&mut model).clone();
    let message = built.mate_status(m).unwrap().message().unwrap();
    assert!(message.contains("is gone"), "{message}");
    assert_eq!(frame(&model, top), before);
}

#[test]
fn a_part_inside_a_sub_assembly_can_be_mated() {
    let part = plate("Plate", 40.0, 30.0, 5.0, 0.0);
    // A sub-assembly of one plate, standing 100 up in it.
    let mut sub = Model::new_assembly();
    sub.name = "Stand".to_owned();
    let inner = {
        let a = sub.assembly_mut().unwrap();
        let d = a.define(Arc::new(part.clone()));
        a.insert(d, at(0.0, 0.0, 100.0)).unwrap()
    };
    let mut model = Model::new_assembly();
    let (base, stand) = {
        let a = model.assembly_mut().unwrap();
        let d = a.define(Arc::new(part.clone()));
        let s = a.define(Arc::new(sub));
        (
            a.insert(d, Frame::WORLD).unwrap(),
            a.insert(s, at(60.0, 0.0, 0.0)).unwrap(),
        )
    };
    let m = mate(
        &mut model,
        MateKind::Coincident,
        end(base, face(&part, UP)),
        MateEnd {
            path: vec![stand, inner],
            geom: Some(face(&part, DOWN)),
        },
    );
    let mut engine = Engine::new();
    ok(&engine.regenerate(&mut model).clone(), m);
    // The plate inside is 100 up in the stand, so the stand goes 95 down.
    assert!((frame(&model, stand).origin.z + 95.0).abs() < 1e-9);
}

#[test]
fn fifty_components_are_solved_quickly() {
    // A stack of plates, each held on the one below by three face mates.
    let part = plate("Plate", 40.0, 30.0, 5.0, 0.0);
    let mut model = Model::new_assembly();
    let mut ids = Vec::new();
    {
        let a = model.assembly_mut().unwrap();
        let d = a.define(Arc::new(part.clone()));
        for i in 0..50 {
            ids.push(
                a.insert(d, at(3.0 * f64::from(i), 0.0, 7.0 * f64::from(i)))
                    .unwrap(),
            );
        }
        let geom = |n| face(&part, n);
        let faces = [
            (geom(UP), geom(DOWN), false),
            (geom(DVec3::X), geom(DVec3::X), true),
            (geom(DVec3::Y), geom(DVec3::Y), true),
        ];
        for pair in ids.windows(2) {
            for (below, above, flip) in &faces {
                let m = a.add_mate(
                    MateKind::Coincident,
                    end(pair[0], below.clone()),
                    end(pair[1], above.clone()),
                );
                a.mate_mut(m).unwrap().flip = *flip;
            }
        }
    }
    let mut engine = Engine::new();
    let built = engine.regenerate(&mut model).clone();
    assert_eq!(built.mate_failures().count(), 0);
    assert_eq!(built.freedom, 0);
    let last = frame(&model, ids[49]);
    assert!(
        last.origin.abs_diff_eq(DVec3::new(0.0, 0.0, 245.0), 1e-7),
        "{last:?}"
    );

    // A drag: the bottom plate is moved, and all the others follow.
    let mut worst: f64 = 0.0;
    for step in 1..=10 {
        let c = model.assembly_mut().unwrap().component_mut(ids[0]).unwrap();
        c.placement = at(2.0 * f64::from(step), 0.0, 0.0);
        let start = std::time::Instant::now();
        let built = engine.regenerate(&mut model);
        worst = worst.max(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(built.mate_failures().count(), 0);
    }
    let last = frame(&model, ids[49]);
    assert!(
        last.origin.abs_diff_eq(DVec3::new(20.0, 0.0, 245.0), 1e-7),
        "{last:?}"
    );
    println!("50 components, 147 mates: {worst:.2} ms per drag step at worst");
    // The budget is 4 ms in a release build; this guards against it getting far worse.
    let limit = if cfg!(debug_assertions) { 400.0 } else { 4.0 };
    assert!(worst < limit, "{worst} ms");

    // The whole stack let go, and pulled by a corner of its top plate with the mouse:
    // all fifty come along.
    model
        .assembly_mut()
        .unwrap()
        .component_mut(ids[0])
        .unwrap()
        .fixed = false;
    assert_eq!(engine.regenerate(&mut model).freedom, 6);
    let corner = DVec3::new(40.0, 30.0, 5.0);
    let from = frame(&model, ids[49]).to_world(corner);
    let mut worst: f64 = 0.0;
    for step in 1..=10 {
        let to = from + DVec3::new(3.0, -2.0, 1.0) * f64::from(step);
        let start = std::time::Instant::now();
        drag(&mut model, &mut engine, ids[49], corner, to);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        // The first step also sets the solver up for these mates; the rest reuse that.
        if step == 1 {
            println!("pulled by the mouse: {ms:.2} ms for the first step");
        } else {
            worst = worst.max(ms);
        }
        let reached = frame(&model, ids[49]).to_world(corner);
        assert!(
            (reached - to).length() < 1e-3,
            "step {step}: {reached} for {to}"
        );
        assert_eq!(engine.evaluation().mate_failures().count(), 0);
    }
    let bottom = frame(&model, ids[0]);
    assert!(
        bottom
            .origin
            .abs_diff_eq(DVec3::new(50.0, -20.0, 10.0), 1e-3),
        "{bottom:?}"
    );
    println!("pulled by the mouse: {worst:.2} ms per drag step at worst");
    assert!(worst < limit, "{worst} ms");
}

fn drag(model: &mut Model, engine: &mut Engine, component: CompId, point: DVec3, to: DVec3) {
    engine.set_drag(Some(peet_model::Drag {
        component,
        point,
        to,
    }));
    engine.regenerate(model);
}

#[test]
fn a_dragged_component_goes_as_far_as_its_mates_allow() {
    let (mut model, part, base, top) = two_plates();
    let mut engine = Engine::new();
    engine.regenerate(&mut model);
    let corner = DVec3::new(40.0, 30.0, 5.0);

    // Free: the point goes exactly there, and the component doesn't turn.
    let before = frame(&model, top);
    let to = DVec3::new(-10.0, 55.0, 80.0);
    drag(&mut model, &mut engine, top, corner, to);
    let f = frame(&model, top);
    assert!(f.to_world(corner).abs_diff_eq(to, 1e-12), "{f:?}");
    assert_eq!(f.rotation, before.rotation);
    // A drag is for one rebuild: the next one leaves everything where it is.
    let dragged = model.clone();
    engine.regenerate(&mut model);
    assert_eq!(model, dragged);
    // A fixed component is not moved.
    drag(&mut model, &mut engine, base, corner, to);
    assert_eq!(frame(&model, base), Frame::WORLD);

    // Lying on the first plate: it slides to under the cursor without leaving the
    // face, and without turning.
    mate(
        &mut model,
        MateKind::Coincident,
        end(base, face(&part, UP)),
        end(top, face(&part, DOWN)),
    );
    engine.regenerate(&mut model);
    let resting = frame(&model, top);
    let to = DVec3::new(120.0, -40.0, 60.0);
    drag(&mut model, &mut engine, top, corner, to);
    let f = frame(&model, top);
    let at = f.to_world(corner);
    assert!((at.z - 10.0).abs() < 1e-8, "it stays on the plate: {at}");
    // (To a hundredth of a millimetre: it is also being pulled off the face.)
    assert!((at.truncate() - to.truncate()).length() < 0.01, "{at}");
    assert!(
        f.rotation.angle_between(resting.rotation) < 1e-6,
        "it slides, not turns"
    );
    assert_eq!(engine.evaluation().mate_failures().count(), 0);
    assert_eq!(engine.evaluation().freedom, 3);

    // Held all round, it can't go anywhere.
    for (n, flip) in [(DVec3::X, true), (DVec3::Y, true)] {
        let m = mate(
            &mut model,
            MateKind::Coincident,
            end(base, face(&part, n)),
            end(top, face(&part, n)),
        );
        model.assembly_mut().unwrap().mate_mut(m).unwrap().flip = flip;
    }
    engine.regenerate(&mut model);
    let held = frame(&model, top);
    drag(&mut model, &mut engine, top, corner, to);
    let f = frame(&model, top);
    assert!(
        f.origin.abs_diff_eq(held.origin, 1e-8) && f.rotation.angle_between(held.rotation) < 1e-9
    );
}

#[test]
fn a_hinged_component_swings_round_to_follow_the_drag() {
    // A plate with a hole on a pin: it can only turn about the pin (and not slide along
    // it, held by a face mate).
    let holed = plate("Holed", 40.0, 30.0, 5.0, 4.0);
    let pin = pin(4.0, 20.0);
    let mut model = Model::new_assembly();
    let (post, arm) = {
        let a = model.assembly_mut().unwrap();
        let dp = a.define(Arc::new(pin.clone()));
        let dh = a.define(Arc::new(holed.clone()));
        (
            a.insert(dp, Frame::WORLD).unwrap(),
            a.insert(dh, at(-20.0, -15.0, 0.0)).unwrap(),
        )
    };
    let round = DVec3::ZERO;
    mate(
        &mut model,
        MateKind::Concentric,
        end(post, face(&pin, round)),
        end(arm, face(&holed, round)),
    );
    let flush = mate(
        &mut model,
        MateKind::Coincident,
        end(post, face(&pin, DOWN)),
        end(arm, face(&holed, DOWN)),
    );
    model.assembly_mut().unwrap().mate_mut(flush).unwrap().flip = true;
    let mut engine = Engine::new();
    assert_eq!(engine.regenerate(&mut model).freedom, 1);

    // The middle of the plate's short edge is 20 from the pin, along +X. Pulled to a
    // place a quarter turn round, in small steps as a mouse would.
    let grab = DVec3::new(40.0, 15.0, 0.0);
    assert!(
        frame(&model, arm)
            .to_world(grab)
            .abs_diff_eq(DVec3::new(20.0, 0.0, 0.0), 1e-8)
    );
    for step in 1..=18 {
        let angle = f64::from(step) * 5.0_f64.to_radians();
        let to = DVec3::new(20.0 * angle.cos(), 20.0 * angle.sin(), 0.0);
        drag(&mut model, &mut engine, arm, grab, to);
        let reached = frame(&model, arm).to_world(grab);
        assert!(
            (reached - to).length() < 0.05,
            "step {step}: {reached} for {to}"
        );
        assert_eq!(engine.evaluation().mate_failures().count(), 0);
    }
    let f = frame(&model, arm);
    assert!(
        f.vector_to_world(DVec3::X).abs_diff_eq(DVec3::Y, 1e-3),
        "a quarter turn: {f:?}"
    );
    // Still exactly on the pin and flush.
    let centre = f.to_world(DVec3::new(20.0, 15.0, 0.0));
    assert!(centre.length() < 1e-8, "{centre}");

    // Pulled to somewhere it can't reach, it points that way.
    let far = DVec3::new(-500.0, 0.0, 300.0);
    for _ in 0..12 {
        drag(&mut model, &mut engine, arm, grab, far);
    }
    let reached = frame(&model, arm).to_world(grab);
    assert!(
        reached.abs_diff_eq(DVec3::new(-20.0, 0.0, 0.0), 0.5),
        "{reached}"
    );
    // And what a drag leaves is a solved assembly: nothing moves on the next rebuild.
    let after = model.clone();
    engine.regenerate(&mut model);
    assert_eq!(model, after);
}

#[test]
fn what_a_dragged_component_is_mated_to_comes_along() {
    // Two free plates, the second on the first; a third, fixed, elsewhere.
    let (mut model, part, base, top) = two_plates();
    {
        let a = model.assembly_mut().unwrap();
        a.component_mut(base).unwrap().fixed = false;
        let d = a.component(base).unwrap().definition;
        let third = a.insert(d, at(500.0, 0.0, 0.0)).unwrap();
        a.component_mut(third).unwrap().fixed = true;
    }
    let whole = |c| MateEnd {
        path: vec![c],
        geom: None,
    };
    let mut engine = Engine::new();
    engine.regenerate(&mut model);
    let relative = frame(&model, base).inverse().compose(&frame(&model, top));
    mate(
        &mut model,
        MateKind::Fasten(relative),
        whole(base),
        whole(top),
    );
    engine.regenerate(&mut model);
    let to = DVec3::new(0.0, 200.0, 0.0);
    drag(&mut model, &mut engine, top, DVec3::ZERO, to);
    assert!(frame(&model, top).origin.abs_diff_eq(to, 1e-3));
    let now = frame(&model, base).inverse().compose(&frame(&model, top));
    assert!(now.origin.abs_diff_eq(relative.origin, 1e-8), "{now:?}");
    assert!(now.rotation.angle_between(relative.rotation) < 1e-9);
    let _ = part;
}

#[test]
fn each_component_says_how_it_can_still_move() {
    let (mut model, part, base, top) = two_plates();
    // A third plate, to hang on the second.
    let third = {
        let a = model.assembly_mut().unwrap();
        let d = a.component(base).unwrap().definition;
        a.insert(d, at(0.0, 0.0, 200.0)).unwrap()
    };
    let mut engine = Engine::new();
    let free = |engine: &Engine, id| engine.evaluation().component_freedom(id);
    engine.regenerate(&mut model);
    assert_eq!(
        [base, top, third].map(|c| free(&engine, c)),
        [Some(0), Some(6), Some(6)],
        "the fixed one can't move; the others are loose"
    );
    assert_eq!(engine.evaluation().freedom, 12);

    // The second on the first: it slides two ways and turns one.
    mate(
        &mut model,
        MateKind::Coincident,
        end(base, face(&part, UP)),
        end(top, face(&part, DOWN)),
    );
    engine.regenerate(&mut model);
    assert_eq!([top, third].map(|c| free(&engine, c)), [Some(3), Some(6)]);
    // And against a side: it only slides along the edge.
    let side = mate(
        &mut model,
        MateKind::Coincident,
        end(base, face(&part, DVec3::NEG_X)),
        end(top, face(&part, DVec3::NEG_X)),
    );
    model.assembly_mut().unwrap().mate_mut(side).unwrap().flip = true;
    engine.regenerate(&mut model);
    assert_eq!([top, third].map(|c| free(&engine, c)), [Some(1), Some(6)]);

    // The third fastened to the second: it can't move on its own, but it goes where the
    // second goes, so it has the second's one way to move. Together they have one.
    let whole = |c| MateEnd {
        path: vec![c],
        geom: None,
    };
    let relative = frame(&model, top).inverse().compose(&frame(&model, third));
    let held = mate(
        &mut model,
        MateKind::Fasten(relative),
        whole(top),
        whole(third),
    );
    engine.regenerate(&mut model);
    assert_eq!([top, third].map(|c| free(&engine, c)), [Some(1), Some(1)]);
    assert_eq!(engine.evaluation().freedom, 1);

    // The second held all round: nothing is loose any more.
    let end_stop = mate(
        &mut model,
        MateKind::Distance(Scalar::new(4.0)),
        end(base, face(&part, DVec3::NEG_Y)),
        end(top, face(&part, DVec3::Y)),
    );
    engine.regenerate(&mut model);
    assert_eq!(
        [base, top, third].map(|c| free(&engine, c)),
        [Some(0), Some(0), Some(0)]
    );
    assert_eq!(engine.evaluation().freedom, 0);
    // It stays known through a drag (which changes where things are, not what holds
    // them), and is worked out again when a mate goes.
    drag(
        &mut model,
        &mut engine,
        third,
        DVec3::ZERO,
        DVec3::new(9.0, 9.0, 9.0),
    );
    assert_eq!(free(&engine, third), Some(0));
    model.assembly_mut().unwrap().remove_mate(end_stop);
    model.assembly_mut().unwrap().remove_mate(held);
    engine.regenerate(&mut model);
    assert_eq!([top, third].map(|c| free(&engine, c)), [Some(1), Some(6)]);

    // With nothing fixed, everything can move, held together or not.
    model
        .assembly_mut()
        .unwrap()
        .component_mut(base)
        .unwrap()
        .fixed = false;
    engine.regenerate(&mut model);
    assert_eq!(
        [base, top, third].map(|c| free(&engine, c)),
        [Some(6), Some(6), Some(6)]
    );
    assert_eq!(
        engine.evaluation().freedom,
        13,
        "6 + 1 for the pair, 6 for the third"
    );

    // A suppressed component is not there to move.
    model
        .assembly_mut()
        .unwrap()
        .component_mut(third)
        .unwrap()
        .suppressed = true;
    engine.regenerate(&mut model);
    assert_eq!(free(&engine, third), None);
}

#[test]
fn a_hinge_leaves_one_way_to_move() {
    let holed = plate("Holed", 40.0, 30.0, 5.0, 4.0);
    let pin = pin(4.0, 20.0);
    let mut model = Model::new_assembly();
    let (post, arm) = {
        let a = model.assembly_mut().unwrap();
        let dp = a.define(Arc::new(pin.clone()));
        let dh = a.define(Arc::new(holed.clone()));
        (
            a.insert(dp, Frame::WORLD).unwrap(),
            a.insert(dh, at(-20.0, -15.0, 0.0)).unwrap(),
        )
    };
    mate(
        &mut model,
        MateKind::Concentric,
        end(post, face(&pin, DVec3::ZERO)),
        end(arm, face(&holed, DVec3::ZERO)),
    );
    let mut engine = Engine::new();
    engine.regenerate(&mut model);
    assert_eq!(engine.evaluation().component_freedom(arm), Some(2));
    let flush = mate(
        &mut model,
        MateKind::Coincident,
        end(post, face(&pin, DOWN)),
        end(arm, face(&holed, DOWN)),
    );
    model.assembly_mut().unwrap().mate_mut(flush).unwrap().flip = true;
    engine.regenerate(&mut model);
    assert_eq!(engine.evaluation().component_freedom(arm), Some(1));
    assert_eq!(engine.evaluation().component_freedom(post), Some(0));
    // A mate that can't hold is left out: the freedom is that of the ones that do.
    let clash = mate(
        &mut model,
        MateKind::Distance(Scalar::new(9.0)),
        end(post, face(&pin, DOWN)),
        end(arm, face(&holed, DOWN)),
    );
    let built = engine.regenerate(&mut model).clone();
    assert!(built.mate_status(clash).unwrap().is_failed());
    assert_eq!(built.component_freedom(arm), Some(1));
}
