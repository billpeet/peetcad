//! 2D sketches for PeetCAD: entities, constraints, dimensions and the constraint solver.
//!
//! This crate is headless (no UI or GPU dependency) and works in sketch-plane coordinates
//! (mm, `f64`). Placing a sketch in 3D is the caller's business (see `peet_math::Plane`).
//!
//! - [`sketch`]: the data model ([`Sketch`], entities, constraints, dimensions)
//! - [`curve`]: resolved curve geometry, closest points and intersections
//! - [`spline`]: splines through fit points, as exact B-spline curves
//! - [`solver`]: the geometric constraint solver, DOF and redundancy analysis
//! - [`expr`]: expressions and named parameters for dimension values
//! - [`infer`]: automatic constraint inference while drawing
//! - [`shapes`]: rectangle, slot and polygon helpers
//! - [`ops`]: trim, extend, offset, mirror and fillet
//! - [`region`]: closed loop and region detection for features
//! - [`text`]: sketch text, its glyph outlines as lines and arcs
//! - [`triangulate`]: triangulation of regions, for shading profiles

pub mod curve;
pub mod expr;
pub mod infer;
pub mod ops;
pub mod region;
pub mod shapes;
pub mod sketch;
pub mod solver;
pub mod spline;
pub mod text;
pub mod triangulate;

pub use curve::{Curve, Intersection};
pub use sketch::{
    Along, ArcSide, Constraint, ConstraintId, ConstraintKind, Dimension, Entity, EntityId,
    EntityKind, Geometry, SideTerms, Sketch, SketchError, side_terms,
};
pub use solver::{Analysis, Drag, SolveReport, Solver};
pub use spline::{Spline, SplinePiece};
