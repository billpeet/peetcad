//! Shared helpers for the model's integration tests.
#![allow(dead_code)]

use std::sync::Arc;

use peet_kernel::validate::{measure, validate};
use peet_kernel::{FaceId, Surface};
use peet_math::{DVec2, DVec3, Plane};
use peet_model::{
    Body, Engine, Evaluation, FaceRef, FeatureId, Model, Operation, PlaneRef, Status, StdPlane,
};
use peet_sketch::{ConstraintKind, Sketch, shapes};

/// A model with its engine.
#[derive(Default)]
pub struct Part {
    pub model: Model,
    pub engine: Engine,
}

impl Part {
    pub fn rebuild(&mut self) -> &Evaluation {
        self.engine.regenerate(&mut self.model)
    }

    pub fn eval(&self) -> &Evaluation {
        self.engine.evaluation()
    }

    pub fn bodies(&self) -> &[Arc<Body>] {
        &self.eval().bodies
    }

    pub fn status(&self, id: FeatureId) -> &Status {
        self.eval().status(id).expect("the feature has a state")
    }

    /// Fails the test if any feature failed or any body is invalid.
    #[track_caller]
    pub fn assert_ok(&self) {
        for f in self.model.features() {
            if let Some(Status::Failed(m)) = self.eval().status(f.id) {
                panic!("{} failed: {m}", f.name);
            }
        }
        for b in self.bodies() {
            if let Err(problems) = validate(&b.solid) {
                panic!("invalid body: {problems:#?}");
            }
            assert_eq!(b.face_names.len(), b.solid.faces.len());
        }
    }

    pub fn volume(&self) -> f64 {
        self.bodies()
            .iter()
            .map(|b| measure::volume(&b.solid))
            .sum()
    }

    /// Adds a sketch and lets `draw` fill it.
    pub fn sketch(&mut self, plane: PlaneRef, draw: impl FnOnce(&mut Sketch)) -> FeatureId {
        let id = self.model.add_sketch(plane, Plane::TOP);
        draw(self.sketch_mut(id));
        id
    }

    pub fn sketch_mut(&mut self, id: FeatureId) -> &mut Sketch {
        &mut self
            .model
            .feature_mut(id)
            .and_then(|f| f.sketch_mut())
            .expect("a sketch")
            .sketch
    }

    /// Adds an extrusion of `sketch` and lets `edit` set it up.
    pub fn extrude(
        &mut self,
        sketch: FeatureId,
        operation: Operation,
        edit: impl FnOnce(&mut peet_model::Extrude),
    ) -> FeatureId {
        let id = self.model.add_extrude(sketch, operation);
        edit(&mut self.extrude_mut(id).params);
        id
    }

    pub fn extrude_mut(&mut self, id: FeatureId) -> &mut peet_model::ExtrudeFeature {
        self.model
            .feature_mut(id)
            .and_then(|f| f.extrude_mut())
            .expect("an extrusion")
    }

    /// Sets a sketch dimension (by name, like "d1") from user input.
    pub fn set_dimension(&mut self, sketch: FeatureId, name: &str, input: &str) {
        let params = self.model.parameters.clone();
        let s = self.sketch_mut(sketch);
        let dim = s
            .dimension_by_name(name)
            .expect("a dimension with that name");
        peet_sketch::expr::set_dimension_input(s, &params, dim, input).expect("a valid value");
    }

    /// The planar face with outward normal `n` whose plane passes through `through`.
    #[track_caller]
    pub fn face(&self, n: DVec3, through: DVec3) -> (usize, FaceId) {
        for (bi, body) in self.bodies().iter().enumerate() {
            for f in body.solid.face_ids() {
                if let Surface::Plane(p) = body.solid.face(f).surface
                    && body.solid.face_normal_at(f, through).dot(n) > 0.999
                    && p.signed_distance(through).abs() < 1e-9
                {
                    return (bi, f);
                }
            }
        }
        panic!("no face with normal {n} through {through}");
    }

    /// A persistent reference to that face.
    #[track_caller]
    pub fn face_ref(&self, n: DVec3, through: DVec3) -> FaceRef {
        let (body, face) = self.face(n, through);
        self.bodies()[body].face_ref(face)
    }

    #[track_caller]
    pub fn on_face(&self, n: DVec3, through: DVec3) -> PlaneRef {
        PlaneRef::Face(self.face_ref(n, through))
    }
}

pub const TOP: PlaneRef = PlaneRef::Standard(StdPlane::Top);
pub const FRONT: PlaneRef = PlaneRef::Standard(StdPlane::Front);

/// A fully defined rectangle with its lower-left corner at the sketch origin: `d1` is its
/// width and `d2` its height.
pub fn dimensioned_rectangle(s: &mut Sketch, width: f64, height: f64) {
    let shape = shapes::rectangle(s, DVec2::ZERO, DVec2::new(width, height));
    let (bottom, right) = (shape.curves[0], shape.curves[1]);
    let corner = s.endpoints(bottom).unwrap().0;
    s.add_constraint(ConstraintKind::Coincident(corner, Sketch::ORIGIN))
        .unwrap();
    s.add_dimension(ConstraintKind::Length(bottom), width)
        .unwrap();
    s.add_dimension(ConstraintKind::Length(right), height)
        .unwrap();
}

#[track_caller]
pub fn assert_close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6 * b.abs().max(1.0), "{a} vs {b}");
}
