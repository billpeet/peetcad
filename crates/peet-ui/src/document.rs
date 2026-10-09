//! The open document as the UI uses it. The document itself (model, rebuilds, undo,
//! bodies) is headless and lives in `peet-document`; this adds what only an interactive
//! session has.

use peet_math::Plane;
use peet_sketch::Sketch;

pub use peet_document::{
    Document, FileLocation, ItemId, Persistent, Session, SketchStatus, sketch_bounds,
};

/// The sketch being edited: a working copy, written back to the model (as one undo step)
/// when editing ends.
#[derive(Clone, Debug)]
pub struct SketchItem {
    pub plane: Plane,
    /// Name of what the sketch lies on, for the properties panel.
    pub plane_name: String,
    pub sketch: Sketch,
    pub status: SketchStatus,
}
