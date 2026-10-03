//! PeetCAD's modelling kernel: a B-rep of planes, cylinders, cones, spheres and tori,
//! with lines, circles and ellipses as edge curves, plus NURBS curves and surfaces for
//! what those can't hold. Extruded and turned parts and sheet metal are analytic
//! throughout, which keeps them exact; freeform geometry comes in with lofts and with
//! other systems' files.
//!
//! - [`geom`]: surfaces and curves
//! - [`topo`]: the B-rep data structure ([`Solid`])
//! - [`validate`]: topology and geometry checks (Euler, manifoldness, orientation)
//! - [`extrude`]: solids from sketch regions
//! - [`revolve`]: solids from sketch regions turned about an axis
//! - [`loft`]: solids through a series of profiles
//! - [`nurbs`]: freeform curves and surfaces
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
pub mod loft;
pub mod nurbs;
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
