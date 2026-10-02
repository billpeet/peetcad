//! Sample parts, built through the same API as the UI uses. They serve the exit-criterion
//! test, the rebuild benchmark and "open a sample" in the app.

use peet_kernel::Surface;
use peet_math::{DVec2, DVec3, Plane};
use peet_sketch::{ConstraintKind, Sketch, shapes};

use crate::feature::{FeatureKind, PlaneDef, PlaneRef, Scalar, StdPlane};
use crate::{EndCondition, Engine, FeatureId, Model, Operation};

/// A bracket with 20 features: a dimensioned base plate (its width is `d1` of Sketch1),
/// a pocket with a hole in its floor, four mounting holes, a rib on an offset plane with a
/// hole in its top, a slot in the end face, a boss underneath with a hole through it, and
/// a reference plane. Sketches sit on faces of earlier features wherever that makes sense,
/// so a change upstream exercises persistent naming everywhere.
pub fn bracket() -> (Model, Engine) {
    let mut b = Builder {
        model: Model::new(),
        engine: Engine::new(),
    };
    b.model.name = "Bracket".to_owned();
    let top_plane = PlaneRef::Standard(StdPlane::Top);
    let base = b.sketch(top_plane, |s| {
        let shape = shapes::rectangle(s, DVec2::ZERO, DVec2::new(120.0, 80.0));
        let (bottom, right) = (shape.curves[0], shape.curves[1]);
        let corner = s.endpoints(bottom).expect("a line").0;
        let _ = s.add_constraint(ConstraintKind::Coincident(corner, Sketch::ORIGIN));
        let _ = s.add_dimension(ConstraintKind::Length(bottom), 120.0);
        let _ = s.add_dimension(ConstraintKind::Length(right), 80.0);
    });
    b.extrude(base, Operation::Add, |e| e.depth = Scalar::new(8.0));

    let top = b.face(DVec3::Z, DVec3::new(1.0, 1.0, 8.0));
    let pocket = b.sketch(top.clone(), |s| {
        shapes::rectangle(s, DVec2::new(20.0, 20.0), DVec2::new(60.0, 60.0));
    });
    b.extrude(pocket, Operation::Cut, |e| e.depth = Scalar::new(3.0));

    let floor = b.face(DVec3::Z, DVec3::new(40.0, 40.0, 5.0));
    let drain = b.sketch(floor, |s| {
        s.add_circle(DVec2::new(40.0, 40.0), 6.0);
    });
    b.extrude(drain, Operation::Cut, |e| e.end = EndCondition::ThroughAll);

    let holes = b.sketch(top, |s| {
        for (x, y) in [(80.0, 15.0), (110.0, 15.0), (80.0, 65.0), (110.0, 65.0)] {
            s.add_circle(DVec2::new(x, y), 3.0);
        }
    });
    b.extrude(holes, Operation::Cut, |e| e.end = EndCondition::ThroughAll);

    let rib_plane = b.model.add(FeatureKind::Plane(PlaneDef::Offset {
        from: PlaneRef::Standard(StdPlane::Front),
        distance: Scalar::new(40.0),
        flip: true,
    }));
    let rib = b.sketch(PlaneRef::Feature(rib_plane), |s| {
        shapes::rectangle(s, DVec2::new(70.0, 8.0), DVec2::new(110.0, 28.0));
    });
    b.extrude(rib, Operation::Add, |e| {
        e.end = EndCondition::Symmetric;
        e.depth = Scalar::new(6.0);
    });

    let rib_top = b.face(DVec3::Z, DVec3::new(90.0, 40.0, 28.0));
    let rib_hole = b.sketch(rib_top, |s| {
        s.add_circle(DVec2::new(90.0, 40.0), 2.0);
    });
    b.extrude(rib_hole, Operation::Cut, |e| e.depth = Scalar::new(10.0));

    let end = b.face(DVec3::X, DVec3::new(120.0, 1.0, 1.0));
    let slot = b.sketch(end, |s| {
        shapes::rectangle(s, DVec2::new(30.0, 2.0), DVec2::new(50.0, 6.0));
    });
    b.extrude(slot, Operation::Cut, |e| e.depth = Scalar::new(15.0));

    let bottom = b.face(-DVec3::Z, DVec3::new(1.0, 1.0, 0.0));
    let boss = b.sketch(bottom, |s| {
        s.add_circle(DVec2::new(15.0, -70.0), 5.0);
    });
    b.extrude(boss, Operation::Add, |e| e.depth = Scalar::new(5.0));

    let boss_end = b.face(-DVec3::Z, DVec3::new(15.0, 70.0, -5.0));
    let boss_hole = b.sketch(boss_end, |s| {
        s.add_circle(DVec2::new(15.0, -70.0), 2.0);
    });
    b.extrude(boss_hole, Operation::Cut, |e| {
        e.end = EndCondition::ThroughAll
    });

    let top = b.face(DVec3::Z, DVec3::new(1.0, 1.0, 8.0));
    b.model.add(FeatureKind::Plane(PlaneDef::Offset {
        from: top,
        distance: Scalar::new(20.0),
        flip: false,
    }));
    b.engine.regenerate(&mut b.model);
    (b.model, b.engine)
}

/// The bracket's exact volume for a base plate `width` wide.
pub fn bracket_volume(width: f64) -> f64 {
    let pi = std::f64::consts::PI;
    width * 80.0 * 8.0 // plate
        - 40.0 * 40.0 * 3.0 // pocket
        - pi * 36.0 * 5.0 // hole in the pocket floor
        - 4.0 * pi * 9.0 * 8.0 // mounting holes
        + 40.0 * 6.0 * 20.0 // rib
        - pi * 4.0 * 10.0 // hole in the rib
        - 20.0 * 4.0 * 15.0 // slot in the end face
        + pi * 25.0 * 5.0 // boss
        - pi * 4.0 * 13.0 // hole through the boss and the plate
}

struct Builder {
    model: Model,
    engine: Engine,
}

impl Builder {
    fn sketch(&mut self, plane: PlaneRef, draw: impl FnOnce(&mut Sketch)) -> FeatureId {
        let id = self.model.add_sketch(plane, Plane::TOP);
        if let Some(s) = self.model.feature_mut(id).and_then(|f| f.sketch_mut()) {
            draw(&mut s.sketch);
        }
        id
    }

    fn extrude(
        &mut self,
        sketch: FeatureId,
        operation: Operation,
        edit: impl FnOnce(&mut crate::Extrude),
    ) -> FeatureId {
        let id = self.model.add_extrude(sketch, operation);
        if let Some(e) = self.model.feature_mut(id).and_then(|f| f.extrude_mut()) {
            edit(&mut e.params);
        }
        id
    }

    /// A reference to the planar face with outward normal `n` through `at`, in the model
    /// as built so far.
    fn face(&mut self, n: DVec3, at: DVec3) -> PlaneRef {
        let eval = self.engine.regenerate(&mut self.model);
        for body in &eval.bodies {
            for f in body.solid.face_ids() {
                if let Surface::Plane(p) = body.solid.face(f).surface
                    && body.solid.face_normal_at(f, at).dot(n) > 0.999
                    && p.signed_distance(at).abs() < 1e-9
                {
                    return PlaneRef::Face(body.face_ref(f));
                }
            }
        }
        panic!("the sample has no face with normal {n} through {at}");
    }
}
