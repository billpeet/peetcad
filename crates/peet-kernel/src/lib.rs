//! PeetCAD's modelling kernel: an analytic B-rep restricted (for now) to planes and
//! cylinders, with lines, circles and ellipses as edge curves. That covers extruded parts
//! and sheet metal completely, and is far more tractable than a general NURBS kernel.
//!
//! - [`geom`]: surfaces and curves
//! - [`topo`]: the B-rep data structure ([`Solid`])
//! - [`validate`]: topology and geometry checks (Euler, manifoldness, orientation)
//! - [`extrude`]: solids from sketch regions
//! - [`boolean`]: union / subtract / intersect, for add and cut features
//! - [`tessellate`]: display meshes, edge polylines and silhouettes
//!
//! Every operation returns a valid solid or a [`KernelError`]; it must never panic or
//! produce corrupt topology.

pub mod boolean;
mod error;
pub mod extrude;
pub mod geom;
pub mod tessellate;
pub mod topo;
pub mod validate;

pub use error::KernelError;
pub use geom::{Curve3, Cylinder, Surface};
pub use topo::{CoedgeId, EdgeId, FaceId, LoopId, ShellId, Solid, VertexId};
