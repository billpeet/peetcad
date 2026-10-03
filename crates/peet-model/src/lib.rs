//! PeetCAD's parametric core: a history-based feature model.
//!
//! - [`Model`]: the feature tree (the history) and the parameter table. The source of
//!   truth, and what is saved.
//! - [`feature`]: what features there are (sketches, extrusions, sheet metal, reference
//!   geometry) and how they refer to each other.
//! - [`sheet`]: sheet metal features (base flange, edge flange, sheet metal cut, hem,
//!   sketched bend, jog, mitre flange, corner, forms) on top of `peet-sheetmetal`.
//! - `pattern`: linear and circular patterns and mirrors of features.
//! - [`naming`]: persistent names for faces, edges and vertices, so references survive
//!   upstream edits.
//! - [`DependencyGraph`]: which feature uses which.
//! - [`Engine`]: incremental regeneration into bodies, with per-feature status.
//! - [`History`]: undo and redo.
//!
//! The crate has no UI or GPU dependency: everything here runs headless.

mod extrude;
pub mod feature;
pub mod hash;
mod history;
mod model;
pub mod naming;
mod pattern;
mod placement;
mod regen;
pub mod samples;
pub mod sheet;
mod units;

pub use extrude::{
    EndCondition, Extrude, ExtrudeInput, Operation, RegionSelection, apply_extrude, default_regions,
};
pub use feature::{
    Axis, AxisDef, AxisRef, BaseFlangeFeature, BendModelDef, CoordSystemDef, CornerFeature,
    EdgeFlangeFeature, ExtrudeFeature, Feature, FeatureId, FeatureKind, FormFeature, HemFeature,
    JogFeature, LinearDirection, MirrorFeature, MiterFlangeFeature, PatternDef, PatternFeature,
    PlaneDef, PlaneRef, PointDef, PointRef, Scalar, ScalarKind, SheetCutFeature, SheetSettingsDef,
    SketchFeature, SketchedBendFeature, StdAxis, StdPlane,
};
pub use history::History;
pub use model::{Datum, DependencyGraph, Model};
pub use naming::{Body, EdgeRef, FaceName, FaceOrigin, FaceRef, FaceRole, Found, VertexRef};
pub use placement::face_sketch_plane;
pub use regen::{Engine, Evaluation, FeatureState, Output, SketchStatus, Stats, Status};

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
