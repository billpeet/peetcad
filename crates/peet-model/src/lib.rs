//! PeetCAD's feature model: what each feature means and how it rebuilds into solids.
//!
//! For now this covers extrusions (new body, add and cut) and placing sketches on faces.
//! The full parametric history (dependency graph, incremental regeneration, persistent
//! naming) arrives in Phase 3 and will grow out of this crate.

mod extrude;
mod placement;

pub use extrude::{
    EndCondition, Extrude, Operation, RegionSelection, apply_extrude, default_regions,
};
pub use placement::face_sketch_plane;

/// Why a feature failed to rebuild. The message is shown next to the feature.
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureError(pub String);

impl std::fmt::Display for FeatureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FeatureError {}

impl From<peet_kernel::KernelError> for FeatureError {
    fn from(e: peet_kernel::KernelError) -> Self {
        Self(e.to_string())
    }
}
