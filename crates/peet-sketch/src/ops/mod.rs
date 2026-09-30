//! Sketch editing operations: trim, extend, offset, mirror and fillet.
//!
//! Each operation edits the sketch in place and keeps it sensibly constrained: new
//! endpoints created at intersections get coincident / point-on-curve relations, copies get
//! symmetric relations, fillets get tangency, so the result stays parametric.

mod extend;
mod fillet;
mod mirror;
mod offset;
mod trim;

pub use extend::extend;
pub use fillet::fillet;
pub use mirror::mirror;
pub use offset::offset;
pub use trim::trim;

use crate::sketch::{EntityId, SketchError};

/// Why an editing operation could not be applied. Messages are shown to the user.
#[derive(Clone, Debug, PartialEq)]
pub enum OpError {
    Sketch(SketchError),
    /// The operation doesn't apply to this kind of entity.
    Unsupported(&'static str),
    /// Nothing to trim to / extend to / intersect with.
    NoIntersection,
    /// The fillet radius is too large for the corner, or the offset collapses geometry.
    TooLarge,
    /// The selected curves don't form a connected chain / don't meet at a corner.
    NotConnected,
    /// A referenced entity is missing.
    Missing(EntityId),
}

impl From<SketchError> for OpError {
    fn from(e: SketchError) -> Self {
        Self::Sketch(e)
    }
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sketch(e) => e.fmt(f),
            Self::Unsupported(what) => write!(f, "not supported: {what}"),
            Self::NoIntersection => write!(f, "no intersecting geometry found"),
            Self::TooLarge => write!(f, "the value is too large for this geometry"),
            Self::NotConnected => write!(f, "the selected curves are not connected"),
            Self::Missing(id) => write!(f, "entity {} does not exist", id.0),
        }
    }
}

impl std::error::Error for OpError {}
