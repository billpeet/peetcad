//! Sketch splines through the model: extruded, cut, revolved, lofted and swept, with the
//! faces they make named after them, and the features that refuse them saying so.

mod common;

use std::f64::consts::PI;

use common::{FRONT, Part, TOP};
use peet_kernel::Surface;
use peet_math::DVec2;
use peet_model::{
    EndCondition, FaceRole, FeatureId, FeatureKind, Operation, PlaneRef, Scalar, Status, StdPlane,
};
use peet_sketch::region::find_regions;
use peet_sketch::{ConstraintKind, EntityId, Sketch, Solver};

fn v2(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

fn close(a: f64, b: f64) -> bool {
    // The kernel measures freeform faces by quadrature, good to a few parts in 10⁵.
    (a - b).abs() <= 1e-4 * b.abs().max(1.0)
}

/// A plate 40 wide and about 20 high whose top edge is a spline. Returns the spline.
fn wavy_plate(s: &mut Sketch) -> EntityId {
    let left = s.add_line(v2(0.0, 20.0), v2(0.0, 0.0));
    let bottom = s.add_line(v2(0.0, 0.0), v2(40.0, 0.0));
    let right = s.add_line(v2(40.0, 0.0), v2(40.0, 20.0));
    let top = s
        .add_spline(
            &[
                v2(40.0, 20.0),
                v2(28.0, 26.0),
                v2(12.0, 16.0),
                v2(0.0, 20.0),
            ],
            false,
        )
        .unwrap();
    // Joined end to end, like a profile drawn with the tools.
    let chain = [left, bottom, right, top, left];
    for w in chain.windows(2) {
        let (_, end) = s.endpoints(w[0]).unwrap();
        let (start, _) = s.endpoints(w[1]).unwrap();
        s.add_constraint(ConstraintKind::Coincident(end, start))
            .unwrap();
    }
    top
}

fn area(s: &Sketch) -> f64 {
    let profile = find_regions(s);
    assert_eq!(profile.regions.len(), 1);
    profile.regions[0].area()
}

fn failure(p: &Part, id: FeatureId) -> String {
    match p.status(id) {
        Status::Failed(m) => m.clone(),
        other => panic!("expected a failure, got {other:?}"),
    }
}

fn freeform_faces(p: &Part) -> usize {
    p.bodies()[0]
        .solid
        .faces
        .iter()
        .filter(|f| matches!(f.surface, Surface::Nurbs(_)))
        .count()
}

#[test]
fn extrude_a_spline_profile_and_edit_it() {
    let mut p = Part::default();
    let mut spline = EntityId(0);
    let sketch = p.sketch(TOP, |s| spline = wavy_plate(s));
    let boss = p.extrude(sketch, Operation::NewBody, |e| e.depth = Scalar::new(8.0));
    p.rebuild();
    p.assert_ok();
    let before = area(p.sketch_mut(sketch));
    assert!(close(p.volume(), before * 8.0), "{}", p.volume());
    assert_eq!(freeform_faces(&p), 1);
    // The freeform wall is named after the spline, like a line's wall after its line.
    let wall = |p: &Part| {
        let body = &p.bodies()[0];
        let i = body
            .solid
            .faces
            .iter()
            .position(|f| matches!(f.surface, Surface::Nurbs(_)))
            .unwrap();
        body.face_names[i].clone()
    };
    let name = wall(&p);
    assert_eq!(name.origins().len(), 1);
    assert_eq!(name.origins()[0].feature, boss);
    assert_eq!(name.origins()[0].role, FaceRole::Side(spline));

    // Drag a fit point (through the solver, as the application does) and rebuild.
    let s = p.sketch_mut(sketch);
    let points = s.spline_points(spline).unwrap().0.to_vec();
    let fix: Vec<_> = s
        .entities()
        .filter(|(id, e)| e.kind() == peet_sketch::EntityKind::Line && *id != spline)
        .map(|(id, _)| id)
        .collect();
    for line in fix {
        s.fix(line).unwrap();
    }
    s.set_point(points[1], v2(28.0, 34.0));
    let report = Solver::new().solve(s);
    assert!(report.converged);
    assert!(s.point(points[1]).distance(v2(28.0, 34.0)) < 1e-6);
    // The ends stayed on the lines they are joined to.
    assert!(s.point(points[0]).distance(v2(40.0, 20.0)) < 1e-6);
    let after = area(s);
    assert!(after > before + 10.0);
    p.rebuild();
    p.assert_ok();
    assert!(
        close(p.volume(), after * 8.0),
        "{} vs {}",
        p.volume(),
        after * 8.0
    );
    assert_eq!(wall(&p), name, "the wall keeps its name through the edit");
}

#[test]
fn cut_through_a_spline_sided_body_and_cut_with_a_spline() {
    let mut p = Part::default();
    let sketch = p.sketch(TOP, |s| {
        wavy_plate(s);
    });
    p.extrude(sketch, Operation::NewBody, |e| e.depth = Scalar::new(8.0));
    // A round hole straight through.
    let hole = p.sketch(TOP, |s| {
        s.add_circle(v2(20.0, 9.0), 4.0);
    });
    p.extrude(hole, Operation::Cut, |e| {
        e.end = EndCondition::ThroughAll;
        e.reverse = true;
    });
    p.rebuild();
    p.assert_ok();
    let plate = area(p.sketch_mut(sketch));
    assert!(
        close(p.volume(), (plate - PI * 16.0) * 8.0),
        "{}",
        p.volume()
    );

    // A closed spline as a pocket, cut half-way down from the top face.
    let pocket = p.sketch(TOP, |s| {
        s.add_spline(
            &[v2(5.0, 4.0), v2(11.0, 3.0), v2(12.0, 9.0), v2(6.0, 11.0)],
            true,
        )
        .unwrap();
    });
    p.extrude(pocket, Operation::Cut, |e| {
        e.depth = Scalar::new(3.0);
        e.reverse = true;
    });
    p.rebuild();
    p.assert_ok();
    let hollow = area(p.sketch_mut(pocket));
    assert!(
        close(p.volume(), (plate - PI * 16.0) * 8.0 - hollow * 3.0),
        "{}",
        p.volume()
    );
}

#[test]
fn revolve_a_spline_profile() {
    let mut p = Part::default();
    // A vase wall on the front plane, clear of the axis (the sketch's y axis).
    let profile = p.sketch(FRONT, |s| {
        s.add_line(v2(5.0, 0.0), v2(12.0, 0.0));
        s.add_spline(
            &[
                v2(12.0, 0.0),
                v2(18.0, 10.0),
                v2(11.0, 22.0),
                v2(14.0, 30.0),
            ],
            false,
        )
        .unwrap();
        s.add_line(v2(14.0, 30.0), v2(5.0, 30.0));
        s.add_line(v2(5.0, 30.0), v2(5.0, 0.0));
    });
    let vase = p.model.add_revolve(profile, Operation::NewBody);
    p.rebuild();
    p.assert_ok();
    // All the way round, the spline's face is in two halves, both named after it.
    assert_eq!(freeform_faces(&p), 2);
    let body = &p.bodies()[0];
    let names: Vec<_> = body
        .solid
        .faces
        .iter()
        .zip(&body.face_names)
        .filter(|(f, _)| matches!(f.surface, Surface::Nurbs(_)))
        .map(|(_, n)| n.clone())
        .collect();
    assert_eq!(names[0], names[1]);
    assert_eq!(names[0].origins()[0].feature, vase);
    assert!(matches!(names[0].origins()[0].role, FaceRole::Side(_)));
    // More than the cylinder inside the narrowest part, less than the one round the widest.
    let volume = p.volume();
    assert!(volume > PI * (11.0 * 11.0 - 25.0) * 30.0 * 0.9);
    assert!(volume < PI * (18.5 * 18.5 - 25.0) * 30.0);

    // A spline that reaches the axis is refused, with what to do instead.
    let mut p = Part::default();
    let dome = p.sketch(FRONT, |s| {
        s.add_line(v2(0.0, 0.0), v2(10.0, 0.0));
        s.add_spline(&[v2(10.0, 0.0), v2(8.0, 6.0), v2(0.0, 9.0)], false)
            .unwrap();
        s.add_line(v2(0.0, 9.0), v2(0.0, 0.0));
    });
    let revolve = p.model.add_revolve(dome, Operation::NewBody);
    p.rebuild();
    let message = failure(&p, revolve);
    assert!(
        message.contains("spline") && message.contains("axis"),
        "{message}"
    );
}

#[test]
fn loft_between_closed_splines() {
    let mut p = Part::default();
    let outline = |scale: f64| {
        [
            v2(10.0, 0.0),
            v2(4.0, 9.0),
            v2(-8.0, 5.0),
            v2(-9.0, -6.0),
            v2(3.0, -8.0),
        ]
        .map(|q| q * scale)
    };
    let base = p.sketch(TOP, |s| {
        s.add_spline(&outline(1.0), true).unwrap();
    });
    let plane = p
        .model
        .add(FeatureKind::Plane(peet_model::PlaneDef::Offset {
            from: PlaneRef::Standard(StdPlane::Top),
            distance: Scalar::new(20.0),
            flip: false,
        }));
    let top = p.sketch(PlaneRef::Feature(plane), |s| {
        s.add_spline(&outline(0.6), true).unwrap();
    });
    p.model.add_loft(vec![base, top], Operation::NewBody);
    p.rebuild();
    p.assert_ok();
    let (a0, a1) = (area(p.sketch_mut(base)), area(p.sketch_mut(top)));
    let frustum = 20.0 / 3.0 * (a0 + a1 + (a0 * a1).sqrt());
    assert!(
        (p.volume() - frustum).abs() < 1e-3 * frustum,
        "{}",
        p.volume()
    );
}

#[test]
fn sweep_takes_a_spline_profile_and_follows_a_spline_path() {
    // A spline section swept along a straight path is an extrusion.
    let mut p = Part::default();
    let section = p.sketch(PlaneRef::Standard(StdPlane::Right), |s| {
        s.add_spline(
            &[v2(4.0, 0.0), v2(0.0, 3.0), v2(-4.0, 0.0), v2(0.0, -3.0)],
            true,
        )
        .unwrap();
    });
    let path = p.sketch(TOP, |s| {
        s.add_line(v2(0.0, 0.0), v2(30.0, 0.0));
    });
    p.model.add_sweep(section, Some(path), Operation::NewBody);
    p.rebuild();
    p.assert_ok();
    let a = area(p.sketch_mut(section));
    assert!(close(p.volume(), a * 30.0), "{}", p.volume());

    // A pipe along a spline. The spline leaves the profile's plane at a slant (its end
    // direction can't be set), and the profile keeps that attitude: the volume is the
    // ring's area, as the path sees it, times the path's length.
    let mut p = Part::default();
    let mut ring = EntityId(0);
    let section = p.sketch(PlaneRef::Standard(StdPlane::Right), |s| {
        ring = s.add_circle(DVec2::ZERO, 4.0);
        s.add_circle(DVec2::ZERO, 2.5);
    });
    let mut spline = EntityId(0);
    let path = p.sketch(TOP, |s| {
        spline = s
            .add_spline(
                &[v2(0.0, 0.0), v2(20.0, 5.0), v2(40.0, -5.0), v2(60.0, 0.0)],
                false,
            )
            .unwrap();
    });
    let sweep = p.model.add_sweep(section, Some(path), Operation::NewBody);
    p.rebuild();
    p.assert_ok();
    let expected = |p: &mut Part| {
        let curve = p.sketch_mut(path).curve(spline).unwrap();
        let n = 4000;
        let length: f64 = (0..n)
            .map(|k| {
                curve
                    .point_at(f64::from(k) / f64::from(n))
                    .distance(curve.point_at(f64::from(k + 1) / f64::from(n)))
            })
            .sum();
        // The Right plane's normal is the sketch's X axis on Top.
        let slant = curve.tangent_at(0.0).x.abs();
        PI * (16.0 - 6.25) * length * slant
    };
    let want = expected(&mut p);
    assert!(close(p.volume(), want), "{} vs {want}", p.volume());
    // Freeform sides named after the profile's circles, flat ends.
    let body = &p.bodies()[0];
    let named = |role: FaceRole| {
        body.face_names
            .iter()
            .filter(|n| n.origins()[0].feature == sweep && n.origins()[0].role == role)
            .count()
    };
    assert_eq!(named(FaceRole::NearCap), 1);
    assert_eq!(named(FaceRole::FarCap), 1);
    assert_eq!(named(FaceRole::Side(ring)), 4);
    assert!(
        body.solid
            .faces
            .iter()
            .any(|f| matches!(f.surface, Surface::Nurbs(_)))
    );
    // A point of the spline moves: the pipe follows.
    let points = p.sketch_mut(path).spline_points(spline).unwrap().0.to_vec();
    p.sketch_mut(path).set_point(points[2], v2(40.0, -12.0));
    p.rebuild();
    p.assert_ok();
    let moved = expected(&mut p);
    assert!(moved > want);
    assert!(close(p.volume(), moved), "{} vs {moved}", p.volume());

    // What can't be followed says why.
    let s = p.sketch_mut(path);
    s.add_line(v2(60.0, 0.0), v2(60.0, 30.0));
    p.rebuild();
    let message = failure(&p, sweep);
    assert!(message.contains("has a corner"), "{message}");
    let s = p.sketch_mut(path);
    *s = Sketch::new();
    s.add_spline(&[v2(5.0, 0.0), v2(20.0, 5.0), v2(40.0, -5.0)], false)
        .unwrap();
    p.rebuild();
    let message = failure(&p, sweep);
    assert!(
        message.contains("must start on the profile's plane"),
        "{message}"
    );
    let s = p.sketch_mut(path);
    *s = Sketch::new();
    s.add_spline(
        &[v2(0.0, 0.0), v2(20.0, 15.0), v2(40.0, 0.0), v2(20.0, -15.0)],
        true,
    )
    .unwrap();
    p.rebuild();
    let message = failure(&p, sweep);
    assert!(message.contains("closed spline"), "{message}");
}

#[test]
fn sheet_metal_refuses_splines() {
    let mut p = Part::default();
    let sketch = p.sketch(TOP, |s| {
        wavy_plate(s);
    });
    let flange = p.model.add_base_flange(sketch);
    p.rebuild();
    let message = failure(&p, flange);
    assert!(
        message.contains("Sheet metal can't use splines"),
        "{message}"
    );
    assert!(message.contains("lines and arcs"), "{message}");
    // A construction spline is no part of the profile, and is no trouble.
    let mut p = Part::default();
    let sketch = p.sketch(TOP, |s| {
        peet_sketch::shapes::rectangle(s, v2(0.0, 0.0), v2(60.0, 40.0));
        let guide = s
            .add_spline(&[v2(5.0, 5.0), v2(30.0, 30.0), v2(55.0, 5.0)], false)
            .unwrap();
        s.set_construction(guide, true);
    });
    p.model.add_base_flange(sketch);
    p.rebuild();
    p.assert_ok();
}
