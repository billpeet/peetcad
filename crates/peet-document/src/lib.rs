//! The open PeetCAD document, without a user interface.
//!
//! [`Document`] is what the application edits and what a script or a command line run
//! edits: the parametric model, its rebuild state, the undo history and the bodies of the
//! last rebuild. Every change goes through [`Document::change`], so it is one undo step
//! and is followed by a rebuild, whoever makes it.
//!
//! A [`Session`] is the documents that are open together, one of them current.
//!
//! The crate has no UI or GPU dependency. Bodies are tessellated only when something asks
//! for their triangles ([`BodyView::tess`]): the viewport, a mesh export or a file's mesh
//! cache. A run that only edits and saves the model never tessellates.

mod body;
mod convert;
mod document;
mod session;
mod solids;

pub use body::{BodyView, GeomRef, tolerance};
pub use document::{Document, FileLocation, ItemId, Persistent, SketchStatus, sketch_bounds};
pub use session::{DocId, Session};
pub use solids::{MassTotal, StepImported};
