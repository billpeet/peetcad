//! Fillets and chamfers on freeform edges and faces, through the rebuild engine: they
//! keep their names through upstream edits, and what the kernel refuses reaches the
//! feature tree as a sentence that says what to do.

mod common;

use common::Part;
use peet_kernel::validate::measure;
use peet_kernel::{Curve3, Surface};
use peet_math::{DVec2, DVec3};
use peet_model::{
    BlendKind, EdgeRef, FaceRole, FeatureId, FeatureKind, Operation, PlaneDef, PlaneRef, Scalar,
    Status, StdPlane,
};
use peet_sketch::ConstraintKind;

fn v2(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

fn plane_at(p: &mut Part, distance: f64) -> FeatureId {
    p.model.add(FeatureKind::Plane(PlaneDef::Offset {
        from: PlaneRef::Standard(StdPlane::Top),
        distance: Scalar::new(distance),
        flip: false,
    }))
}

/// A circle about the origin on the plane `distance` above the top plane; `d1` is its
/// radius.
fn circle_at(p: &mut Part, distance: f64, radius: f64) -> FeatureId {
    let plane = plane_at(p, distance);
    p.sketch(PlaneRef::Feature(plane), |s| {
        let c = s.add_circle(DVec2::ZERO, radius);
        s.add_dimension(ConstraintKind::Radius(c), radius).unwrap();
    })
}

fn rectangle_at(p: &mut Part, distance: f64, w: f64, h: f64) -> FeatureId {
    let plane = plane_at(p, distance);
    p.sketch(PlaneRef::Feature(plane), |s| {
        peet_sketch::shapes::rectangle(s, v2(-w / 2.0, -h / 2.0), v2(w / 2.0, h / 2.0));
    })
}

/// The first edge of body 0 whose middle satisfies `pick`.
#[track_caller]
fn edge_where(p: &Part, pick: impl Fn(&Curve3, DVec3) -> bool) -> EdgeRef {
    let body = &p.bodies()[0];
    let found = body
        .solid
        .edge_ids()
        .find(|&e| {
            let edge = body.solid.edge(e);
            pick(&edge.curve, edge.point_at_fraction(0.5))
        })
        .expect("an edge like that");
    body.edge_ref(found).unwrap()
}

fn blend_mut(p: &mut Part, id: FeatureId) -> &mut peet_model::BlendFeature {
    match &mut p.model.feature_mut(id).unwrap().kind {
        FeatureKind::Blend(b) => b,
        _ => panic!("not a blend"),
    }
}

#[track_caller]
fn failure(p: &Part, id: FeatureId) -> String {
    match p.status(id) {
        Status::Failed(m) => m.clone(),
        other => panic!("expected a failure, got {other:?}"),
    }
}

/// How many faces of body 0 are `feature`'s blend faces.
fn blend_faces(p: &Part, feature: FeatureId) -> usize {
    p.bodies()[0]
        .face_names
        .iter()
        .filter(|n| {
            n.origins()
                .iter()
                .any(|o| o.feature == feature && matches!(o.role, FaceRole::Blend(_)))
        })
        .count()
}

/// How far from the axis the vertices on the vase's top face reach.
fn mouth_reach(p: &Part) -> f64 {
    p.bodies()[0]
        .solid
        .vertices
        .iter()
        .filter(|v| v.point.z > 23.9)
        .map(|v| v.point.truncate().length())
        .fold(0.0, f64::max)
}

#[test]
fn a_rim_fillet_follows_its_loft_through_edits() {
    // A vase through three circles: freeform sides, round rims in four arcs each.
    let mut p = Part::default();
    let foot = circle_at(&mut p, 0.0, 10.0);
    let waist = circle_at(&mut p, 12.0, 6.0);
    let mouth = circle_at(&mut p, 24.0, 9.0);
    p.model
        .add_loft(vec![foot, waist, mouth], Operation::NewBody);
    p.rebuild();
    p.assert_ok();
    let plain = p.volume();

    // One arc of the mouth's rim is picked; the chain carries the fillet all the way
    // round, across the four freeform sides.
    let rim = edge_where(&p, |c, at| {
        matches!(c, Curve3::Circle(_)) && (at.z - 24.0).abs() < 1e-9
    });
    let fillet = p.model.add_blend(BlendKind::Fillet, vec![rim]);
    blend_mut(&mut p, fillet).size = Scalar::new(1.5);
    p.rebuild();
    p.assert_ok();
    assert_eq!(blend_faces(&p, fillet), 4);
    let removed = plain - p.volume();
    assert!(removed > 55.0 && removed < 75.0, "{removed}");
    // The sides and the caps are still there under their own names.
    let sides = p.bodies()[0]
        .face_names
        .iter()
        .filter(|n| matches!(n.origins()[0].role, FaceRole::Side(_)))
        .count();
    assert_eq!(sides, 4);

    // The mouth gets wider: the fillet stays on its rim, and moves out with it.
    let before = mouth_reach(&p);
    p.set_dimension(mouth, "d1", "12");
    p.rebuild();
    p.assert_ok();
    assert_eq!(blend_faces(&p, fillet), 4);
    let after = mouth_reach(&p);
    assert!(
        before < 9.0 && after > before + 1.0 && after < 12.0,
        "{before} {after}"
    );
    // A different radius, then a chamfer instead.
    blend_mut(&mut p, fillet).size = Scalar::new(0.8);
    p.rebuild();
    p.assert_ok();
    blend_mut(&mut p, fillet).kind = BlendKind::Chamfer;
    p.rebuild();
    p.assert_ok();
    assert_eq!(blend_faces(&p, fillet), 4);

    // Far too big for the rim: refused in words, and the body passes through as it was.
    blend_mut(&mut p, fillet).kind = BlendKind::Fillet;
    blend_mut(&mut p, fillet).size = Scalar::new(20.0);
    p.rebuild();
    let m = failure(&p, fillet);
    assert!(m.starts_with("Fillet: ") && m.ends_with('.'), "{m}");
    assert_eq!(blend_faces(&p, fillet), 0);
}

#[test]
fn an_edge_between_freeform_faces_ends_on_the_caps() {
    // A duct through three rectangles: its sides are freeform and meet at sharp edges.
    let mut p = Part::default();
    let a = rectangle_at(&mut p, 0.0, 20.0, 20.0);
    let b = rectangle_at(&mut p, 10.0, 14.0, 16.0);
    let c = rectangle_at(&mut p, 20.0, 18.0, 12.0);
    p.model.add_loft(vec![a, b, c], Operation::NewBody);
    p.rebuild();
    p.assert_ok();
    let plain = p.volume();
    let faces = p.bodies()[0].solid.faces.len();
    let rail = edge_where(&p, |c, at| {
        matches!(c, Curve3::Nurbs(_)) && at.x > 0.0 && at.y > 0.0
    });
    let fillet = p.model.add_blend(BlendKind::Fillet, vec![rail]);
    blend_mut(&mut p, fillet).size = Scalar::new(2.0);
    p.rebuild();
    p.assert_ok();
    assert_eq!(blend_faces(&p, fillet), 1);
    assert_eq!(p.bodies()[0].solid.faces.len(), faces + 1);
    assert!(p.volume() < plain);
    // The fillet is freeform, and stops in the caps' planes.
    let solid = &p.bodies()[0].solid;
    assert!(
        solid
            .vertices
            .iter()
            .all(|v| v.point.z > -1e-6 && v.point.z < 20.0 + 1e-6)
    );
    assert_eq!(
        solid
            .faces
            .iter()
            .filter(|f| matches!(f.surface, Surface::Nurbs(_)))
            .count(),
        5
    );
    assert!((measure::volume(solid) - p.volume()).abs() < 1e-9);
}

#[test]
fn refusals_say_what_to_do() {
    // A rectangle lofted to a circle.
    let mut p = Part::default();
    let base = rectangle_at(&mut p, 0.0, 30.0, 20.0);
    let top = circle_at(&mut p, 25.0, 8.0);
    p.model.add_loft(vec![base, top], Operation::NewBody);
    p.rebuild();
    p.assert_ok();
    let volume = p.volume();
    // The round rim: just below it the sides meet at an angle, which needs a mitre.
    let rim = edge_where(&p, |c, at| {
        matches!(c, Curve3::Circle(_)) && (at.z - 25.0).abs() < 1e-9
    });
    let fillet = p.model.add_blend(BlendKind::Fillet, vec![rim]);
    blend_mut(&mut p, fillet).size = Scalar::new(1.0);
    p.rebuild();
    let m = failure(&p, fillet);
    assert!(
        m.starts_with("Fillet: Not supported yet: ") && m.contains("meet at an angle"),
        "{m}"
    );
    assert!((p.volume() - volume).abs() < 1e-9);
    // An edge up the side: the sides are tangent where they reach the circle.
    let rail = edge_where(&p, |c, at| {
        matches!(c, Curve3::Line(_)) && at.z > 5.0 && at.z < 20.0
    });
    blend_mut(&mut p, fillet).edges = vec![rail];
    p.rebuild();
    let m = failure(&p, fillet);
    assert!(m.contains("meet smoothly"), "{m}");
    // The rectangular rim, all four edges: corners between freeform faces.
    let foot: Vec<EdgeRef> = {
        let body = &p.bodies()[0];
        body.solid
            .edge_ids()
            .filter(|&e| body.solid.edge(e).point_at_fraction(0.5).z.abs() < 1e-9)
            .map(|e| body.edge_ref(e).unwrap())
            .collect()
    };
    assert_eq!(foot.len(), 4);
    blend_mut(&mut p, fillet).kind = BlendKind::Chamfer;
    blend_mut(&mut p, fillet).edges = foot;
    p.rebuild();
    let m = failure(&p, fillet);
    assert!(m.starts_with("Chamfer: ") && m.contains("mitred"), "{m}");
}
