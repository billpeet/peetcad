//! PeetCAD's modelling kernel: an analytic B-rep of planes, cylinders, cones, spheres and
//! tori, with lines, circles and ellipses as edge curves. That covers extruded and turned
//! parts and sheet metal completely, and is far more tractable than a general NURBS kernel.
//!
//! - [`geom`]: surfaces and curves
//! - [`topo`]: the B-rep data structure ([`Solid`])
//! - [`validate`]: topology and geometry checks (Euler, manifoldness, orientation)
//! - [`extrude`]: solids from sketch regions
//! - [`revolve`]: solids from sketch regions turned about an axis
//! - [`primitive`]: blocks and balls
//! - [`boolean`]: union / subtract / intersect, for add and cut features
//! - [`blend`]: fillets and chamfers
//! - [`reshape`]: offset faces, shell and draft
//! - [`tessellate`]: display meshes, edge polylines and silhouettes
//! - [`transform`]: rigid placement of geometry and solids
//! - [`query`]: mass properties and measurements
//!
//! Every operation returns a valid solid or a [`KernelError`]; it must never panic or
//! produce corrupt topology.

pub mod blend;
pub mod boolean;
mod error;
pub mod extrude;
pub mod geom;
pub mod primitive;
pub mod query;
pub mod reshape;
pub mod revolve;
pub mod tessellate;
pub mod topo;
pub mod transform;
pub mod validate;

pub use error::KernelError;
pub use geom::{Cone, Curve3, Cylinder, Sphere, Surface, Torus};
pub use topo::{CoedgeId, EdgeId, FaceId, LoopId, ShellId, Solid, VertexId};
