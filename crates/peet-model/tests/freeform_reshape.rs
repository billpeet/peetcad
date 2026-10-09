//! Shells and draft on bodies with freeform faces, through the rebuild engine: a loft
//! hollowed out, a loft's sides drafted, and both following edits upstream.

mod common;

use std::f64::consts::PI;

use common::{Part, TOP, assert_close, dimensioned_rectangle};
use peet_kernel::Surface;
use peet_math::{DVec2, DVec3};
use peet_model::{
    FaceRef, FaceRole, FeatureId, FeatureKind, Operation, PlaneDef, PlaneRef, Scalar, Status,
    StdPlane,
};

fn v3(x: f64, y: f64, z: f64) -> DVec3 {
    DVec3::new(x, y, z)
}

/// A reference plane `distance` above the top plane.
fn plane_above(p: &mut Part, distance: f64) -> FeatureId {
    p.model.add(FeatureKind::Plane(PlaneDef::Offset {
        from: PlaneRef::Standard(StdPlane::Top),
        distance: Scalar::new(distance),
        flip: false,
    }))
}

fn failure(p: &Part, id: FeatureId) -> String {
    match p.status(id) {
        Status::Failed(m) => m.clone(),
        other => panic!("expected a failure, got {other:?}"),
    }
}

/// The freeform faces of the only body.
fn freeform_faces(p: &Part) -> Vec<FaceRef> {
    let body = &p.bodies()[0];
    body.solid
        .face_ids()
        .filter(|f| matches!(body.solid.face(*f).surface, Surface::Nurbs(_)))
        .map(|f| body.face_ref(f))
        .collect()
}

#[test]
fn shell_a_loft_open_at_the_top() {
    // A rectangle (dimensioned: `d1` wide, `d2` deep) lofted up to a circle.
    let mut p = Part::default();
    let base = p.sketch(TOP, |s| dimensioned_rectangle(s, 24.0, 18.0));
    let above = plane_above(&mut p, 25.0);
    let top = p.sketch(PlaneRef::Feature(above), |s| {
        s.add_circle(DVec2::new(12.0, 9.0), 6.0);
    });
    p.model.add_loft(vec![base, top], Operation::NewBody);
    p.rebuild();
    p.assert_ok();
    let whole = p.volume();
    assert_eq!(freeform_faces(&p).len(), 4);

    // Hollowed out, open at the round end, walls 1 thick.
    let opening = p.face_ref(DVec3::Z, v3(12.0, 9.0, 25.0));
    let shell = p.model.add_shell(vec![opening.clone()]);
    let FeatureKind::Shell(s) = &mut p.model.feature_mut(shell).unwrap().kind else {
        unreachable!()
    };
    s.thickness = Scalar::new(1.0);
    p.rebuild();
    p.assert_ok();
    let shelled = p.volume();
    assert!(shelled > 0.1 * whole && shelled < 0.5 * whole, "{shelled}");
    // Four walls and a floor inside, each named after the face it stands behind; the
    // faces outside keep their names, the opened one as the rim.
    let body = &p.bodies()[0];
    assert_eq!(body.solid.faces.len(), 11);
    let inner: Vec<_> = body
        .face_names
        .iter()
        .filter(|n| n.origins().iter().any(|o| o.role == FaceRole::Inner))
        .collect();
    assert_eq!(inner.len(), 5);
    assert!(
        inner
            .iter()
            .all(|n| n.origins().len() == 2 && n.origins().iter().any(|o| o.feature == shell))
    );
    let rim = body
        .face_names
        .iter()
        .position(|n| *n == opening.name)
        .expect("the opened face is still there, as the rim");
    assert_eq!(body.solid.faces[rim].loops.len(), 2);

    // The base gets wider: the loft and its shell follow.
    p.set_dimension(base, "d1", "30");
    p.rebuild();
    p.assert_ok();
    assert!(p.volume() > shelled && p.volume() < 1.3 * shelled);
    assert_eq!(p.bodies()[0].solid.faces.len(), 11);

    // Walls thicker than the round end's radius would fold over there.
    let FeatureKind::Shell(s) = &mut p.model.feature_mut(shell).unwrap().kind else {
        unreachable!()
    };
    s.thickness = Scalar::new(8.0);
    p.rebuild();
    let m = failure(&p, shell);
    assert!(m.contains("tightest curve of a freeform face"), "{m}");
    assert!(
        m.starts_with("The wall is thicker") && m.ends_with('.'),
        "{m}"
    );
}

#[test]
fn draft_the_sides_of_a_loft() {
    // A stadium lofted straight up to the same stadium: flat walls, and round ones
    // that are freeform faces, like the walls of an extruded spline.
    let stadium = |s: &mut peet_sketch::Sketch| {
        peet_sketch::shapes::slot(s, DVec2::new(-10.0, 0.0), DVec2::new(10.0, 0.0), 6.0);
    };
    let mut p = Part::default();
    let base = p.sketch(TOP, stadium);
    let above = plane_above(&mut p, 20.0);
    let top = p.sketch(PlaneRef::Feature(above), stadium);
    p.model.add_loft(vec![base, top], Operation::NewBody);
    p.rebuild();
    p.assert_ok();
    // The section with its radius less by `inset`, and the volume up to a height
    // (Simpson's rule is exact: the section's area is a quadratic in the height).
    let section = |inset: f64| 40.0 * (6.0 - inset) + PI * (6.0 - inset) * (6.0 - inset);
    let volume = |height: f64, tan: f64| {
        height / 6.0 * (section(0.0) + 4.0 * section(0.5 * height * tan) + section(height * tan))
    };
    assert_close(p.volume(), volume(20.0, 0.0));
    let names = p.bodies()[0].face_names.clone();

    // All four walls: the two flat ones, and the two freeform ones.
    let mut walls = freeform_faces(&p);
    assert_eq!(walls.len(), 2);
    walls.push(p.face_ref(DVec3::Y, v3(0.0, 6.0, 0.0)));
    walls.push(p.face_ref(-DVec3::Y, v3(0.0, -6.0, 0.0)));
    let draft = p
        .model
        .add_draft(walls, Some(PlaneRef::Standard(StdPlane::Top)));
    let FeatureKind::Draft(d) = &mut p.model.feature_mut(draft).unwrap().kind else {
        unreachable!()
    };
    d.angle = Scalar::new(4.0);
    p.rebuild();
    p.assert_ok();
    let tan = 4f64.to_radians().tan();
    assert_close(p.volume(), volume(20.0, tan));
    // The same faces, tilted: they keep their names.
    assert_eq!(p.bodies()[0].face_names, names);
    assert_eq!(freeform_faces(&p).len(), 2);

    // The loft gets taller: the draft follows.
    let FeatureKind::Plane(PlaneDef::Offset { distance, .. }) =
        &mut p.model.feature_mut(above).unwrap().kind
    else {
        unreachable!()
    };
    *distance = Scalar::new(30.0);
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), volume(30.0, tan));

    // The other way: wider at the top.
    let FeatureKind::Draft(d) = &mut p.model.feature_mut(draft).unwrap().kind else {
        unreachable!()
    };
    d.flip = true;
    p.rebuild();
    p.assert_ok();
    assert_close(p.volume(), volume(30.0, -tan));
}
