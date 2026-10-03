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

use crate::extrude::Extrude;
use crate::naming::{EdgeRef, FaceRef, VertexRef};

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

/// One direction of a linear pattern.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinearDirection {
    pub direction: AxisRef,
    pub spacing: Scalar,
    /// How many, the original included.
    pub count: u32,
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
        count: u32,
        angle: Scalar,
        flip: bool,
    },
}

/// Copies of features (extrusions, cuts, sheet metal cuts, forms) in a pattern.
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
            _ => None,
        }
    }

    /// Whether a pattern or a mirror can copy the feature.
    pub fn can_be_copied(&self) -> bool {
        matches!(self, Self::Extrude(_) | Self::SheetCut(_) | Self::Form(_))
    }

    /// The features this one refers to directly, without duplicates.
    pub fn dependencies(&self) -> Vec<FeatureId> {
        let mut out = Vec::new();
        match self {
            Self::Sketch(s) => plane_deps(&s.plane, &mut out),
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
                    let mut v = vec![&first.spacing];
                    v.extend(second.as_ref().map(|s| &s.spacing));
                    v
                }
                PatternDef::Circular { angle, .. } => vec![angle],
            },
        }
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

/// One step of the part's history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub id: FeatureId,
    pub name: String,
    /// Suppressed features are skipped when rebuilding, as if they were not there.
    pub suppressed: bool,
    /// Whether the feature's own geometry (a sketch, a reference plane) is drawn.
    pub visible: bool,
    pub kind: FeatureKind,
}

impl Feature {
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
