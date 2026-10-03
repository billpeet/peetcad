//! PeetCAD's parametric core: a history-based feature model.
//!
//! - [`Model`]: the feature tree (the history) and the parameter table. The source of
//!   truth, and what is saved.
//! - [`feature`]: what features there are (sketches, extrusions, sheet metal, reference
//!   geometry) and how they refer to each other.
//! - [`sheet`]: sheet metal features (base flange, edge flange, sheet metal cut, hem,
//!   sketched bend, jog, mitre flange, corner, forms) on top of `peet-sheetmetal`.
//! - `pattern`: linear and circular patterns and mirrors of features.
//! - `convert`: turning a solid of constant wall thickness into a sheet metal body.
//! - `revolve`, `dressup`, [`hole`], `import`: revolves; fillets, chamfers, shells and
//!   draft; the hole wizard; bodies imported from other CAD systems.
//! - [`naming`]: persistent names for faces, edges and vertices, so references survive
//!   upstream edits.
//! - [`DependencyGraph`]: which feature uses which.
//! - [`Engine`]: incremental regeneration into bodies, with per-feature status.
//! - [`Assembly`]: parts placed relative to each other, inside a [`Model`] that is an
//!   assembly instead of a part.
//! - [`History`]: undo and redo.
//!
//! The crate has no UI or GPU dependency: everything here runs headless.

mod assembly;
mod convert;
mod dressup;
mod extrude;
pub mod feature;
pub mod hash;
mod history;
pub mod hole;
mod import;
mod loft;
mod mate;
mod material;
mod model;
pub mod naming;
mod pattern;
mod placement;
mod regen;
mod revolve;
pub mod samples;
pub mod sheet;
mod sweep;
mod units;

pub use assembly::{Assembly, CompId, Component, DefId, Definition};
pub use convert::ConvertToSheetFeature;
pub use dressup::{BlendFeature, BlendKind, DraftFeature, ShellFeature};
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
pub use hole::{HoleEnd, HoleFeature, HoleFit, HoleKind, HoleSizes, METRIC, MetricSize};
pub use import::{ImportFeature, ImportedSolid};
pub use loft::{LoftFeature, LoftInput, apply_loft};
pub use mate::{Drag, Mate, MateEnd, MateGeom, MateId, MateKind};
pub use material::{Material, STEEL_DENSITY};
pub use model::{Datum, DependencyGraph, Model, ModelV4, ModelV5};
pub use naming::{Body, EdgeRef, FaceName, FaceOrigin, FaceRef, FaceRole, Found, VertexRef};
pub use placement::face_sketch_plane;
pub use regen::{Engine, Evaluation, FeatureState, Instance, Output, SketchStatus, Stats, Status};
pub use revolve::{
    RevolveAxisRef, RevolveFeature, RevolveInput, apply_revolve, default_axis, sketch_axis,
};
pub use sweep::{SweepFeature, SweepInput, apply_sweep};

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
    /// The kernel's message as a sentence: it is shown next to the feature as it is.
    fn from(e: peet_kernel::KernelError) -> Self {
        dressup::sentence(&e)
    }
}
