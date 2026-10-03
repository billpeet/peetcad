//! The parametric core end to end: history, references that survive edits, incremental
//! rebuilds, failures that don't take the model down, reference geometry and undo.

mod common;

use std::f64::consts::PI;

use common::{FRONT, Part, TOP, assert_close, dimensioned_rectangle};
use peet_kernel::Surface;
use peet_math::{DVec2, DVec3};
use peet_model::naming::{find_edge, find_face, find_vertex};
use peet_model::{
    AxisDef, AxisRef, CoordSystemDef, DependencyGraph, EndCondition, FeatureId, FeatureKind,
    History, Model, Operation, Output, PlaneDef, PlaneRef, PointDef, PointRef, Scalar, Status,
    StdAxis,
};
use peet_sketch::shapes;

/// A 100 × 60 × 10 plate with a 40 × 20 × 4 pocket in the top and a hole through the
/// pocket floor.
fn plate() -> (Part, [FeatureId; 6]) {
    let mut p = Part::default();
    let base = p.sketch(TOP, |s| dimensioned_rectangle(s, 100.0, 60.0));
    let plate = p.extrude(base, Operation::Add, |e| e.depth = Scalar::new(10.0));
    p.rebuild();
    let top = p.on_face(DVec3::Z, DVec3::new(1.0, 1.0, 10.0));
    let pocket_sketch = p.sketch(top, |s| {
        shapes::rectangle(s, DVec2::new(30.0, 20.0), DVec2::new(70.0, 40.0));
    });
    let pocket = p.extrude(pocket_sketch, Operation::Cut, |e| {
        e.depth = Scalar::new(4.0);
    });
    p.rebuild();
    let floor = p.on_face(DVec3::Z, DVec3::new(50.0, 30.0, 6.0));
    let hole_sketch = p.sketch(floor, |s| {
        s.add_circle(DVec2::new(50.0, 30.0), 5.0);
    });
    let hole = p.extrude(hole_sketch, Operation::Cut, |e| {
        e.end = EndCondition::ThroughAll;
    });
    p.rebuild();
    (p, [base, plate, pocket_sketch, pocket, hole_sketch, hole])
}

fn plate_volume(width: f64, thickness: f64, pocket_depth: f64) -> f64 {
    width * 60.0 * thickness - 40.0 * 20.0 * pocket_depth - PI * 25.0 * (thickness - pocket_depth)
}

#[test]
fn sketch_extrude_sketch_on_face_cut() {
    let (p, _) = plate();
    p.assert_ok();
    assert_eq!(p.bodies().len(), 1);
    assert_close(p.volume(), plate_volume(100.0, 10.0, 4.0));
    let counts = peet_kernel::validate::validate(&p.bodies()[0].solid).unwrap();
    assert_eq!(counts.genus, 1, "the through hole makes a handle");
    assert_eq!(counts.faces, 12);
}

#[test]
fn faces_are_named_after_what_made_them() {
    let (p, [_, plate, _, pocket, _, hole]) = plate();
    let body = &p.bodies()[0];
    let describe = |n: DVec3, at: DVec3| {
        let (_, f) = p.face(n, at);
        p.model.describe_face(body.face_name(f))
    };
    assert_eq!(
        describe(DVec3::Z, DVec3::new(50.0, 30.0, 6.0)),
        "the end face of Cut-Extrude1"
    );
    assert_eq!(
        describe(DVec3::Z, DVec3::new(1.0, 1.0, 10.0)),
        "the end face of Extrude1"
    );
    assert_eq!(
        describe(-DVec3::Z, DVec3::ZERO),
        "the start face of Extrude1"
    );
    // Every face has one origin, from one of the three solid features, and only the
    // hole's wall is round.
    for f in body.solid.face_ids() {
        let name = body.face_name(f);
        assert_eq!(name.origins().len(), 1);
        let feature = name.origins()[0].feature;
        assert!([plate, pocket, hole].contains(&feature));
        let round = matches!(body.solid.face(f).surface, Surface::Cylinder(_));
        assert_eq!(round, feature == hole);
    }
}

#[test]
fn a_dimension_change_in_the_first_sketch_carries_through() {
    let (mut p, [base, plate, ..]) = plate();
    p.set_dimension(base, "d1", "140");
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), plate_volume(140.0, 10.0, 4.0));

    // Thicker: the sketches on the top face and the pocket floor move up with their faces,
    // so the pocket is still 4 deep and the hole still starts at its floor.
    p.extrude_mut(plate).params.depth = Scalar::new(16.0);
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), plate_volume(140.0, 16.0, 4.0));
    let (_, floor) = p.face(DVec3::Z, DVec3::new(50.0, 30.0, 12.0));
    assert_eq!(
        p.model.describe_face(p.bodies()[0].face_name(floor)),
        "the end face of Cut-Extrude1"
    );
}

#[test]
fn references_survive_a_feature_inserted_upstream() {
    let (mut p, [_, _, pocket_sketch, ..]) = plate();
    let before = p.bodies()[0].face_names.clone();
    // Roll back to before the pocket and drill two holes through the plate there.
    p.model.set_rollback(p.model.index_of(pocket_sketch));
    p.rebuild();
    let top = p.on_face(DVec3::Z, DVec3::new(1.0, 1.0, 10.0));
    let holes = p.sketch(top, |s| {
        s.add_circle(DVec2::new(12.0, 12.0), 4.0);
        s.add_circle(DVec2::new(88.0, 48.0), 4.0);
    });
    p.extrude(holes, Operation::Cut, |e| e.end = EndCondition::ThroughAll);
    assert_eq!(
        p.model.index_of(holes),
        Some(2),
        "added at the rollback bar"
    );
    p.model.set_rollback(None);
    p.rebuild();
    p.assert_ok();
    assert_close(
        p.volume(),
        plate_volume(100.0, 10.0, 4.0) - 2.0 * PI * 16.0 * 10.0,
    );
    // The faces are numbered differently now, which indices can't survive and names can.
    assert_ne!(before, p.bodies()[0].face_names);
    assert_eq!(p.model.len(), 8);
}

#[test]
fn rebuilds_are_incremental() {
    let (mut p, [base, plate, pocket_sketch, pocket, hole_sketch, hole]) = plate();
    let stats = p.rebuild().stats;
    assert_eq!((stats.rebuilt, stats.reused), (0, 6), "nothing changed");

    // Changing the last feature recomputes only it.
    p.extrude_mut(hole).params.reverse = true;
    p.rebuild();
    let rebuilt = |p: &Part, id| p.eval().state(id).unwrap().rebuilt;
    assert!(rebuilt(&p, hole));
    for id in [base, plate, pocket_sketch, pocket, hole_sketch] {
        assert!(!rebuilt(&p, id), "{}", p.model.name_of(id));
    }
    assert!(p.status(hole).is_failed(), "reversed, it cuts nothing");
    p.extrude_mut(hole).params.reverse = false;
    assert_eq!(p.rebuild().stats.rebuilt, 1);
    p.assert_ok();

    // A deeper pocket: the pocket and everything built on it are recomputed. The hole's
    // sketch is solved again because its face moved.
    p.extrude_mut(pocket).params.depth = Scalar::new(5.0);
    p.rebuild();
    for id in [base, plate, pocket_sketch] {
        assert!(!rebuilt(&p, id), "{}", p.model.name_of(id));
    }
    assert!(rebuilt(&p, pocket) && rebuilt(&p, hole_sketch) && rebuilt(&p, hole));
    assert_close(p.volume(), plate_volume(100.0, 10.0, 5.0));

    // A wider plate rebuilds the solids, but sketches on faces that didn't move come out
    // the same and are reused.
    p.set_dimension(base, "d1", "120");
    p.rebuild();
    assert!(rebuilt(&p, base) && rebuilt(&p, plate) && rebuilt(&p, pocket) && rebuilt(&p, hole));
    assert!(!rebuilt(&p, pocket_sketch) && !rebuilt(&p, hole_sketch));
    p.assert_ok();

    // Bodies a feature doesn't touch are shared, not copied.
    let stamp = p.bodies()[0].stamp;
    let far = p.sketch(TOP, |s| {
        shapes::rectangle(s, DVec2::new(500.0, 0.0), DVec2::new(520.0, 20.0));
    });
    p.extrude(far, Operation::NewBody, |_| {});
    p.rebuild();
    assert_eq!(p.bodies().len(), 2);
    assert_eq!(p.bodies()[0].stamp, stamp);
}

#[test]
fn parameters_drive_sketches_and_features() {
    let mut p = Part::default();
    p.model.parameters.set("width", "80").unwrap();
    p.model.parameters.set("thickness", "width / 10").unwrap();
    let base = p.sketch(TOP, |s| dimensioned_rectangle(s, 100.0, 50.0));
    p.set_dimension(base, "d1", "width");
    let params = p.model.parameters.clone();
    let plate = p.extrude(base, Operation::Add, |e| {
        e.depth
            .set_input("thickness + 2", peet_model::ScalarKind::Length, &params)
            .unwrap();
    });
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), 80.0 * 50.0 * 10.0);

    p.model.parameters.set("width", "120").unwrap();
    p.rebuild();
    assert_close(p.volume(), 120.0 * 50.0 * 14.0);

    // A parameter that goes missing is reported by what needs it. The sketch keeps its
    // last value with a warning; the depth can't be evaluated at all.
    p.model.parameters.remove("width");
    p.rebuild();
    assert!(matches!(p.status(base), Status::Warning(m) if m.contains("d1")));
    let message = p.status(plate).message().expect("the depth fails");
    assert!(message.contains("Depth"), "{message}");
}

#[test]
fn a_failing_feature_is_flagged_and_the_rest_still_builds() {
    let (mut p, [_, _, pocket_sketch, pocket, hole_sketch, hole]) = plate();
    // An open profile can't be cut: the pocket fails, the plate stays, and the features on
    // the pocket floor say what they lost.
    let line = p
        .sketch_mut(pocket_sketch)
        .entities()
        .find(|(_, e)| e.kind() == peet_sketch::EntityKind::Line)
        .unwrap()
        .0;
    p.sketch_mut(pocket_sketch).remove_entity(line).unwrap();
    p.rebuild();
    let m = p.status(pocket).message().unwrap().to_owned();
    assert!(m.contains("closed"), "{m}");
    assert_eq!(p.bodies().len(), 1);
    let lost = p.status(hole_sketch).message().unwrap().to_owned();
    assert!(
        lost.contains("no longer exists") && lost.contains("Cut-Extrude1"),
        "{lost}"
    );
    let m = p.status(hole).message().unwrap().to_owned();
    assert!(m.contains("Sketch3") && m.contains("failed"), "{m}");
    assert_close(p.volume(), 100.0 * 60.0 * 10.0);

    // Deleting a sketch: the feature using it explains itself.
    let (mut p, [base, plate, ..]) = plate();
    p.model.remove(base);
    p.rebuild();
    let m = p.status(plate).message().unwrap();
    assert!(m.contains("sketch was deleted"), "{m}");
    assert!(p.bodies().is_empty());
}

#[test]
fn suppress_and_roll_back() {
    let (mut p, [_, _, pocket_sketch, pocket, hole_sketch, hole]) = plate();
    p.model.feature_mut(hole).unwrap().suppressed = true;
    p.rebuild();
    assert_eq!(*p.status(hole), Status::Suppressed);
    assert_close(p.volume(), 100.0 * 60.0 * 10.0 - 40.0 * 20.0 * 4.0);
    p.model.feature_mut(hole).unwrap().suppressed = false;
    p.rebuild();
    assert_close(p.volume(), plate_volume(100.0, 10.0, 4.0));

    // Suppressing the pocket suppresses what is built on its floor with it.
    p.model.feature_mut(pocket).unwrap().suppressed = true;
    p.rebuild();
    assert_eq!(*p.status(hole_sketch), Status::SuppressedBy(pocket));
    assert_eq!(*p.status(hole), Status::SuppressedBy(hole_sketch));
    assert_close(p.volume(), 100.0 * 60.0 * 10.0);
    p.model.feature_mut(pocket).unwrap().suppressed = false;
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), plate_volume(100.0, 10.0, 4.0));

    // The rollback bar: only the features above it are built.
    p.model.set_rollback(p.model.index_of(pocket_sketch));
    let stats = p.rebuild().stats;
    assert!(p.model.is_rolled_back());
    assert_eq!(*p.status(pocket), Status::RolledBack);
    assert_close(p.volume(), 100.0 * 60.0 * 10.0);
    assert_eq!(stats.rebuilt, 0, "rolling back reuses what was built");
    p.model.set_rollback(None);
    let stats = p.rebuild().stats;
    assert_eq!(stats.rebuilt, 0, "and rolling forward does too");
    p.assert_ok();
    assert_close(p.volume(), plate_volume(100.0, 10.0, 4.0));
}

#[test]
fn the_dependency_graph_guards_reordering() {
    let (mut p, [base, plate, pocket_sketch, pocket, hole_sketch, hole]) = plate();
    let graph = DependencyGraph::new(&p.model);
    assert_eq!(graph.dependencies(plate), [base]);
    assert_eq!(graph.dependencies(pocket_sketch), [plate]);
    assert_eq!(graph.dependencies(hole_sketch), [pocket]);
    assert_eq!(graph.dependents(base), [plate]);
    assert_eq!(
        graph.downstream(base),
        [plate, pocket_sketch, pocket, hole_sketch, hole]
    );
    assert_eq!(graph.downstream(pocket_sketch), [pocket, hole_sketch, hole]);
    assert!(graph.downstream(hole).is_empty());
    assert!(graph.broken().is_empty());

    // An unrelated sketch can go anywhere; its extrusion must stay below it.
    let extra = p.sketch(FRONT, |s| {
        shapes::rectangle(s, DVec2::new(200.0, 0.0), DVec2::new(220.0, 20.0));
    });
    let boss = p.extrude(extra, Operation::NewBody, |_| {});
    p.model.move_to(extra, 0).unwrap();
    assert_eq!(p.model.index_of(extra), Some(0));
    p.model.move_to(boss, 1).unwrap();
    assert_eq!(p.model.index_of(boss), Some(1));
    let e = p.model.move_to(boss, 0).unwrap_err();
    assert!(
        e.contains("Sketch4") && e.contains("must come first"),
        "{e}"
    );
    let e = p.model.move_to(extra, 5).unwrap_err();
    assert!(e.contains("Extrude2"), "{e}");
    // The pocket can't go above the plate it is sketched on, nor below the hole on its
    // floor.
    let e = p.model.move_to(pocket_sketch, 0).unwrap_err();
    assert!(e.contains("Extrude1"), "{e}");
    let e = p.model.move_to(pocket, 7).unwrap_err();
    assert!(e.contains("Sketch3"), "{e}");
    p.rebuild();
    p.assert_ok();
    assert_eq!(p.bodies().len(), 2);
    assert_close(
        p.volume(),
        plate_volume(100.0, 10.0, 4.0) + 20.0 * 20.0 * 10.0,
    );
}

#[test]
fn split_and_merged_faces_resolve_to_the_right_one() {
    let mut p = Part::default();
    let base = p.sketch(TOP, |s| dimensioned_rectangle(s, 100.0, 60.0));
    p.extrude(base, Operation::Add, |e| e.depth = Scalar::new(10.0));
    p.rebuild();
    // A slot right across the top splits it into two faces with the same name.
    let top = p.on_face(DVec3::Z, DVec3::new(1.0, 1.0, 10.0));
    let slot = p.sketch(top, |s| {
        shapes::rectangle(s, DVec2::new(40.0, -10.0), DVec2::new(60.0, 70.0));
    });
    p.extrude(slot, Operation::Cut, |e| e.depth = Scalar::new(3.0));
    p.rebuild();
    p.assert_ok();
    let body = &p.bodies()[0];
    let halves: Vec<_> = body
        .solid
        .face_ids()
        .filter(|f| p.model.describe_face(body.face_name(*f)) == "the end face of Extrude1")
        .collect();
    assert_eq!(halves.len(), 2);
    let left = halves
        .iter()
        .copied()
        .find(|f| body.face_center(*f).x < 50.0)
        .unwrap();
    let right = halves.iter().copied().find(|f| *f != left).unwrap();
    let (left_ref, right_ref) = (body.face_ref(left), body.face_ref(right));
    assert_eq!(left_ref.name, right_ref.name);
    // The right half's edge along the plate's far end, and its corner at the back.
    let end_edge = body
        .solid
        .edge_ids()
        .find(|e| {
            let m = body.solid.edge(*e).point_at_fraction(0.5);
            m.distance(DVec3::new(100.0, 30.0, 10.0)) < 1e-9
        })
        .unwrap();
    let corner = body
        .solid
        .vertex_ids()
        .find(|v| {
            body.solid
                .vertex(*v)
                .point
                .distance(DVec3::new(100.0, 60.0, 10.0))
                < 1e-9
        })
        .unwrap();
    let edge_ref = body.edge_ref(end_edge).unwrap();
    let vertex_ref = body.vertex_ref(corner);

    // Stretch the plate: the faces move and are renumbered, and the references still
    // tell the halves apart.
    p.set_dimension(base, "d1", "180");
    p.rebuild();
    p.assert_ok();
    let bodies = p.bodies();
    let l = find_face(bodies, &left_ref).unwrap();
    let r = find_face(bodies, &right_ref).unwrap();
    assert_ne!(l.id, r.id);
    assert!(bodies[0].face_center(l.id).x < 40.0);
    assert!(bodies[0].face_center(r.id).x > 60.0);
    let e = find_edge(bodies, &edge_ref).unwrap();
    let mid = bodies[0].solid.edge(e.id).point_at_fraction(0.5);
    assert!(mid.distance(DVec3::new(180.0, 30.0, 10.0)) < 1e-9, "{mid}");
    let v = find_vertex(bodies, &vertex_ref).unwrap();
    let at = bodies[0].solid.vertex(v.id).point;
    assert!(at.distance(DVec3::new(180.0, 60.0, 10.0)) < 1e-9, "{at}");

    // Two bosses joined flush: their shared top face carries both origins, and a
    // reference made to one boss's top before the other existed still finds it.
    let mut p = Part::default();
    let a = p.sketch(TOP, |s| {
        shapes::rectangle(s, DVec2::ZERO, DVec2::new(50.0, 40.0));
    });
    p.extrude(a, Operation::Add, |_| {});
    p.rebuild();
    let top_of_a = p.face_ref(DVec3::Z, DVec3::new(1.0, 1.0, 10.0));
    let b = p.sketch(TOP, |s| {
        shapes::rectangle(s, DVec2::new(50.0, 0.0), DVec2::new(90.0, 40.0));
    });
    p.extrude(b, Operation::Add, |_| {});
    p.rebuild();
    p.assert_ok();
    assert_eq!(p.bodies().len(), 1);
    assert_eq!(p.bodies()[0].solid.faces.len(), 6);
    let found = find_face(p.bodies(), &top_of_a).expect("the merged face shares an origin");
    let name = p.bodies()[0].face_name(found.id);
    assert_eq!(name.origins().len(), 2);
    assert_eq!(
        p.model.describe_face(name),
        "the end face of Extrude1 joined with the end face of Extrude2"
    );
}

#[test]
fn reference_geometry_follows_the_model() {
    let (mut p, [base, plate, ..]) = plate();
    let hole_wall = {
        let body = &p.bodies()[0];
        let f = body
            .solid
            .face_ids()
            .find(|f| matches!(body.solid.face(*f).surface, Surface::Cylinder(_)))
            .unwrap();
        body.face_ref(f)
    };
    let corner = {
        let body = &p.bodies()[0];
        let v = body
            .solid
            .vertex_ids()
            .find(|v| {
                body.solid
                    .vertex(*v)
                    .point
                    .distance(DVec3::new(100.0, 60.0, 10.0))
                    < 1e-9
            })
            .unwrap();
        body.vertex_ref(v)
    };
    let top = p.on_face(DVec3::Z, DVec3::new(1.0, 1.0, 10.0));
    let offset = p.model.add(FeatureKind::Plane(PlaneDef::Offset {
        from: top.clone(),
        distance: Scalar::new(25.0),
        flip: false,
    }));
    let mid = p.model.add(FeatureKind::Plane(PlaneDef::Midplane {
        a: top.clone(),
        b: p.on_face(-DVec3::Z, DVec3::ZERO),
    }));
    let tilted = p.model.add(FeatureKind::Plane(PlaneDef::Angled {
        from: TOP,
        about: AxisRef::Standard(StdAxis::X),
        angle: Scalar::new(90.0),
    }));
    let axis = p.model.add(FeatureKind::Axis(AxisDef::Cylinder(hole_wall)));
    let crossing = p.model.add(FeatureKind::Axis(AxisDef::TwoPlanes(
        PlaneRef::Feature(offset),
        FRONT,
    )));
    let point = p.model.add(FeatureKind::Point(PointDef::Vertex(corner)));
    let fixed = p.model.add(FeatureKind::Point(PointDef::Coordinates {
        x: Scalar::new(1.0),
        y: Scalar::new(2.0),
        z: Scalar::new(3.0),
    }));
    let csys = p.model.add(FeatureKind::CoordSystem(CoordSystemDef {
        origin: PointRef::Feature(point),
        orientation: PlaneRef::Feature(offset),
    }));
    p.rebuild();
    p.assert_ok();
    let plane_of = |p: &Part, id| match p.eval().output(id) {
        Output::Plane(pl) => pl,
        other => panic!("{other:?}"),
    };
    assert_close(plane_of(&p, offset).origin().z, 35.0);
    assert_close(plane_of(&p, mid).origin().z, 5.0);
    assert!(plane_of(&p, tilted).normal().abs_diff_eq(-DVec3::Y, 1e-12));
    let Output::Axis(a) = p.eval().output(axis) else {
        panic!()
    };
    assert!(a.dir.cross(DVec3::Z).length() < 1e-12);
    assert!((a.origin.truncate() - DVec2::new(50.0, 30.0)).length() < 1e-9);
    let Output::Axis(c) = p.eval().output(crossing) else {
        panic!()
    };
    assert!(c.dir.cross(DVec3::X).length() < 1e-12);
    assert!((c.origin.z - 35.0).abs() < 1e-9 && c.origin.y.abs() < 1e-9);
    assert_eq!(
        p.eval().output(point),
        Output::Point(DVec3::new(100.0, 60.0, 10.0))
    );
    assert_eq!(
        p.eval().output(fixed),
        Output::Point(DVec3::new(1.0, 2.0, 3.0))
    );

    // A boss sketched on the offset plane, extruded down to the plate's top face.
    let boss = p.sketch(PlaneRef::Feature(offset), |s| {
        s.add_circle(DVec2::new(15.0, 15.0), 6.0);
    });
    let post = p.extrude(boss, Operation::Add, |e| {
        e.end = EndCondition::UpTo(top);
    });
    p.rebuild();
    p.assert_ok();
    assert_close(
        p.volume(),
        plate_volume(100.0, 10.0, 4.0) + PI * 36.0 * 25.0,
    );

    // Wider and thicker: everything follows.
    p.set_dimension(base, "d1", "130");
    p.extrude_mut(plate).params.depth = Scalar::new(12.0);
    p.rebuild();
    p.assert_ok();
    assert_close(plane_of(&p, offset).origin().z, 37.0);
    assert_close(plane_of(&p, mid).origin().z, 6.0);
    assert_eq!(
        p.eval().output(point),
        Output::Point(DVec3::new(130.0, 60.0, 12.0))
    );
    let Output::Frame(f) = p.eval().output(csys) else {
        panic!()
    };
    assert_eq!(f.origin, DVec3::new(130.0, 60.0, 12.0));
    assert!(f.z_axis().abs_diff_eq(DVec3::Z, 1e-12));
    assert_close(
        p.volume(),
        plate_volume(130.0, 12.0, 4.0) + PI * 36.0 * 25.0,
    );
    assert!(
        DependencyGraph::new(&p.model)
            .dependencies(post)
            .contains(&plate)
    );

    // Suppressing the plane suppresses what stands on it, naming what it went with.
    p.model.feature_mut(offset).unwrap().suppressed = true;
    p.rebuild();
    assert_eq!(*p.status(boss), Status::SuppressedBy(offset));
    assert!(p.status(post).is_suppressed() && p.status(csys).is_suppressed());
    assert!(p.eval().failures().next().is_none());
    assert_close(p.volume(), plate_volume(130.0, 12.0, 4.0));
}

#[test]
fn undo_and_redo_restore_the_model() {
    let (mut p, [base, _, _, pocket, ..]) = plate();
    let mut history: History<Model> = History::default();
    let original = p.model.clone();

    history.record("Edit Sketch1", p.model.clone());
    p.set_dimension(base, "d1", "150");
    p.rebuild();
    history.record("Delete Cut-Extrude1", p.model.clone());
    p.model.remove(pocket);
    p.rebuild();
    assert!(p.eval().failures().count() > 0);

    assert_eq!(
        history.undo(&mut p.model).as_deref(),
        Some("Delete Cut-Extrude1")
    );
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), plate_volume(150.0, 10.0, 4.0));
    assert_eq!(history.undo(&mut p.model).as_deref(), Some("Edit Sketch1"));
    assert_eq!(p.model, original);
    p.rebuild();
    assert_close(p.volume(), plate_volume(100.0, 10.0, 4.0));

    assert_eq!(history.redo(&mut p.model).as_deref(), Some("Edit Sketch1"));
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), plate_volume(150.0, 10.0, 4.0));
}

#[test]
fn the_model_survives_serialization() {
    let (mut p, _) = plate();
    p.model.parameters.set("gap", "2.5").unwrap();
    let bytes = postcard::to_allocvec(&p.model).unwrap();
    let mut loaded: Model = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(loaded, p.model);
    loaded.validate().unwrap();
    // A fresh engine rebuilds the loaded model to the same bodies, names and stamps.
    let mut engine = peet_model::Engine::new();
    let eval = engine.regenerate(&mut loaded);
    assert_eq!(eval.bodies.len(), 1);
    assert_eq!(eval.bodies[0].solid, p.bodies()[0].solid);
    assert_eq!(eval.bodies[0].face_names, p.bodies()[0].face_names);
    assert_eq!(eval.bodies[0].stamp, p.bodies()[0].stamp);
    // New features get fresh ids and names.
    let id = loaded.add_sketch(TOP, peet_math::Plane::TOP);
    assert_eq!(loaded.name_of(id), "Sketch4");
    assert!(p.model.feature(id).is_none());
}

#[test]
fn values_follow_the_document_units() {
    use peet_sketch::expr::{LengthUnit, Units};
    let mut p = Part::default();
    p.model.parameters.set_units(Units::new(LengthUnit::Inch));
    let base = p.sketch(TOP, |s| dimensioned_rectangle(s, 100.0, 50.0));
    // A plain number is in inches; a number with a unit keeps its unit.
    p.set_dimension(base, "d1", "4");
    p.set_dimension(base, "d2", "50mm");
    let params = p.model.parameters.clone();
    let plate = p.extrude(base, Operation::Add, |e| {
        e.depth
            .set_input("0.5", peet_model::ScalarKind::Length, &params)
            .unwrap();
    });
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), 101.6 * 50.0 * 12.7);
    // Constant input is stored as a value, so switching units later keeps the geometry.
    let depth = &p.extrude_mut(plate).params.depth;
    assert_eq!(depth.expression, None);
    assert_close(depth.value, 12.7);
    p.model.parameters.set_units(Units::new(LengthUnit::Mm));
    p.rebuild();
    assert_close(p.volume(), 101.6 * 50.0 * 12.7);
    // Mixing kinds is an error that says so.
    let mut s = Scalar::new(1.0);
    let e = s
        .set_input(
            "10mm + 30deg",
            peet_model::ScalarKind::Length,
            &p.model.parameters,
        )
        .unwrap_err();
    assert!(e.contains("angle"), "{e}");
}
