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
//! - [`flange`]: edge flanges, hems and sketched flange profiles
//! - [`corner`]: corners where two flanges meet: reliefs, gaps and how the flanges end
//! - [`form`]: dimples, embosses and louvers
//! - [`split`]: sketched bends and jogs, which bend a flange along a line
//! - [`build`]: the flat and folded solids, the flat pattern outline and bend lines
//! - [`report`]: the bend table and flat size
//! - [`checks`]: manufacturing checks (short flanges, holes near bends and edges, collisions)
//! - [`gauge`]: gauge and material tables, shareable as CSV
//! - [`recognize`]: the way back: reading a layout out of a plain solid of constant
//!   thickness (converting a solid to sheet metal)
//!
//! The crate is headless and has no knowledge of features: owners are plain numbers that
//! the model maps to its feature ids.

pub mod build;
pub mod checks;
pub mod corner;
pub mod flange;
pub mod form;
pub mod gauge;
pub mod layout;
pub mod recognize;
pub mod report;
pub mod settings;
pub mod split;

#[cfg(test)]
mod tests;

pub use build::{BendLine, FaceTag, FlatLoop, SheetBody, SheetError, build};
pub use checks::{CheckKind, CheckRules, Finding, Rule, Severity, check, error_finding};
pub use corner::{Corner, CornerKind, CornerRelief, CornerSpec};
pub use flange::{Attachment, EdgeFlangeSpec, HemKind, HemSpec, Segment};
pub use form::{Form, FormKind, FormMark, FormShape};
pub use gauge::{CsvError, GaugeEntry, GaugeTable, MaterialLibrary};
pub use layout::{
    Area, Bend, ChainLine, CurveTag, Cut, Edge2, EdgeSite, Layout, Origin, Piece, PieceKind,
};
pub use recognize::{Recognized, recognize, recognize_layout};
pub use report::{BendRow, Report};
pub use settings::{BendModel, BendValues, FlangePosition, ReliefType, SheetSettings};
pub use split::{BendLinePosition, JogDimension, JogSpec, SketchedBendSpec, Split};
