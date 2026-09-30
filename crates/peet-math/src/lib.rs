//! Double precision geometry primitives and the tolerance model for PeetCAD.
//!
//! All model geometry is `f64` and measured in millimetres. Rendering converts to
//! `f32` at the last moment, so nothing in the model depends on GPU precision.
//!
//! Coordinate convention: right handed, **Z up**. The standard reference planes are
//! Top (XY), Front (XZ) and Right (YZ).

mod aabb;
mod frame;
mod plane;
mod ray;
pub mod tolerance;

pub use aabb::Aabb;
pub use frame::Frame;
pub use plane::Plane;
pub use ray::Ray;

pub use glam::{DMat3, DMat4, DQuat, DVec2, DVec3, DVec4, EulerRot};

/// A position in model space (mm). Alias for readability; points and vectors share a type.
pub type Point3 = DVec3;
/// A direction or displacement in model space.
pub type Vector3 = DVec3;
/// A position in a 2D sketch plane (mm).
pub type Point2 = DVec2;
