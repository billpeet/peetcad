//! Features: the steps of a part's history, and the references between them.
//!
//! A feature never stores resolved geometry of another feature. It stores a *reference*
//! (to a feature by id, or to a face, edge or vertex by its persistent name, see
//! [`crate::naming`]) which is resolved again on every rebuild. That is what makes the
//! model parametric: a sketch drawn on a face follows the face when the face moves.

use peet_math::{DVec3, Plane};
use peet_sketch::Sketch;
use peet_sketch::expr::{Expr, Parameters};
use serde::{Deserialize, Serialize};

use peet_sheetmetal::corner::{CornerKind, CornerRelief};
use peet_sheetmetal::{
    BendLinePosition, FlangePosition, FormKind, HemKind, JogDimension, ReliefType,
};

use crate::convert::ConvertToSheetFeature;
use crate::dressup::{BlendFeature, BlendKind, DraftFeature, ShellFeature};
use crate::extrude::Extrude;
use crate::hole::HoleFeature;
use crate::import::ImportFeature;
use crate::loft::LoftFeature;
use crate::naming::{EdgeRef, FaceRef, VertexRef};
use crate::revolve::{RevolveAxisRef, RevolveFeature};
use crate::sweep::SweepFeature;

/// Stable identifier of a feature within its model. Never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FeatureId(pub u32);

/// What a typed value means, which decides the unit a plain number is taken in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarKind {
    /// Millimetres internally; a plain number is in the document's length unit.
    Length,
    /// Degrees.
    Angle,
    /// A plain number (a K-factor, a ratio).
    Number,
}

/// A number the user entered: a plain value, or an expression over the parameter table
/// (`2 * thickness`), re-evaluated on every rebuild.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scalar {
    /// The value in base units (mm or degrees). For expressions this is the value when
    /// the expression was last entered, kept as a fallback.
    pub value: f64,
    pub expression: Option<String>,
}

impl Scalar {
    pub fn new(value: f64) -> Self {
        Self {
            value,
            expression: None,
        }
    }

    /// The current value: the expression evaluated against `params`, or the plain value.
    pub fn evaluate(&self, kind: ScalarKind, params: &Parameters) -> Result<f64, String> {
        match &self.expression {
            None => Ok(self.value),
            Some(source) => evaluate_expression(source, kind, params),
        }
    }

    /// Sets the value from user input. Input that refers to no parameter (`25`, `1in`,
    /// `2 * 15mm`) is stored as a plain value, so it keeps its size if the document's unit
    /// changes later; anything else is stored as an expression. Unchanged on error.
    pub fn set_input(
        &mut self,
        input: &str,
        kind: ScalarKind,
        params: &Parameters,
    ) -> Result<f64, String> {
        let input = input.trim();
        let value = evaluate_expression(input, kind, params)?;
        let constant = Expr::parse(input).is_ok_and(|e| e.names().is_empty());
        self.value = value;
        self.expression = (!constant).then(|| input.to_owned());
        Ok(value)
    }

    /// What to show in an edit field: the expression, or the value in document units.
    pub fn input_text(&self, kind: ScalarKind, params: &Parameters) -> String {
        match &self.expression {
            Some(e) => e.clone(),
            None => match kind {
                ScalarKind::Length => crate::units::length_value_text(self.value, params),
                ScalarKind::Angle | ScalarKind::Number => format_number(self.value),
            },
        }
    }

    /// Names of the parameters the expression refers to.
    pub fn names(&self) -> Vec<String> {
        self.expression
            .as_deref()
            .and_then(|e| Expr::parse(e).ok())
            .map(|e| e.names())
            .unwrap_or_default()
    }
}

fn evaluate_expression(source: &str, kind: ScalarKind, params: &Parameters) -> Result<f64, String> {
    let value = crate::units::evaluate(source, kind, params)?;
    if value.is_finite() {
        Ok(value)
    } else {
        Err("the value is not a finite number".to_owned())
    }
}

/// A number without noise: up to six decimals, no trailing zeros.
pub fn format_number(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".to_owned()
    } else {
        s.to_owned()
    }
}

/// The three planes every part starts with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StdPlane {
    Front,
    Top,
    Right,
}

impl StdPlane {
    pub const ALL: [Self; 3] = [Self::Front, Self::Top, Self::Right];

    pub fn label(self) -> &'static str {
        match self {
            Self::Front => "Front Plane",
            Self::Top => "Top Plane",
            Self::Right => "Right Plane",
        }
    }

    pub fn plane(self) -> Plane {
        match self {
            Self::Front => Plane::front(),
            Self::Top => Plane::TOP,
            Self::Right => Plane::right(),
        }
    }
}

/// The axes of the part's origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StdAxis {
    X,
    Y,
    Z,
}

impl StdAxis {
    pub const ALL: [Self; 3] = [Self::X, Self::Y, Self::Z];

    pub fn label(self) -> &'static str {
        match self {
            Self::X => "X Axis",
            Self::Y => "Y Axis",
            Self::Z => "Z Axis",
        }
    }

    pub fn axis(self) -> Axis {
        Axis {
            origin: DVec3::ZERO,
            dir: match self {
                Self::X => DVec3::X,
                Self::Y => DVec3::Y,
                Self::Z => DVec3::Z,
            },
        }
    }
}

/// An infinite line with a direction: a reference axis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Axis {
    pub origin: DVec3,
    /// Unit direction.
    pub dir: DVec3,
}

/// Something flat to sketch on or measure from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlaneRef {
    Standard(StdPlane),
    /// A reference plane feature (or a coordinate system: its XY plane).
    Feature(FeatureId),
    /// A planar face of a body.
    Face(FaceRef),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AxisRef {
    Standard(StdAxis),
    /// A reference axis feature (or a coordinate system: its Z axis).
    Feature(FeatureId),
    /// Along a straight edge (or through the centre of a round one).
    Edge(EdgeRef),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PointRef {
    Origin,
    /// A reference point feature (or a coordinate system: its origin).
    Feature(FeatureId),
    Vertex(VertexRef),
}

/// How a reference plane is defined.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PlaneDef {
    /// Parallel to `from`, moved along its normal (against it when `flip` is set).
    Offset {
        from: PlaneRef,
        distance: Scalar,
        flip: bool,
    },
    /// `from` turned about an axis lying in it (or parallel to it) by `angle` degrees.
    Angled {
        from: PlaneRef,
        about: AxisRef,
        angle: Scalar,
    },
    /// Halfway between two parallel planes.
    Midplane { a: PlaneRef, b: PlaneRef },
}

/// How a reference axis is defined.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AxisDef {
    /// Along a straight edge.
    Edge(EdgeRef),
    /// The axis of a cylindrical face (a hole or a round boss).
    Cylinder(FaceRef),
    /// Where two planes meet.
    TwoPlanes(PlaneRef, PlaneRef),
}

/// How a reference point is defined.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PointDef {
    /// At a vertex of a body.
    Vertex(VertexRef),
    /// At model coordinates.
    Coordinates { x: Scalar, y: Scalar, z: Scalar },
}

/// A coordinate system: an origin point, with the axes of a plane (Z along its normal).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoordSystemDef {
    pub origin: PointRef,
    pub orientation: PlaneRef,
}

/// A sketch placed on a plane or a face.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SketchFeature {
    pub plane: PlaneRef,
    /// Where `plane` was the last time it could be resolved. This is what the sketch is
    /// drawn on if its plane goes missing (the face it was on is gone), so it can still
    /// be opened and moved to another plane.
    pub placement: Plane,
    pub sketch: Sketch,
    /// Model references refreshed into locked sketch geometry before each solve.
    #[serde(default)]
    pub projections: Vec<crate::projection::Projection>,
}

/// An extrusion of a sketch's regions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtrudeFeature {
    pub sketch: FeatureId,
    pub params: Extrude,
}

/// How a sheet metal body works out the flat length of its bends, as entered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum BendModelDef {
    KFactor(Scalar),
    /// Bend allowance in mm.
    Allowance(Scalar),
    /// Bend deduction in mm.
    Deduction(Scalar),
}

/// A sheet metal body's settings, as entered (values may be expressions).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SheetSettingsDef {
    pub thickness: Scalar,
    /// Default inner bend radius.
    pub radius: Scalar,
    pub model: BendModelDef,
    pub relief: ReliefType,
    /// Relief width and how far it reaches past the bend, as a multiple of the thickness.
    pub relief_ratio: Scalar,
}

impl Default for SheetSettingsDef {
    fn default() -> Self {
        let d = peet_sheetmetal::SheetSettings::default();
        let k = match d.model {
            peet_sheetmetal::BendModel::KFactor(k) => k,
            _ => 0.44,
        };
        Self {
            thickness: Scalar::new(d.thickness),
            radius: Scalar::new(d.radius),
            model: BendModelDef::KFactor(Scalar::new(k)),
            relief: d.relief,
            relief_ratio: Scalar::new(d.relief_ratio),
        }
    }
}

impl SheetSettingsDef {
    /// Every value that can hold an expression.
    pub fn scalars(&self) -> Vec<&Scalar> {
        let model = match &self.model {
            BendModelDef::KFactor(v) | BendModelDef::Allowance(v) | BendModelDef::Deduction(v) => v,
        };
        vec![&self.thickness, &self.radius, model, &self.relief_ratio]
    }
}

/// The first feature of a sheet metal body: a plate from a closed sketch, or a profile of
/// lines from an open one, with a bend at every corner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BaseFlangeFeature {
    pub sketch: FeatureId,
    pub settings: SheetSettingsDef,
    /// Put the thickness on the other side: against the sketch normal for a plate, to the
    /// right of the lines for an open profile.
    pub reverse: bool,
    /// Open profiles: how far the profile is extruded.
    pub depth: Scalar,
    /// Open profiles: extrude to both sides of the sketch plane.
    pub symmetric: bool,
    /// Open profiles: extrude against the sketch normal.
    pub flip_depth: bool,
}

impl BaseFlangeFeature {
    pub fn new(sketch: FeatureId) -> Self {
        Self {
            sketch,
            settings: SheetSettingsDef::default(),
            reverse: false,
            depth: Scalar::new(50.0),
            symmetric: false,
            flip_depth: false,
        }
    }
}

/// A flange added on an edge of a sheet metal body, with a bend.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeFlangeFeature {
    /// The edge: where a flat face of the sheet meets its side. `None` until picked.
    pub edge: Option<EdgeRef>,
    /// Outside length: from the outer virtual sharp to the end of the flange.
    pub length: Scalar,
    /// Bend angle in degrees (90: square to the face).
    pub angle: Scalar,
    pub position: FlangePosition,
    /// Set-back of the flange from the start and the end of the edge.
    pub offset_start: Scalar,
    pub offset_end: Scalar,
    /// Bend to the other side of the sheet.
    pub flip: bool,
    /// Inner bend radius, if not the body's default.
    pub radius: Option<Scalar>,
}

impl EdgeFlangeFeature {
    pub fn new(edge: Option<EdgeRef>) -> Self {
        Self {
            edge,
            length: Scalar::new(20.0),
            angle: Scalar::new(90.0),
            position: FlangePosition::MaterialInside,
            offset_start: Scalar::new(0.0),
            offset_end: Scalar::new(0.0),
            flip: false,
            radius: None,
        }
    }
}

/// A cut through a sheet metal body, square to the sheet, from a sketch on one of its flat
/// faces. It is made in the flat pattern, so it can run across bends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SheetCutFeature {
    pub sketch: FeatureId,
}

/// A hem on an edge of a sheet metal body: the edge folded back over the sheet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HemFeature {
    /// The edge: where a flat face of the sheet meets its side. `None` until picked.
    pub edge: Option<EdgeRef>,
    pub kind: HemKind,
    /// Closed and open hems: from the outside of the fold to the end. Teardrop: the
    /// flat end's length.
    pub length: Scalar,
    /// Open hems: the gap between the sheet and the hem.
    pub gap: Scalar,
    /// Teardrop and rolled hems: the inner radius.
    pub radius: Scalar,
    /// Teardrop and rolled hems: the angle turned through, in degrees.
    pub angle: Scalar,
    /// The fold's outside is flush with the original edge (else the fold starts there).
    pub inside: bool,
    pub offset_start: Scalar,
    pub offset_end: Scalar,
    /// Fold to the other side of the sheet.
    pub flip: bool,
}

impl HemFeature {
    pub fn new(edge: Option<EdgeRef>) -> Self {
        Self {
            edge,
            kind: HemKind::Closed,
            length: Scalar::new(10.0),
            gap: Scalar::new(1.0),
            radius: Scalar::new(1.0),
            angle: Scalar::new(270.0),
            inside: true,
            offset_start: Scalar::new(0.0),
            offset_end: Scalar::new(0.0),
            flip: false,
        }
    }
}

/// Bends a flat face of a sheet metal body along the lines of a sketch drawn on it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SketchedBendFeature {
    pub sketch: FeatureId,
    /// Bend angle in degrees.
    pub angle: Scalar,
    /// Inner radius, if not the body's default.
    pub radius: Option<Scalar>,
    pub position: BendLinePosition,
    /// Bend away from the face the sketch is on (else towards it).
    pub flip: bool,
    /// On the part's first face: keep the other side fixed.
    pub flip_fixed: bool,
}

impl SketchedBendFeature {
    pub fn new(sketch: FeatureId) -> Self {
        Self {
            sketch,
            angle: Scalar::new(90.0),
            radius: None,
            position: BendLinePosition::Centerline,
            flip: false,
            flip_fixed: false,
        }
    }
}

/// A step in a flat face: two bends along a sketched line, offsetting the far side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JogFeature {
    pub sketch: FeatureId,
    pub offset: Scalar,
    pub dimension: JogDimension,
    /// Angle of both bends, in degrees.
    pub angle: Scalar,
    pub radius: Option<Scalar>,
    pub position: BendLinePosition,
    /// Step away from the face the sketch is on (else towards it).
    pub flip: bool,
    pub flip_fixed: bool,
}

impl JogFeature {
    pub fn new(sketch: FeatureId) -> Self {
        Self {
            sketch,
            offset: Scalar::new(10.0),
            dimension: JogDimension::Overall,
            angle: Scalar::new(90.0),
            radius: None,
            position: BendLinePosition::BendOutside,
            flip: false,
            flip_fixed: false,
        }
    }
}

/// A flange with a sketched profile, run along a chain of edges of one face.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MiterFlangeFeature {
    /// The profile: connected lines drawn square to one of the edges, starting at it.
    pub sketch: FeatureId,
    pub edges: Vec<EdgeRef>,
    /// The gap left where the flanges of neighbouring edges meet.
    pub gap: Scalar,
    pub offset_start: Scalar,
    pub offset_end: Scalar,
}

impl MiterFlangeFeature {
    pub fn new(sketch: FeatureId, edges: Vec<EdgeRef>) -> Self {
        Self {
            sketch,
            edges,
            gap: Scalar::new(0.1),
            offset_start: Scalar::new(0.0),
            offset_end: Scalar::new(0.0),
        }
    }
}

/// How the corners where flanges meet are treated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CornerFeature {
    /// Faces of the flanges at the corners (their ends or their sides). None: every
    /// corner of the body.
    pub faces: Vec<FaceRef>,
    pub kind: CornerKind,
    pub gap: Scalar,
    pub relief: CornerRelief,
    /// How far a rectangular relief reaches past the bends.
    pub relief_size: Scalar,
}

impl CornerFeature {
    pub fn new(faces: Vec<FaceRef>) -> Self {
        Self {
            faces,
            kind: CornerKind::Butt,
            gap: Scalar::new(0.1),
            relief: CornerRelief::Rectangular,
            relief_size: Scalar::new(1.0),
        }
    }
}

/// Dimples, embosses or louvers from the shapes of a sketch on a flat face.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FormFeature {
    pub sketch: FeatureId,
    pub kind: FormKind,
    /// How far the plateau stands out of the sheet.
    pub height: Scalar,
    /// Press into the face the sketch is on (else stand out of it).
    pub flip: bool,
    /// Louvers: which side of each outline is open (counted round the outline).
    pub open_side: u32,
}

impl FormFeature {
    pub fn new(sketch: FeatureId, kind: FormKind) -> Self {
        Self {
            sketch,
            kind,
            height: Scalar::new(3.0),
            flip: false,
            open_side: 0,
        }
    }
}

/// Text dumps from older versions stored counts as integers. The binary layout uses
/// the schema-specific migrations; current text also accepts the scalar representation.
fn deserialize_pattern_count<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Scalar, D::Error> {
    if !d.is_human_readable() {
        return Scalar::deserialize(d);
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum CountInput {
        Scalar(Scalar),
        Number(f64),
    }
    CountInput::deserialize(d).map(|input| match input {
        CountInput::Scalar(s) => s,
        CountInput::Number(n) => Scalar::new(n),
    })
}

/// One direction of a linear pattern.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinearDirection {
    pub direction: AxisRef,
    pub spacing: Scalar,
    /// How many, the original included.
    #[serde(deserialize_with = "deserialize_pattern_count")]
    pub count: Scalar,
    pub flip: bool,
}

/// How a pattern places its copies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PatternDef {
    /// In a row along a direction, or a grid along two.
    Linear {
        first: LinearDirection,
        second: Option<LinearDirection>,
    },
    /// Around an axis: `count` copies (the original included) over `angle` degrees; 360
    /// spaces them evenly all the way round.
    Circular {
        axis: AxisRef,
        #[serde(deserialize_with = "deserialize_pattern_count")]
        count: Scalar,
        angle: Scalar,
        flip: bool,
    },
}

/// Copies of features (extrusions, cuts, revolves, holes, sheet metal cuts, forms) in a
/// pattern.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PatternFeature {
    pub seeds: Vec<FeatureId>,
    pub def: PatternDef,
}

/// Mirror images of features across a plane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MirrorFeature {
    pub seeds: Vec<FeatureId>,
    pub plane: PlaneRef,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FeatureKind {
    Sketch(Box<SketchFeature>),
    Extrude(Box<ExtrudeFeature>),
    Plane(PlaneDef),
    Axis(AxisDef),
    Point(PointDef),
    CoordSystem(CoordSystemDef),
    BaseFlange(Box<BaseFlangeFeature>),
    EdgeFlange(Box<EdgeFlangeFeature>),
    SheetCut(Box<SheetCutFeature>),
    Hem(Box<HemFeature>),
    SketchedBend(Box<SketchedBendFeature>),
    Jog(Box<JogFeature>),
    MiterFlange(Box<MiterFlangeFeature>),
    Corner(Box<CornerFeature>),
    Form(Box<FormFeature>),
    Pattern(Box<PatternFeature>),
    Mirror(Box<MirrorFeature>),
    // New kinds go at the end: files store the variant's index.
    Revolve(Box<RevolveFeature>),
    Blend(Box<BlendFeature>),
    Shell(Box<ShellFeature>),
    Draft(Box<DraftFeature>),
    Hole(Box<HoleFeature>),
    Import(Box<ImportFeature>),
    Sweep(Box<SweepFeature>),
    Loft(Box<LoftFeature>),
    ConvertToSheet(Box<ConvertToSheetFeature>),
}

impl FeatureKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Sketch(_) => "Sketch",
            Self::Extrude(e) if e.params.operation == crate::Operation::Cut => "Cut-extrude",
            Self::Extrude(_) => "Extrude",
            Self::Plane(_) => "Reference plane",
            Self::Axis(_) => "Reference axis",
            Self::Point(_) => "Reference point",
            Self::CoordSystem(_) => "Coordinate system",
            Self::BaseFlange(_) => "Base flange",
            Self::EdgeFlange(_) => "Edge flange",
            Self::SheetCut(_) => "Sheet metal cut",
            Self::Hem(_) => "Hem",
            Self::SketchedBend(_) => "Sketched bend",
            Self::Jog(_) => "Jog",
            Self::MiterFlange(_) => "Miter flange",
            Self::Corner(_) => "Corner",
            Self::Form(f) => match f.kind {
                FormKind::Dimple => "Dimple",
                FormKind::Emboss => "Emboss",
                FormKind::Louver => "Louver",
            },
            Self::Pattern(p) => match p.def {
                PatternDef::Linear { .. } => "Linear pattern",
                PatternDef::Circular { .. } => "Circular pattern",
            },
            Self::Mirror(_) => "Mirror",
            Self::Revolve(r) if r.operation == crate::Operation::Cut => "Cut-revolve",
            Self::Revolve(_) => "Revolve",
            Self::Blend(b) => match b.kind {
                BlendKind::Fillet => "Fillet",
                BlendKind::Chamfer => "Chamfer",
            },
            Self::Shell(_) => "Shell",
            Self::Draft(_) => "Draft",
            Self::Hole(_) => "Hole",
            Self::Import(_) => "Imported body",
            Self::Sweep(s) if s.operation == crate::Operation::Cut => "Cut-sweep",
            Self::Sweep(_) => "Sweep",
            Self::Loft(l) if l.operation == crate::Operation::Cut => "Cut-loft",
            Self::Loft(_) => "Loft",
            Self::ConvertToSheet(_) => "Convert to sheet metal",
        }
    }

    /// The prefix of automatic names ("Sketch" in "Sketch3").
    pub fn name_prefix(&self) -> &'static str {
        match self {
            Self::Sketch(_) => "Sketch",
            Self::Extrude(e) if e.params.operation == crate::Operation::Cut => "Cut-Extrude",
            Self::Extrude(_) => "Extrude",
            Self::Plane(_) => "Plane",
            Self::Axis(_) => "Axis",
            Self::Point(_) => "Point",
            Self::CoordSystem(_) => "Coordinate System",
            Self::BaseFlange(_) => "Base-Flange",
            Self::EdgeFlange(_) => "Edge-Flange",
            Self::SheetCut(_) => "Sheet-Cut",
            Self::Hem(_) => "Hem",
            Self::SketchedBend(_) => "Sketched-Bend",
            Self::Jog(_) => "Jog",
            Self::MiterFlange(_) => "Miter-Flange",
            Self::Corner(_) => "Corner",
            Self::Form(f) => match f.kind {
                FormKind::Dimple => "Dimple",
                FormKind::Emboss => "Emboss",
                FormKind::Louver => "Louver",
            },
            Self::Pattern(p) => match p.def {
                PatternDef::Linear { .. } => "LPattern",
                PatternDef::Circular { .. } => "CirPattern",
            },
            Self::Mirror(_) => "Mirror",
            Self::Revolve(r) if r.operation == crate::Operation::Cut => "Cut-Revolve",
            Self::Revolve(_) => "Revolve",
            Self::Blend(b) => match b.kind {
                BlendKind::Fillet => "Fillet",
                BlendKind::Chamfer => "Chamfer",
            },
            Self::Shell(_) => "Shell",
            Self::Draft(_) => "Draft",
            Self::Hole(_) => "Hole",
            Self::Import(_) => "Imported",
            Self::Sweep(s) if s.operation == crate::Operation::Cut => "Cut-Sweep",
            Self::Sweep(_) => "Sweep",
            Self::Loft(l) if l.operation == crate::Operation::Cut => "Cut-Loft",
            Self::Loft(_) => "Loft",
            Self::ConvertToSheet(_) => "Convert-To-Sheet",
        }
    }

    /// Whether the feature changes the bodies (as opposed to sketches and reference geometry).
    pub fn is_solid(&self) -> bool {
        matches!(
            self,
            Self::Extrude(_)
                | Self::BaseFlange(_)
                | Self::EdgeFlange(_)
                | Self::SheetCut(_)
                | Self::Hem(_)
                | Self::SketchedBend(_)
                | Self::Jog(_)
                | Self::MiterFlange(_)
                | Self::Corner(_)
                | Self::Form(_)
                | Self::Pattern(_)
                | Self::Mirror(_)
                | Self::Revolve(_)
                | Self::Blend(_)
                | Self::Shell(_)
                | Self::Draft(_)
                | Self::Hole(_)
                | Self::Import(_)
                | Self::Sweep(_)
                | Self::Loft(_)
                | Self::ConvertToSheet(_)
        )
    }

    /// Whether the feature makes or changes a sheet metal body.
    pub fn is_sheet_metal(&self) -> bool {
        matches!(
            self,
            Self::BaseFlange(_)
                | Self::EdgeFlange(_)
                | Self::SheetCut(_)
                | Self::Hem(_)
                | Self::SketchedBend(_)
                | Self::Jog(_)
                | Self::MiterFlange(_)
                | Self::Corner(_)
                | Self::Form(_)
                | Self::ConvertToSheet(_)
        )
    }

    /// The sketch the feature is made from, if it has one.
    pub fn sketch(&self) -> Option<FeatureId> {
        match self {
            Self::Extrude(e) => Some(e.sketch),
            Self::BaseFlange(b) => Some(b.sketch),
            Self::SheetCut(c) => Some(c.sketch),
            Self::SketchedBend(b) => Some(b.sketch),
            Self::Jog(j) => Some(j.sketch),
            Self::MiterFlange(m) => Some(m.sketch),
            Self::Form(f) => Some(f.sketch),
            Self::Revolve(r) => Some(r.sketch),
            Self::Hole(h) => Some(h.sketch),
            Self::Sweep(s) => Some(s.profile),
            Self::Loft(l) => l.sections.first().copied(),
            _ => None,
        }
    }

    /// Whether a pattern or a mirror can copy the feature.
    pub fn can_be_copied(&self) -> bool {
        matches!(
            self,
            Self::Extrude(_) | Self::SheetCut(_) | Self::Form(_) | Self::Revolve(_) | Self::Hole(_)
        )
    }

    /// The features this one refers to directly, without duplicates.
    pub fn dependencies(&self) -> Vec<FeatureId> {
        let mut out = Vec::new();
        match self {
            Self::Sketch(s) => {
                plane_deps(&s.plane, &mut out);
                for p in &s.projections {
                    match &p.source {
                        crate::projection::Source::Edge(e) => out.extend(e.features()),
                        crate::projection::Source::Point(p) => point_deps(p, &mut out),
                        crate::projection::Source::Plane(p) => plane_deps(p, &mut out),
                    }
                }
            }
            Self::Extrude(e) => {
                out.push(e.sketch);
                if let crate::EndCondition::UpTo(p) = &e.params.end {
                    plane_deps(p, &mut out);
                }
            }
            Self::Plane(def) => match def {
                PlaneDef::Offset { from, .. } => plane_deps(from, &mut out),
                PlaneDef::Angled { from, about, .. } => {
                    plane_deps(from, &mut out);
                    axis_deps(about, &mut out);
                }
                PlaneDef::Midplane { a, b } => {
                    plane_deps(a, &mut out);
                    plane_deps(b, &mut out);
                }
            },
            Self::Axis(def) => match def {
                AxisDef::Edge(e) => out.extend(e.features()),
                AxisDef::Cylinder(f) => out.extend(f.name.features()),
                AxisDef::TwoPlanes(a, b) => {
                    plane_deps(a, &mut out);
                    plane_deps(b, &mut out);
                }
            },
            Self::Point(def) => match def {
                PointDef::Vertex(v) => out.extend(v.features()),
                PointDef::Coordinates { .. } => {}
            },
            Self::CoordSystem(def) => {
                point_deps(&def.origin, &mut out);
                plane_deps(&def.orientation, &mut out);
            }
            Self::BaseFlange(b) => out.push(b.sketch),
            Self::EdgeFlange(e) => {
                if let Some(edge) = &e.edge {
                    out.extend(edge.features());
                }
            }
            Self::SheetCut(c) => out.push(c.sketch),
            Self::Hem(h) => {
                if let Some(edge) = &h.edge {
                    out.extend(edge.features());
                }
            }
            Self::SketchedBend(b) => out.push(b.sketch),
            Self::Jog(j) => out.push(j.sketch),
            Self::MiterFlange(m) => {
                out.push(m.sketch);
                for e in &m.edges {
                    out.extend(e.features());
                }
            }
            Self::Corner(c) => {
                for f in &c.faces {
                    out.extend(f.name.features());
                }
            }
            Self::Form(f) => out.push(f.sketch),
            Self::Pattern(p) => {
                out.extend(&p.seeds);
                match &p.def {
                    PatternDef::Linear { first, second } => {
                        axis_deps(&first.direction, &mut out);
                        if let Some(s) = second {
                            axis_deps(&s.direction, &mut out);
                        }
                    }
                    PatternDef::Circular { axis, .. } => axis_deps(axis, &mut out),
                }
            }
            Self::Mirror(m) => {
                out.extend(&m.seeds);
                plane_deps(&m.plane, &mut out);
            }
            Self::Revolve(r) => {
                out.push(r.sketch);
                if let RevolveAxisRef::Axis(a) = &r.axis {
                    axis_deps(a, &mut out);
                }
            }
            Self::Blend(b) => {
                for e in &b.edges {
                    out.extend(e.features());
                }
            }
            Self::Shell(s) => {
                for f in &s.open {
                    out.extend(f.name.features());
                }
            }
            Self::Draft(d) => {
                for f in &d.faces {
                    out.extend(f.name.features());
                }
                if let Some(p) = &d.neutral {
                    plane_deps(p, &mut out);
                }
            }
            Self::Hole(h) => out.push(h.sketch),
            Self::Import(_) => {}
            Self::Sweep(s) => {
                out.push(s.profile);
                out.extend(s.path);
            }
            Self::Loft(l) => out.extend(&l.sections),
            Self::ConvertToSheet(c) => {
                if let Some(f) = &c.face {
                    out.extend(f.name.features());
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Every value in the feature that can hold an expression.
    pub fn scalars(&self) -> Vec<&Scalar> {
        match self {
            Self::Sketch(_) | Self::Axis(_) | Self::CoordSystem(_) => Vec::new(),
            Self::Extrude(e) => vec![&e.params.depth],
            Self::Plane(PlaneDef::Offset { distance, .. }) => vec![distance],
            Self::Plane(PlaneDef::Angled { angle, .. }) => vec![angle],
            Self::Plane(PlaneDef::Midplane { .. }) => Vec::new(),
            Self::Point(PointDef::Coordinates { x, y, z }) => vec![x, y, z],
            Self::Point(PointDef::Vertex(_)) => Vec::new(),
            Self::BaseFlange(b) => {
                let mut v = b.settings.scalars();
                v.push(&b.depth);
                v
            }
            Self::EdgeFlange(e) => {
                let mut v = vec![&e.length, &e.angle, &e.offset_start, &e.offset_end];
                v.extend(e.radius.as_ref());
                v
            }
            Self::SheetCut(_) | Self::Mirror(_) => Vec::new(),
            Self::Hem(h) => vec![
                &h.length,
                &h.gap,
                &h.radius,
                &h.angle,
                &h.offset_start,
                &h.offset_end,
            ],
            Self::SketchedBend(b) => {
                let mut v = vec![&b.angle];
                v.extend(b.radius.as_ref());
                v
            }
            Self::Jog(j) => {
                let mut v = vec![&j.offset, &j.angle];
                v.extend(j.radius.as_ref());
                v
            }
            Self::MiterFlange(m) => vec![&m.gap, &m.offset_start, &m.offset_end],
            Self::Corner(c) => vec![&c.gap, &c.relief_size],
            Self::Form(f) => vec![&f.height],
            Self::Pattern(p) => match &p.def {
                PatternDef::Linear { first, second } => {
                    let mut v = vec![&first.spacing, &first.count];
                    if let Some(s) = second {
                        v.extend([&s.spacing, &s.count]);
                    }
                    v
                }
                PatternDef::Circular { angle, count, .. } => vec![angle, count],
            },
            Self::Revolve(r) => vec![&r.angle],
            Self::Blend(b) => vec![&b.size],
            Self::Shell(s) => vec![&s.thickness],
            Self::Draft(d) => vec![&d.angle],
            Self::Hole(h) => vec![
                &h.diameter,
                &h.depth,
                &h.tip_angle,
                &h.counterbore_diameter,
                &h.counterbore_depth,
                &h.countersink_diameter,
                &h.countersink_angle,
            ],
            Self::Import(_) | Self::Sweep(_) | Self::Loft(_) => Vec::new(),
            Self::ConvertToSheet(c) => c.scalars(),
        }
    }
}

/// Writes [`FeatureKind::slots`] and [`FeatureKind::slots_mut`] from one list, so the two
/// can't differ.
macro_rules! slots {
    ($(#[$doc:meta])* $name:ident $(, $m:tt)?) => {
        $(#[$doc])*
        pub fn $name(& $($m)? self) -> Vec<(&'static str, ScalarKind, & $($m)? Scalar)> {
            use ScalarKind::{Angle, Length, Number};
            /// A bend model's value, named after the model.
            macro_rules! bend {
                ($model:expr) => {
                    match $model {
                        BendModelDef::KFactor(s) => ("k_factor", Number, s),
                        BendModelDef::Allowance(s) => ("allowance", Length, s),
                        BendModelDef::Deduction(s) => ("deduction", Length, s),
                    }
                };
            }
            match self {
                Self::Extrude(e) => vec![("depth", Length, & $($m)? e.params.depth)],
                Self::Plane(PlaneDef::Offset { distance, .. }) => {
                    vec![("distance", Length, distance)]
                }
                Self::Plane(PlaneDef::Angled { angle, .. }) => vec![("angle", Angle, angle)],
                Self::Point(PointDef::Coordinates { x, y, z }) => {
                    vec![("x", Length, x), ("y", Length, y), ("z", Length, z)]
                }
                Self::BaseFlange(b) => {
                    let b = & $($m)? **b;
                    vec![
                        ("thickness", Length, & $($m)? b.settings.thickness),
                        ("radius", Length, & $($m)? b.settings.radius),
                        bend!(& $($m)? b.settings.model),
                        ("relief_ratio", Number, & $($m)? b.settings.relief_ratio),
                        ("depth", Length, & $($m)? b.depth),
                    ]
                }
                Self::EdgeFlange(e) => {
                    let e = & $($m)? **e;
                    let mut v = vec![
                        ("length", Length, & $($m)? e.length),
                        ("angle", Angle, & $($m)? e.angle),
                        ("offset_start", Length, & $($m)? e.offset_start),
                        ("offset_end", Length, & $($m)? e.offset_end),
                    ];
                    if let Some(r) = & $($m)? e.radius {
                        v.push(("radius", Length, r));
                    }
                    v
                }
                Self::Hem(h) => {
                    let h = & $($m)? **h;
                    vec![
                        ("length", Length, & $($m)? h.length),
                        ("gap", Length, & $($m)? h.gap),
                        ("radius", Length, & $($m)? h.radius),
                        ("angle", Angle, & $($m)? h.angle),
                        ("offset_start", Length, & $($m)? h.offset_start),
                        ("offset_end", Length, & $($m)? h.offset_end),
                    ]
                }
                Self::SketchedBend(b) => {
                    let b = & $($m)? **b;
                    let mut v = vec![("angle", Angle, & $($m)? b.angle)];
                    if let Some(r) = & $($m)? b.radius {
                        v.push(("radius", Length, r));
                    }
                    v
                }
                Self::Jog(j) => {
                    let j = & $($m)? **j;
                    let mut v = vec![
                        ("offset", Length, & $($m)? j.offset),
                        ("angle", Angle, & $($m)? j.angle),
                    ];
                    if let Some(r) = & $($m)? j.radius {
                        v.push(("radius", Length, r));
                    }
                    v
                }
                Self::MiterFlange(f) => {
                    let f = & $($m)? **f;
                    vec![
                        ("gap", Length, & $($m)? f.gap),
                        ("offset_start", Length, & $($m)? f.offset_start),
                        ("offset_end", Length, & $($m)? f.offset_end),
                    ]
                }
                Self::Corner(c) => {
                    let c = & $($m)? **c;
                    vec![
                        ("gap", Length, & $($m)? c.gap),
                        ("relief_size", Length, & $($m)? c.relief_size),
                    ]
                }
                Self::Form(f) => vec![("height", Length, & $($m)? f.height)],
                Self::Pattern(p) => match & $($m)? p.def {
                    PatternDef::Linear { first, second } => {
                        let mut v = vec![("spacing", Length, & $($m)? first.spacing), ("count", Number, & $($m)? first.count)];
                        if let Some(s) = second {
                            v.push(("spacing2", Length, & $($m)? s.spacing));
                            v.push(("count2", Number, & $($m)? s.count));
                        }
                        v
                    }
                    PatternDef::Circular { angle, count, .. } => vec![("angle", Angle, angle), ("count", Number, count)],
                },
                Self::Revolve(r) => vec![("angle", Angle, & $($m)? r.angle)],
                Self::Blend(b) => vec![("size", Length, & $($m)? b.size)],
                Self::Shell(s) => vec![("thickness", Length, & $($m)? s.thickness)],
                Self::Draft(d) => vec![("angle", Angle, & $($m)? d.angle)],
                Self::Hole(h) => {
                    let h = & $($m)? **h;
                    vec![
                        ("diameter", Length, & $($m)? h.diameter),
                        ("depth", Length, & $($m)? h.depth),
                        ("tip_angle", Angle, & $($m)? h.tip_angle),
                        ("counterbore_diameter", Length, & $($m)? h.counterbore_diameter),
                        ("counterbore_depth", Length, & $($m)? h.counterbore_depth),
                        ("countersink_diameter", Length, & $($m)? h.countersink_diameter),
                        ("countersink_angle", Angle, & $($m)? h.countersink_angle),
                    ]
                }
                Self::ConvertToSheet(c) => {
                    let c = & $($m)? **c;
                    vec![
                        bend!(& $($m)? c.model),
                        ("relief_ratio", Number, & $($m)? c.relief_ratio),
                    ]
                }
                Self::Sketch(_)
                | Self::Axis(_)
                | Self::CoordSystem(_)
                | Self::Plane(PlaneDef::Midplane { .. })
                | Self::Point(PointDef::Vertex(_))
                | Self::SheetCut(_)
                | Self::Mirror(_)
                | Self::Import(_)
                | Self::Sweep(_)
                | Self::Loft(_) => Vec::new(),
            }
        }
    };
}

impl FeatureKind {
    slots! {
        /// The feature's numeric values (those of [`FeatureKind::scalars`]), each with
        /// the name its operations give it and what it measures. These are the values
        /// that can differ between configurations (see [`crate::config`]). A bend model's
        /// value goes by the model's name (`k_factor`, `allowance`, `deduction`).
        slots
    }
    slots! {
        /// [`FeatureKind::slots`], for changing the values.
        slots_mut, mut
    }
}

fn plane_deps(r: &PlaneRef, out: &mut Vec<FeatureId>) {
    match r {
        PlaneRef::Standard(_) => {}
        PlaneRef::Feature(id) => out.push(*id),
        PlaneRef::Face(f) => out.extend(f.name.features()),
    }
}

fn axis_deps(r: &AxisRef, out: &mut Vec<FeatureId>) {
    match r {
        AxisRef::Standard(_) => {}
        AxisRef::Feature(id) => out.push(*id),
        AxisRef::Edge(e) => out.extend(e.features()),
    }
}

fn point_deps(r: &PointRef, out: &mut Vec<FeatureId>) {
    match r {
        PointRef::Origin => {}
        PointRef::Feature(id) => out.push(*id),
        PointRef::Vertex(v) => out.extend(v.features()),
    }
}

/// Frozen layouts used when migrating model schemas through 9.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct LinearDirectionV9 {
    direction: AxisRef,
    spacing: Scalar,
    count: u32,
    flip: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
enum PatternDefV9 {
    Linear {
        first: LinearDirectionV9,
        second: Option<LinearDirectionV9>,
    },
    Circular {
        axis: AxisRef,
        count: u32,
        angle: Scalar,
        flip: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct PatternFeatureV9 {
    seeds: Vec<FeatureId>,
    def: PatternDefV9,
}

impl From<LinearDirectionV9> for LinearDirection {
    fn from(d: LinearDirectionV9) -> Self {
        Self {
            direction: d.direction,
            spacing: d.spacing,
            count: Scalar::new(f64::from(d.count)),
            flip: d.flip,
        }
    }
}

impl From<LinearDirection> for LinearDirectionV9 {
    fn from(d: LinearDirection) -> Self {
        Self {
            direction: d.direction,
            spacing: d.spacing,
            count: d.count.value as u32,
            flip: d.flip,
        }
    }
}

impl From<PatternFeatureV9> for PatternFeature {
    fn from(p: PatternFeatureV9) -> Self {
        let def = match p.def {
            PatternDefV9::Linear { first, second } => PatternDef::Linear {
                first: first.into(),
                second: second.map(Into::into),
            },
            PatternDefV9::Circular {
                axis,
                count,
                angle,
                flip,
            } => PatternDef::Circular {
                axis,
                count: Scalar::new(f64::from(count)),
                angle,
                flip,
            },
        };
        Self {
            seeds: p.seeds,
            def,
        }
    }
}

impl From<PatternFeature> for PatternFeatureV9 {
    fn from(p: PatternFeature) -> Self {
        let def = match p.def {
            PatternDef::Linear { first, second } => PatternDefV9::Linear {
                first: first.into(),
                second: second.map(Into::into),
            },
            PatternDef::Circular {
                axis,
                count,
                angle,
                flip,
            } => PatternDefV9::Circular {
                axis,
                count: count.value as u32,
                angle,
                flip,
            },
        };
        Self {
            seeds: p.seeds,
            def,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum FeatureKindV9 {
    Sketch(Box<SketchFeature>),
    Extrude(Box<ExtrudeFeature>),
    Plane(PlaneDef),
    Axis(AxisDef),
    Point(PointDef),
    CoordSystem(CoordSystemDef),
    BaseFlange(Box<BaseFlangeFeature>),
    EdgeFlange(Box<EdgeFlangeFeature>),
    SheetCut(Box<SheetCutFeature>),
    Hem(Box<HemFeature>),
    SketchedBend(Box<SketchedBendFeature>),
    Jog(Box<JogFeature>),
    MiterFlange(Box<MiterFlangeFeature>),
    Corner(Box<CornerFeature>),
    Form(Box<FormFeature>),
    Pattern(Box<PatternFeatureV9>),
    Mirror(Box<MirrorFeature>),
    // The variant order and the pattern layout are frozen for schemas through 9.
    Revolve(Box<RevolveFeature>),
    Blend(Box<BlendFeature>),
    Shell(Box<ShellFeature>),
    Draft(Box<DraftFeature>),
    Hole(Box<HoleFeature>),
    Import(Box<ImportFeature>),
    Sweep(Box<SweepFeature>),
    Loft(Box<LoftFeature>),
    ConvertToSheet(Box<ConvertToSheetFeature>),
}
impl From<FeatureKindV9> for FeatureKind {
    fn from(k: FeatureKindV9) -> Self {
        match k {
            FeatureKindV9::Sketch(v) => Self::Sketch(v),
            FeatureKindV9::Extrude(v) => Self::Extrude(v),
            FeatureKindV9::Plane(v) => Self::Plane(v),
            FeatureKindV9::Axis(v) => Self::Axis(v),
            FeatureKindV9::Point(v) => Self::Point(v),
            FeatureKindV9::CoordSystem(v) => Self::CoordSystem(v),
            FeatureKindV9::BaseFlange(v) => Self::BaseFlange(v),
            FeatureKindV9::EdgeFlange(v) => Self::EdgeFlange(v),
            FeatureKindV9::SheetCut(v) => Self::SheetCut(v),
            FeatureKindV9::Hem(v) => Self::Hem(v),
            FeatureKindV9::SketchedBend(v) => Self::SketchedBend(v),
            FeatureKindV9::Jog(v) => Self::Jog(v),
            FeatureKindV9::MiterFlange(v) => Self::MiterFlange(v),
            FeatureKindV9::Corner(v) => Self::Corner(v),
            FeatureKindV9::Form(v) => Self::Form(v),
            FeatureKindV9::Pattern(v) => Self::Pattern(Box::new((*v).into())),
            FeatureKindV9::Mirror(v) => Self::Mirror(v),
            FeatureKindV9::Revolve(v) => Self::Revolve(v),
            FeatureKindV9::Blend(v) => Self::Blend(v),
            FeatureKindV9::Shell(v) => Self::Shell(v),
            FeatureKindV9::Draft(v) => Self::Draft(v),
            FeatureKindV9::Hole(v) => Self::Hole(v),
            FeatureKindV9::Import(v) => Self::Import(v),
            FeatureKindV9::Sweep(v) => Self::Sweep(v),
            FeatureKindV9::Loft(v) => Self::Loft(v),
            FeatureKindV9::ConvertToSheet(v) => Self::ConvertToSheet(v),
        }
    }
}

impl From<FeatureKind> for FeatureKindV9 {
    fn from(k: FeatureKind) -> Self {
        match k {
            FeatureKind::Sketch(v) => Self::Sketch(v),
            FeatureKind::Extrude(v) => Self::Extrude(v),
            FeatureKind::Plane(v) => Self::Plane(v),
            FeatureKind::Axis(v) => Self::Axis(v),
            FeatureKind::Point(v) => Self::Point(v),
            FeatureKind::CoordSystem(v) => Self::CoordSystem(v),
            FeatureKind::BaseFlange(v) => Self::BaseFlange(v),
            FeatureKind::EdgeFlange(v) => Self::EdgeFlange(v),
            FeatureKind::SheetCut(v) => Self::SheetCut(v),
            FeatureKind::Hem(v) => Self::Hem(v),
            FeatureKind::SketchedBend(v) => Self::SketchedBend(v),
            FeatureKind::Jog(v) => Self::Jog(v),
            FeatureKind::MiterFlange(v) => Self::MiterFlange(v),
            FeatureKind::Corner(v) => Self::Corner(v),
            FeatureKind::Form(v) => Self::Form(v),
            FeatureKind::Pattern(v) => Self::Pattern(Box::new((*v).into())),
            FeatureKind::Mirror(v) => Self::Mirror(v),
            FeatureKind::Revolve(v) => Self::Revolve(v),
            FeatureKind::Blend(v) => Self::Blend(v),
            FeatureKind::Shell(v) => Self::Shell(v),
            FeatureKind::Draft(v) => Self::Draft(v),
            FeatureKind::Hole(v) => Self::Hole(v),
            FeatureKind::Import(v) => Self::Import(v),
            FeatureKind::Sweep(v) => Self::Sweep(v),
            FeatureKind::Loft(v) => Self::Loft(v),
            FeatureKind::ConvertToSheet(v) => Self::ConvertToSheet(v),
        }
    }
}

/// A feature as schema 9 stored it: sketch projections, with literal pattern counts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeatureV9 {
    id: FeatureId,
    name: String,
    suppressed: bool,
    visible: bool,
    kind: FeatureKindV9,
}

impl From<Feature> for FeatureV9 {
    fn from(f: Feature) -> Self {
        Self {
            id: f.id,
            name: f.name,
            suppressed: f.suppressed,
            visible: f.visible,
            kind: f.kind.into(),
        }
    }
}

impl From<FeatureV9> for Feature {
    fn from(f: FeatureV9) -> Self {
        Self {
            suppression_expression: None,
            id: f.id,
            name: f.name,
            suppressed: f.suppressed,
            visible: f.visible,
            kind: f.kind.into(),
        }
    }
}

/// One step of the part's history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub id: FeatureId,
    pub name: String,
    /// Suppressed features are skipped when rebuilding, as if they were not there.
    pub suppressed: bool,
    /// Part-wide rule evaluated with the active configuration's parameters. While set,
    /// this controls suppression instead of the saved manual flag. Zero builds the
    /// feature; a finite nonzero plain number suppresses it.
    #[serde(default)]
    pub suppression_expression: Option<String>,
    /// Whether the feature's own geometry (a sketch, a reference plane) is drawn.
    pub visible: bool,
    pub kind: FeatureKind,
}

impl Feature {
    /// Effective suppression, without changing the saved per-configuration flag.
    pub fn effective_suppression(
        &self,
        parameters: &peet_sketch::expr::Parameters,
    ) -> Result<bool, String> {
        let Some(source) = &self.suppression_expression else {
            return Ok(self.suppressed);
        };
        let q = parameters
            .evaluate_expression(source)
            .map_err(|e| format!("Suppression expression: {e}"))?;
        if !q.dim.is_none() || !q.value.is_finite() {
            return Err("Suppression expression must return a finite plain number: zero builds, nonzero suppresses.".to_owned());
        }
        Ok(q.value != 0.0)
    }

    pub fn sketch(&self) -> Option<&SketchFeature> {
        match &self.kind {
            FeatureKind::Sketch(s) => Some(s),
            _ => None,
        }
    }

    pub fn sketch_mut(&mut self) -> Option<&mut SketchFeature> {
        match &mut self.kind {
            FeatureKind::Sketch(s) => Some(s),
            _ => None,
        }
    }

    pub fn extrude(&self) -> Option<&ExtrudeFeature> {
        match &self.kind {
            FeatureKind::Extrude(e) => Some(e),
            _ => None,
        }
    }

    pub fn extrude_mut(&mut self) -> Option<&mut ExtrudeFeature> {
        match &mut self.kind {
            FeatureKind::Extrude(e) => Some(e),
            _ => None,
        }
    }
}

// Binary layouts of sketch features before model schema 9.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SketchFeatureV8 {
    plane: PlaneRef,
    placement: Plane,
    sketch: Sketch,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum FeatureKindV8 {
    Sketch(Box<SketchFeatureV8>),
    Extrude(Box<ExtrudeFeature>),
    Plane(PlaneDef),
    Axis(AxisDef),
    Point(PointDef),
    CoordSystem(CoordSystemDef),
    BaseFlange(Box<BaseFlangeFeature>),
    EdgeFlange(Box<EdgeFlangeFeature>),
    SheetCut(Box<SheetCutFeature>),
    Hem(Box<HemFeature>),
    SketchedBend(Box<SketchedBendFeature>),
    Jog(Box<JogFeature>),
    MiterFlange(Box<MiterFlangeFeature>),
    Corner(Box<CornerFeature>),
    Form(Box<FormFeature>),
    Pattern(Box<PatternFeatureV9>),
    Mirror(Box<MirrorFeature>),
    Revolve(Box<RevolveFeature>),
    Blend(Box<BlendFeature>),
    Shell(Box<ShellFeature>),
    Draft(Box<DraftFeature>),
    Hole(Box<HoleFeature>),
    Import(Box<ImportFeature>),
    Sweep(Box<SweepFeature>),
    Loft(Box<LoftFeature>),
    ConvertToSheet(Box<ConvertToSheetFeature>),
}
impl From<FeatureKindV8> for FeatureKind {
    fn from(kind: FeatureKindV8) -> Self {
        match kind {
            FeatureKindV8::Sketch(s) => Self::Sketch(Box::new(SketchFeature {
                plane: s.plane,
                placement: s.placement,
                sketch: s.sketch,
                projections: Vec::new(),
            })),
            FeatureKindV8::Extrude(v) => Self::Extrude(v),
            FeatureKindV8::Plane(v) => Self::Plane(v),
            FeatureKindV8::Axis(v) => Self::Axis(v),
            FeatureKindV8::Point(v) => Self::Point(v),
            FeatureKindV8::CoordSystem(v) => Self::CoordSystem(v),
            FeatureKindV8::BaseFlange(v) => Self::BaseFlange(v),
            FeatureKindV8::EdgeFlange(v) => Self::EdgeFlange(v),
            FeatureKindV8::SheetCut(v) => Self::SheetCut(v),
            FeatureKindV8::Hem(v) => Self::Hem(v),
            FeatureKindV8::SketchedBend(v) => Self::SketchedBend(v),
            FeatureKindV8::Jog(v) => Self::Jog(v),
            FeatureKindV8::MiterFlange(v) => Self::MiterFlange(v),
            FeatureKindV8::Corner(v) => Self::Corner(v),
            FeatureKindV8::Form(v) => Self::Form(v),
            FeatureKindV8::Pattern(v) => Self::Pattern(Box::new((*v).into())),
            FeatureKindV8::Mirror(v) => Self::Mirror(v),
            FeatureKindV8::Revolve(v) => Self::Revolve(v),
            FeatureKindV8::Blend(v) => Self::Blend(v),
            FeatureKindV8::Shell(v) => Self::Shell(v),
            FeatureKindV8::Draft(v) => Self::Draft(v),
            FeatureKindV8::Hole(v) => Self::Hole(v),
            FeatureKindV8::Import(v) => Self::Import(v),
            FeatureKindV8::Sweep(v) => Self::Sweep(v),
            FeatureKindV8::Loft(v) => Self::Loft(v),
            FeatureKindV8::ConvertToSheet(v) => Self::ConvertToSheet(v),
        }
    }
}
impl From<&FeatureKind> for FeatureKindV8 {
    fn from(kind: &FeatureKind) -> Self {
        match kind {
            FeatureKind::Sketch(s) => Self::Sketch(Box::new(SketchFeatureV8 {
                plane: s.plane.clone(),
                placement: s.placement,
                sketch: s.sketch.clone(),
            })),
            FeatureKind::Extrude(v) => Self::Extrude(v.clone()),
            FeatureKind::Plane(v) => Self::Plane(v.clone()),
            FeatureKind::Axis(v) => Self::Axis(v.clone()),
            FeatureKind::Point(v) => Self::Point(v.clone()),
            FeatureKind::CoordSystem(v) => Self::CoordSystem(v.clone()),
            FeatureKind::BaseFlange(v) => Self::BaseFlange(v.clone()),
            FeatureKind::EdgeFlange(v) => Self::EdgeFlange(v.clone()),
            FeatureKind::SheetCut(v) => Self::SheetCut(v.clone()),
            FeatureKind::Hem(v) => Self::Hem(v.clone()),
            FeatureKind::SketchedBend(v) => Self::SketchedBend(v.clone()),
            FeatureKind::Jog(v) => Self::Jog(v.clone()),
            FeatureKind::MiterFlange(v) => Self::MiterFlange(v.clone()),
            FeatureKind::Corner(v) => Self::Corner(v.clone()),
            FeatureKind::Form(v) => Self::Form(v.clone()),
            FeatureKind::Pattern(v) => Self::Pattern(Box::new((**v).clone().into())),
            FeatureKind::Mirror(v) => Self::Mirror(v.clone()),
            FeatureKind::Revolve(v) => Self::Revolve(v.clone()),
            FeatureKind::Blend(v) => Self::Blend(v.clone()),
            FeatureKind::Shell(v) => Self::Shell(v.clone()),
            FeatureKind::Draft(v) => Self::Draft(v.clone()),
            FeatureKind::Hole(v) => Self::Hole(v.clone()),
            FeatureKind::Import(v) => Self::Import(v.clone()),
            FeatureKind::Sweep(v) => Self::Sweep(v.clone()),
            FeatureKind::Loft(v) => Self::Loft(v.clone()),
            FeatureKind::ConvertToSheet(v) => Self::ConvertToSheet(v.clone()),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeatureV8 {
    id: FeatureId,
    name: String,
    suppressed: bool,
    visible: bool,
    kind: FeatureKindV8,
}
impl From<FeatureV8> for Feature {
    fn from(f: FeatureV8) -> Self {
        Self {
            suppression_expression: None,
            id: f.id,
            name: f.name,
            suppressed: f.suppressed,
            visible: f.visible,
            kind: f.kind.into(),
        }
    }
}
impl From<&Feature> for FeatureV8 {
    fn from(f: &Feature) -> Self {
        Self {
            id: f.id,
            name: f.name.clone(),
            suppressed: f.suppressed,
            visible: f.visible,
            kind: (&f.kind).into(),
        }
    }
}

impl FeatureV8 {
    pub(crate) fn of(f: &Feature) -> Self {
        Self::from(f)
    }
}
impl From<Feature> for FeatureV8 {
    fn from(f: Feature) -> Self {
        Self::from(&f)
    }
}
