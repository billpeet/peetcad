//! PeetCAD sheet metal.
//!
//! A sheet metal body is represented *natively*, not recognised from a solid: a
//! [`Layout`] of flat pieces (flanges and bends) in flat-pattern coordinates, plus the
//! thickness, bend radius and bend model ([`SheetSettings`]). From that one definition
//! [`build`] derives both the folded solid and the flat pattern, so unfolding is exact and
//! costs nothing.
//!
//! - [`settings`]: thickness, radius, K-factor / bend allowance / bend deduction, reliefs
//! - [`layout`]: pieces, bends and cuts; base flanges (plates and open profiles), edge
//!   flanges
//! - [`build`]: the flat and folded solids, the flat pattern outline and bend lines
//! - [`report`]: the bend table and flat size
//!
//! The crate is headless and has no knowledge of features: owners are plain numbers that
//! the model maps to its feature ids.

pub mod build;
pub mod layout;
pub mod report;
pub mod settings;

#[cfg(test)]
mod tests;

pub use build::{BendLine, FaceTag, FlatLoop, SheetBody, SheetError, build};
pub use layout::{
    Area, Bend, ChainLine, CurveTag, Cut, Edge2, EdgeFlangeSpec, EdgeSite, Layout, Origin, Piece,
    PieceKind,
};
pub use report::{BendRow, Report};
pub use settings::{BendModel, BendValues, FlangePosition, ReliefType, SheetSettings};
