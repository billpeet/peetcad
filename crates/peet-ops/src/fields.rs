//! The fields of every kind of feature, described once.
//!
//! Each `feature_args!` table below names a feature's scriptable fields, what kind of
//! value each takes and where it is stored. From that one table come:
//!
//! - the typed arguments (`EdgeFlange { length: Some("flange".into()), .. }`), so a wrong
//!   field name or type in Rust is a compile error;
//! - reading them from a JSON operation, creating the feature, editing it, reading it
//!   back and listing its fields in `help`.
//!
//! Every table destructures its feature exhaustively, so a field added to a feature
//! doesn't compile until it is listed here (or explicitly left out with `_`).

use std::marker::PhantomData;

use peet_document::Document;
use peet_model::feature::{AxisDef, CoordSystemDef, FeatureKind, PatternDef, PlaneDef, PointDef};
use peet_model::{
    AxisRef, BaseFlangeFeature, BendModelDef, CornerFeature, EdgeFlangeFeature, EdgeRef,
    EndCondition, ExtrudeFeature, FaceName, FaceRef, FeatureId, FormFeature, HemFeature,
    JogFeature, LinearDirection, MirrorFeature, MiterFlangeFeature, Operation, PatternFeature,
    PlaneRef, PointRef, RegionSelection, Scalar, ScalarKind, SheetCutFeature, SheetSettingsDef,
    SketchFeature, SketchedBendFeature, StdAxis, StdPlane, VertexRef,
};
use peet_sheetmetal::corner::{CornerKind, CornerRelief};
use peet_sheetmetal::{
    BendLinePosition, FlangePosition, FormKind, HemKind, JogDimension, ReliefType,
};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::args::{
    Args, boolean, integer, length_out, list, mm2, point2_out, point3_out, round, text,
};
use crate::select;
use crate::value::{
    AxisSel, Bend, EdgeSel, End, FaceSel, FeatureSel, Input, PlaneSel, PointSel, Regions, VertexSel,
};

// ---- Kinds of field ----

/// A kind of field: how its value is written, stored and read back.
pub(crate) trait Field {
    /// The value in an operation.
    type Arg;
    /// The value in the feature.
    type Target;
    /// Whether a new feature can't do without it (it has no sensible default).
    const REQUIRED: bool = false;
    /// Whether `null` is a value (it means "the default").
    const NULLABLE: bool = false;
    /// What the field takes, for `help`.
    fn describe() -> String;
    fn parse(v: &Value) -> Result<Self::Arg, String>;
    fn assign(arg: &Self::Arg, target: &mut Self::Target, doc: &Document) -> Result<(), String>;
    fn read(target: &Self::Target, doc: &Document) -> Value;
}

fn scalar_out(s: &Scalar, kind: ScalarKind, doc: &Document) -> Value {
    let params = &doc.model.parameters;
    let value = s.evaluate(kind, params).unwrap_or(s.value);
    let value = match kind {
        ScalarKind::Length => length_out(value, &params.units),
        ScalarKind::Angle | ScalarKind::Number => Value::from(round(value)),
    };
    match &s.expression {
        Some(e) => json!({ "expression": e, "value": value }),
        None => value,
    }
}

macro_rules! scalar_field {
    ($name:ident, $kind:expr, $text:literal) => {
        pub(crate) struct $name;
        impl Field for $name {
            type Arg = Input;
            type Target = Scalar;
            fn describe() -> String {
                $text.to_owned()
            }
            fn parse(v: &Value) -> Result<Input, String> {
                Input::parse(v)
            }
            fn assign(arg: &Input, target: &mut Scalar, doc: &Document) -> Result<(), String> {
                *target = arg.scalar($kind, doc)?;
                Ok(())
            }
            fn read(target: &Scalar, doc: &Document) -> Value {
                scalar_out(target, $kind, doc)
            }
        }
    };
}

scalar_field!(Length, ScalarKind::Length, "length");
scalar_field!(Angle, ScalarKind::Angle, "angle in degrees");
scalar_field!(Number, ScalarKind::Number, "number");

/// A length that falls back to the body's default when absent.
pub(crate) struct OptLength;

impl Field for OptLength {
    type Arg = Option<Input>;
    type Target = Option<Scalar>;
    const NULLABLE: bool = true;
    fn describe() -> String {
        "length, or null for the body's default".to_owned()
    }
    fn parse(v: &Value) -> Result<Self::Arg, String> {
        if v.is_null() {
            Ok(None)
        } else {
            Input::parse(v).map(Some)
        }
    }
    fn assign(arg: &Self::Arg, target: &mut Self::Target, doc: &Document) -> Result<(), String> {
        *target = arg
            .as_ref()
            .map(|a| a.scalar(ScalarKind::Length, doc))
            .transpose()?;
        Ok(())
    }
    fn read(target: &Self::Target, doc: &Document) -> Value {
        target
            .as_ref()
            .map_or(Value::Null, |s| scalar_out(s, ScalarKind::Length, doc))
    }
}

pub(crate) struct Flag;

impl Field for Flag {
    type Arg = bool;
    type Target = bool;
    fn describe() -> String {
        "true or false".to_owned()
    }
    fn parse(v: &Value) -> Result<bool, String> {
        boolean(v)
    }
    fn assign(arg: &bool, target: &mut bool, _: &Document) -> Result<(), String> {
        *target = *arg;
        Ok(())
    }
    fn read(target: &bool, _: &Document) -> Value {
        json!(*target)
    }
}

pub(crate) struct Count;

impl Field for Count {
    type Arg = u32;
    type Target = u32;
    fn describe() -> String {
        "whole number".to_owned()
    }
    fn parse(v: &Value) -> Result<u32, String> {
        integer(v)
    }
    fn assign(arg: &u32, target: &mut u32, _: &Document) -> Result<(), String> {
        *target = *arg;
        Ok(())
    }
    fn read(target: &u32, _: &Document) -> Value {
        json!(*target)
    }
}

/// An enum whose values are written as words (`MaterialInside` as `material_inside`).
pub(crate) trait Words: Sized + Clone + Serialize + 'static {
    fn all() -> &'static [Self];

    fn word(&self) -> String {
        let name = match serde_json::to_value(self) {
            Ok(Value::String(s)) => s,
            _ => String::new(),
        };
        let mut out = String::new();
        for c in name.chars() {
            if c.is_uppercase() && !out.is_empty() {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        }
        out
    }

    fn words() -> Vec<String> {
        Self::all().iter().map(Self::word).collect()
    }
}

macro_rules! words {
    ($($t:ty),*) => { $( impl Words for $t { fn all() -> &'static [Self] { &<$t>::ALL } } )* };
}

words!(
    ReliefType,
    FlangePosition,
    HemKind,
    BendLinePosition,
    JogDimension,
    CornerKind,
    CornerRelief,
    FormKind
);

impl Words for Operation {
    fn all() -> &'static [Self] {
        &[Operation::Add, Operation::NewBody, Operation::Cut]
    }
}

/// One of a fixed set of words.
pub(crate) struct Choice<T>(PhantomData<T>);

impl<T: Words> Field for Choice<T> {
    type Arg = T;
    type Target = T;
    fn describe() -> String {
        T::words().join(" | ")
    }
    fn parse(v: &Value) -> Result<T, String> {
        let w = text(v)?;
        T::all()
            .iter()
            .find(|o| o.word() == w)
            .cloned()
            .ok_or_else(|| format!("'{w}' is not one of {}", T::words().join(", ")))
    }
    fn assign(arg: &T, target: &mut T, _: &Document) -> Result<(), String> {
        *target = arg.clone();
        Ok(())
    }
    fn read(target: &T, _: &Document) -> Value {
        json!(target.word())
    }
}

fn face_out(r: &FaceRef, doc: &Document) -> Value {
    json!({
        "face": doc.model.describe_face(&r.name),
        "at": point3_out(r.hint, &doc.model.parameters.units),
    })
}

fn edge_out(r: &EdgeRef, doc: &Document) -> Value {
    json!({
        "edge": format!(
            "where {} meets {}",
            doc.model.describe_face(&r.faces[0]),
            doc.model.describe_face(&r.faces[1])
        ),
        "at": point3_out(r.hint, &doc.model.parameters.units),
    })
}

fn plane_out(r: &PlaneRef, doc: &Document) -> Value {
    match r {
        PlaneRef::Standard(StdPlane::Front) => json!("front"),
        PlaneRef::Standard(StdPlane::Top) => json!("top"),
        PlaneRef::Standard(StdPlane::Right) => json!("right"),
        PlaneRef::Feature(id) => json!(doc.model.name_of(*id)),
        PlaneRef::Face(f) => face_out(f, doc),
    }
}

fn axis_out(a: &AxisRef, doc: &Document) -> Value {
    match a {
        AxisRef::Standard(StdAxis::X) => json!("x"),
        AxisRef::Standard(StdAxis::Y) => json!("y"),
        AxisRef::Standard(StdAxis::Z) => json!("z"),
        AxisRef::Feature(id) => json!(doc.model.name_of(*id)),
        AxisRef::Edge(e) => edge_out(e, doc),
    }
}

fn vertex_out(v: &VertexRef, doc: &Document) -> Value {
    json!({ "vertex": point3_out(v.hint, &doc.model.parameters.units) })
}

fn each<T, U>(items: &[T], f: impl Fn(&T) -> Result<U, String>) -> Result<Vec<U>, String> {
    items.iter().map(f).collect()
}

/// A reference field: its selector, what it resolves to, and the words for it.
macro_rules! reference_field {
    ($name:ident, $arg:ty, $target:ty, $required:literal, $text:literal,
     parse: $parse:expr, assign: $assign:expr, read: $read:expr) => {
        pub(crate) struct $name;
        impl Field for $name {
            type Arg = $arg;
            type Target = $target;
            const REQUIRED: bool = $required;
            fn describe() -> String {
                $text.to_owned()
            }
            fn parse(v: &Value) -> Result<Self::Arg, String> {
                let parse: fn(&Value) -> Result<$arg, String> = $parse;
                parse(v)
            }
            fn assign(arg: &$arg, target: &mut $target, doc: &Document) -> Result<(), String> {
                let assign: fn(&$arg, &Document) -> Result<$target, String> = $assign;
                *target = assign(arg, doc)?;
                Ok(())
            }
            fn read(target: &$target, doc: &Document) -> Value {
                let read: fn(&$target, &Document) -> Value = $read;
                read(target, doc)
            }
        }
    };
}

reference_field!(PlaneF, PlaneSel, PlaneRef, true, "plane",
    parse: PlaneSel::parse,
    assign: |a, doc| Ok(select::plane(doc, a)?.0),
    read: plane_out);
reference_field!(AxisF, AxisSel, AxisRef, true, "axis",
    parse: AxisSel::parse,
    assign: |a, doc| select::axis(doc, a),
    read: axis_out);
reference_field!(PointF, PointSel, PointRef, true, "point",
parse: PointSel::parse,
assign: |a, doc| select::point(doc, a),
read: |p, doc| match p {
    PointRef::Origin => json!("origin"),
    PointRef::Feature(id) => json!(doc.model.name_of(*id)),
    PointRef::Vertex(v) => vertex_out(v, doc),
});
reference_field!(FaceF, FaceSel, FaceRef, true, "face",
    parse: FaceSel::parse,
    assign: |a, doc| select::face_ref(doc, a),
    read: face_out);
reference_field!(Faces, Vec<FaceSel>, Vec<FaceRef>, false, "list of faces",
    parse: |v| each(list(v)?, FaceSel::parse),
    assign: |a, doc| each(a, |f| select::face_ref(doc, f)),
    read: |f, doc| f.iter().map(|f| face_out(f, doc)).collect());
reference_field!(EdgeF, EdgeSel, EdgeRef, true, "edge",
    parse: EdgeSel::parse,
    assign: |a, doc| select::edge_ref(doc, a),
    read: edge_out);
reference_field!(OptEdge, EdgeSel, Option<EdgeRef>, true, "edge",
    parse: EdgeSel::parse,
    assign: |a, doc| select::edge_ref(doc, a).map(Some),
    read: |e, doc| e.as_ref().map_or(Value::Null, |e| edge_out(e, doc)));
reference_field!(Edges, Vec<EdgeSel>, Vec<EdgeRef>, true, "list of edges",
    parse: |v| each(list(v)?, EdgeSel::parse),
    assign: |a, doc| each(a, |e| select::edge_ref(doc, e)),
    read: |e, doc| e.iter().map(|e| edge_out(e, doc)).collect());
reference_field!(VertexF, VertexSel, VertexRef, true, "vertex",
    parse: VertexSel::parse,
    assign: |a, doc| select::vertex_ref(doc, a),
    read: vertex_out);
reference_field!(SketchF, FeatureSel, FeatureId, true, "sketch (name or id)",
    parse: FeatureSel::parse,
    assign: |a, doc| {
        let id = a.resolve(doc)?;
        if doc.model.sketch(id).is_some() {
            Ok(id)
        } else {
            Err(format!("{} is not a sketch", doc.model.name_of(id)))
        }
    },
    read: |id, doc| json!(doc.model.name_of(*id)));
reference_field!(Features, Vec<FeatureSel>, Vec<FeatureId>, true,
    "list of features (names or ids)",
    parse: |v| each(list(v)?, FeatureSel::parse),
    assign: |a, doc| {
        if a.is_empty() {
            return Err("expected at least one feature".to_owned());
        }
        each(a, |f| f.resolve(doc))
    },
    read: |ids, doc| ids.iter().map(|id| doc.model.name_of(*id)).collect());
reference_field!(EndF, End, EndCondition, false,
"blind | mid_plane | through_all | {\"up_to\": plane}",
parse: End::parse,
assign: |a, doc| Ok(match a {
    End::Blind => EndCondition::Blind,
    End::MidPlane => EndCondition::Symmetric,
    End::ThroughAll => EndCondition::ThroughAll,
    End::UpTo(p) => EndCondition::UpTo(select::plane(doc, p)?.0),
}),
read: |e, doc| match e {
    EndCondition::Blind => json!("blind"),
    EndCondition::Symmetric => json!("mid_plane"),
    EndCondition::ThroughAll => json!("through_all"),
    EndCondition::UpTo(p) => json!({ "up_to": plane_out(p, doc) }),
});
reference_field!(RegionsF, Regions, RegionSelection, false,
"\"auto\", or a list of sketch points [x, y] inside the regions to use",
parse: Regions::parse,
assign: |a, doc| Ok(match a {
    Regions::Auto => RegionSelection::Auto,
    Regions::Points(p) => RegionSelection::Points(
        p.iter().map(|p| mm2(*p, &doc.model.parameters.units)).collect(),
    ),
}),
read: |r, doc| match r {
    RegionSelection::Auto => json!("auto"),
    RegionSelection::Points(p) => p
        .iter()
        .map(|p| point2_out(*p, &doc.model.parameters.units))
        .collect(),
});
reference_field!(BendF, Bend, BendModelDef, false,
"{\"k_factor\": number} | {\"allowance\": length} | {\"deduction\": length}",
parse: Bend::parse,
assign: |a, doc| Ok(match a {
    Bend::KFactor(v) => BendModelDef::KFactor(v.scalar(ScalarKind::Number, doc)?),
    Bend::Allowance(v) => BendModelDef::Allowance(v.scalar(ScalarKind::Length, doc)?),
    Bend::Deduction(v) => BendModelDef::Deduction(v.scalar(ScalarKind::Length, doc)?),
}),
read: |b, doc| match b {
    BendModelDef::KFactor(s) => json!({ "k_factor": scalar_out(s, ScalarKind::Number, doc) }),
    BendModelDef::Allowance(s) => {
        json!({ "allowance": scalar_out(s, ScalarKind::Length, doc) })
    }
    BendModelDef::Deduction(s) => {
        json!({ "deduction": scalar_out(s, ScalarKind::Length, doc) })
    }
});

fn second_direction() -> LinearDirection {
    LinearDirection {
        direction: AxisRef::Standard(StdAxis::Y),
        spacing: Scalar::new(20.0),
        count: 2,
        flip: false,
    }
}

/// A field of a linear pattern's second direction, which exists once any of them is set.
macro_rules! second_field {
    ($name:ident, $inner:ty, $member:ident) => {
        pub(crate) struct $name;
        impl Field for $name {
            type Arg = <$inner as Field>::Arg;
            type Target = Option<LinearDirection>;
            fn describe() -> String {
                format!("{} (the second direction of a grid)", <$inner>::describe())
            }
            fn parse(v: &Value) -> Result<Self::Arg, String> {
                <$inner>::parse(v)
            }
            fn assign(
                arg: &Self::Arg,
                target: &mut Self::Target,
                doc: &Document,
            ) -> Result<(), String> {
                let second = target.get_or_insert_with(second_direction);
                <$inner>::assign(arg, &mut second.$member, doc)
            }
            fn read(target: &Self::Target, doc: &Document) -> Value {
                target
                    .as_ref()
                    .map_or(Value::Null, |s| <$inner>::read(&s.$member, doc))
            }
        }
    };
}

second_field!(Direction2, AxisF, direction);
second_field!(Spacing2, Length, spacing);
second_field!(Count2, Count, count);
second_field!(Flip2, Flag, flip);

fn field_arg<F: Field>(a: &mut Args, name: &str) -> Result<Option<F::Arg>, String> {
    let value = if F::NULLABLE {
        a.take_nullable(name)
    } else {
        a.take(name)
    };
    value
        .map(|v| F::parse(&v).map_err(|e| format!("{name}: {e}")))
        .transpose()
}

fn describe_field<F: Field>() -> String {
    if F::REQUIRED {
        format!("{} (required)", F::describe())
    } else {
        F::describe()
    }
}

// ---- The features ----

/// Declares the arguments of one kind of feature: see the module documentation.
///
/// `Name: outer pattern => place, inner pattern => { field: ArgType as FieldKind = target, … }`
/// where the patterns pick the feature out of a `FeatureKind` and bind its members, and
/// each `target` is one of those bindings.
macro_rules! feature_args {
    (
        $(#[$doc:meta])*
        $name:ident: $outer:pat => $src:expr, $inner:pat => {
            $( $(#[$fdoc:meta])* $field:ident: $arg:ty as $fk:ty = $target:expr ),* $(,)?
        }
    ) => {
        $(#[$doc])*
        ///
        /// Every field is optional: one left as `None` keeps its default in a new
        /// feature, and its value in an edit.
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct $name {
            $( $(#[$fdoc])* pub $field: Option<$arg>, )*
        }

        impl $name {
            pub(crate) fn from_args(a: &mut Args) -> Result<Self, String> {
                Ok(Self {
                    $( $field: field_arg::<$fk>(a, stringify!($field))?, )*
                })
            }

            pub(crate) fn describe() -> Map<String, Value> {
                let mut out = Map::new();
                $( out.insert(stringify!($field).to_owned(), json!(describe_field::<$fk>())); )*
                out
            }

            /// Sets the given fields on `kind`. `None` if `kind` is another kind of
            /// feature. A new feature must be given every reference it needs.
            #[allow(irrefutable_let_patterns)]
            pub(crate) fn set(
                &self,
                kind: &mut FeatureKind,
                doc: &Document,
                creating: bool,
            ) -> Option<Result<(), String>> {
                if let $outer = kind
                    && let $inner = $src
                {
                    let mut missing: Vec<&str> = Vec::new();
                    $(
                        match &self.$field {
                            Some(v) => {
                                if let Err(e) = <$fk as Field>::assign(v, $target, doc) {
                                    return Some(Err(format!("{}: {e}", stringify!($field))));
                                }
                            }
                            None if creating && <$fk as Field>::REQUIRED => {
                                missing.push(stringify!($field));
                            }
                            None => {}
                        }
                    )*
                    Some(if missing.is_empty() {
                        Ok(())
                    } else {
                        Err(format!("Missing: {}.", missing.join(", ")))
                    })
                } else {
                    None
                }
            }

            /// The fields of `kind` as JSON. `None` if `kind` is another kind of feature.
            #[allow(irrefutable_let_patterns)]
            pub(crate) fn get(kind: &mut FeatureKind, doc: &Document) -> Option<Map<String, Value>> {
                if let $outer = kind
                    && let $inner = $src
                {
                    let mut out = Map::new();
                    $(
                        let v = <$fk as Field>::read($target, doc);
                        if !v.is_null() || <$fk as Field>::NULLABLE {
                            out.insert(stringify!($field).to_owned(), v);
                        }
                    )*
                    Some(out)
                } else {
                    None
                }
            }
        }
    };
}

feature_args! {
    /// A sketch's placement. (Sketches are added with `Op::Sketch`.)
    SketchPlane: FeatureKind::Sketch(f) => &mut **f,
        SketchFeature { plane, placement: _, sketch: _ } => {
    /// What the sketch lies on.
    on: PlaneSel as PlaneF = plane,
    }
}

feature_args! {
    /// An extrusion or a cut of a sketch's closed regions.
    Extrude: FeatureKind::Extrude(f) => &mut **f,
        ExtrudeFeature { sketch, params: peet_model::Extrude { end, depth, reverse, operation, regions } } => {
    sketch: FeatureSel as SketchF = sketch,
    operation: Operation as Choice<Operation> = operation,
    end: End as EndF = end,
    depth: Input as Length = depth,
    reverse: bool as Flag = reverse,
    regions: Regions as RegionsF = regions,
    }
}

feature_args! {
    /// A reference plane parallel to another, a distance away.
    OffsetPlane: FeatureKind::Plane(def) => def, PlaneDef::Offset { from, distance, flip } => {
    from: PlaneSel as PlaneF = from,
    distance: Input as Length = distance,
    flip: bool as Flag = flip,
    }
}

feature_args! {
    /// A reference plane turned about an axis.
    AngledPlane: FeatureKind::Plane(def) => def, PlaneDef::Angled { from, about, angle } => {
    from: PlaneSel as PlaneF = from,
    about: AxisSel as AxisF = about,
    angle: Input as Angle = angle,
    }
}

feature_args! {
    /// A reference plane halfway between two parallel planes.
    MidPlane: FeatureKind::Plane(def) => def, PlaneDef::Midplane { a, b } => {
    a: PlaneSel as PlaneF = a,
    b: PlaneSel as PlaneF = b,
    }
}

feature_args! {
    /// A reference axis along an edge.
    EdgeAxis: FeatureKind::Axis(def) => def, AxisDef::Edge(edge) => {
    edge: EdgeSel as EdgeF = edge,
    }
}

feature_args! {
    /// The axis of a round face.
    CylinderAxis: FeatureKind::Axis(def) => def, AxisDef::Cylinder(face) => {
    face: FaceSel as FaceF = face,
    }
}

feature_args! {
    /// A reference axis where two planes meet.
    PlanesAxis: FeatureKind::Axis(def) => def, AxisDef::TwoPlanes(a, b) => {
    a: PlaneSel as PlaneF = a,
    b: PlaneSel as PlaneF = b,
    }
}

feature_args! {
    /// A reference point at a vertex.
    VertexPoint: FeatureKind::Point(def) => def, PointDef::Vertex(vertex) => {
    vertex: VertexSel as VertexF = vertex,
    }
}

feature_args! {
    /// A reference point at coordinates.
    CoordinatesPoint: FeatureKind::Point(def) => def, PointDef::Coordinates { x, y, z } => {
    x: Input as Length = x,
    y: Input as Length = y,
    z: Input as Length = z,
    }
}

feature_args! {
    /// A coordinate system: an origin with the axes of a plane.
    CoordinateSystem: FeatureKind::CoordSystem(def) => def, CoordSystemDef { origin, orientation } => {
    origin: PointSel as PointF = origin,
    orientation: PlaneSel as PlaneF = orientation,
    }
}

feature_args! {
    /// The first feature of a sheet metal body.
    BaseFlange: FeatureKind::BaseFlange(f) => &mut **f,
        BaseFlangeFeature {
            sketch,
            settings: SheetSettingsDef { thickness, radius, model, relief, relief_ratio },
            reverse,
            depth,
            symmetric,
            flip_depth,
        } => {
    sketch: FeatureSel as SketchF = sketch,
    thickness: Input as Length = thickness,
    /// The default inner bend radius.
    radius: Input as Length = radius,
    bend: Bend as BendF = model,
    relief: ReliefType as Choice<ReliefType> = relief,
    relief_ratio: Input as Number = relief_ratio,
    reverse: bool as Flag = reverse,
    /// Open profiles: how far the profile is extruded.
    depth: Input as Length = depth,
    symmetric: bool as Flag = symmetric,
    flip_depth: bool as Flag = flip_depth,
    }
}

feature_args! {
    /// A flange with a bend on an edge of a sheet metal body.
    EdgeFlange: FeatureKind::EdgeFlange(f) => &mut **f,
        EdgeFlangeFeature { edge, length, angle, position, offset_start, offset_end, flip, radius } => {
    edge: EdgeSel as OptEdge = edge,
    length: Input as Length = length,
    angle: Input as Angle = angle,
    position: FlangePosition as Choice<FlangePosition> = position,
    offset_start: Input as Length = offset_start,
    offset_end: Input as Length = offset_end,
    flip: bool as Flag = flip,
    /// `Some(None)` uses the body's default radius.
    radius: Option<Input> as OptLength = radius,
    }
}

feature_args! {
    /// A cut through a sheet metal body, made in the flat pattern.
    SheetCut: FeatureKind::SheetCut(f) => &mut **f, SheetCutFeature { sketch } => {
    sketch: FeatureSel as SketchF = sketch,
    }
}

feature_args! {
    /// An edge of a sheet metal body folded back over itself.
    Hem: FeatureKind::Hem(f) => &mut **f,
        HemFeature { edge, kind, length, gap, radius, angle, inside, offset_start, offset_end, flip } => {
    edge: EdgeSel as OptEdge = edge,
    kind: HemKind as Choice<HemKind> = kind,
    length: Input as Length = length,
    gap: Input as Length = gap,
    radius: Input as Length = radius,
    angle: Input as Angle = angle,
    inside: bool as Flag = inside,
    offset_start: Input as Length = offset_start,
    offset_end: Input as Length = offset_end,
    flip: bool as Flag = flip,
    }
}

feature_args! {
    /// A bend along the lines of a sketch on a flat face.
    SketchedBend: FeatureKind::SketchedBend(f) => &mut **f,
        SketchedBendFeature { sketch, angle, radius, position, flip, flip_fixed } => {
    sketch: FeatureSel as SketchF = sketch,
    angle: Input as Angle = angle,
    radius: Option<Input> as OptLength = radius,
    position: BendLinePosition as Choice<BendLinePosition> = position,
    flip: bool as Flag = flip,
    flip_fixed: bool as Flag = flip_fixed,
    }
}

feature_args! {
    /// A step in a flat face: two bends along a sketched line.
    Jog: FeatureKind::Jog(f) => &mut **f,
        JogFeature { sketch, offset, dimension, angle, radius, position, flip, flip_fixed } => {
    sketch: FeatureSel as SketchF = sketch,
    offset: Input as Length = offset,
    dimension: JogDimension as Choice<JogDimension> = dimension,
    angle: Input as Angle = angle,
    radius: Option<Input> as OptLength = radius,
    position: BendLinePosition as Choice<BendLinePosition> = position,
    flip: bool as Flag = flip,
    flip_fixed: bool as Flag = flip_fixed,
    }
}

feature_args! {
    /// A flange with a sketched profile along several edges of one face.
    MiterFlange: FeatureKind::MiterFlange(f) => &mut **f,
        MiterFlangeFeature { sketch, edges, gap, offset_start, offset_end } => {
    sketch: FeatureSel as SketchF = sketch,
    edges: Vec<EdgeSel> as Edges = edges,
    gap: Input as Length = gap,
    offset_start: Input as Length = offset_start,
    offset_end: Input as Length = offset_end,
    }
}

feature_args! {
    /// How the corners where flanges meet are treated.
    Corner: FeatureKind::Corner(f) => &mut **f,
        CornerFeature { faces, kind, gap, relief, relief_size } => {
    /// Faces of the flanges at the corners. None: every corner of the body.
    faces: Vec<FaceSel> as Faces = faces,
    kind: CornerKind as Choice<CornerKind> = kind,
    gap: Input as Length = gap,
    relief: CornerRelief as Choice<CornerRelief> = relief,
    relief_size: Input as Length = relief_size,
    }
}

feature_args! {
    /// Dimples, embosses or louvers from the shapes of a sketch on a flat face.
    Form: FeatureKind::Form(f) => &mut **f,
        FormFeature { sketch, kind, height, flip, open_side } => {
    sketch: FeatureSel as SketchF = sketch,
    kind: FormKind as Choice<FormKind> = kind,
    height: Input as Length = height,
    flip: bool as Flag = flip,
    open_side: u32 as Count = open_side,
    }
}

feature_args! {
    /// Copies of features in a row, or in a grid once a second direction is given.
    LinearPattern: FeatureKind::Pattern(f) => &mut **f,
        PatternFeature {
            seeds,
            def: PatternDef::Linear { first: LinearDirection { direction, spacing, count, flip }, second },
        } => {
    features: Vec<FeatureSel> as Features = seeds,
    direction: AxisSel as AxisF = direction,
    spacing: Input as Length = spacing,
    /// How many, the original included.
    count: u32 as Count = count,
    flip: bool as Flag = flip,
    direction2: AxisSel as Direction2 = second,
    spacing2: Input as Spacing2 = second,
    count2: u32 as Count2 = second,
    flip2: bool as Flip2 = second,
    }
}

feature_args! {
    /// Copies of features around an axis.
    CircularPattern: FeatureKind::Pattern(f) => &mut **f,
        PatternFeature { seeds, def: PatternDef::Circular { axis, count, angle, flip } } => {
    features: Vec<FeatureSel> as Features = seeds,
    axis: AxisSel as AxisF = axis,
    count: u32 as Count = count,
    angle: Input as Angle = angle,
    flip: bool as Flag = flip,
    }
}

feature_args! {
    /// Mirror images of features across a plane.
    Mirror: FeatureKind::Mirror(f) => &mut **f, MirrorFeature { seeds, plane } => {
    features: Vec<FeatureSel> as Features = seeds,
    plane: PlaneSel as PlaneF = plane,
    }
}

// ---- New features ----

fn no_plane() -> PlaneRef {
    PlaneRef::Standard(StdPlane::Top)
}

fn no_face() -> FaceRef {
    FaceRef {
        name: FaceName::merged([]),
        neighbours: Vec::new(),
        hint: peet_math::DVec3::ZERO,
    }
}

fn no_edge() -> EdgeRef {
    EdgeRef {
        faces: [FaceName::merged([]), FaceName::merged([])],
        hint: peet_math::DVec3::ZERO,
    }
}

const NO_SKETCH: FeatureId = FeatureId(0);

fn extrusion(operation: Operation) -> FeatureKind {
    FeatureKind::Extrude(Box::new(ExtrudeFeature {
        sketch: NO_SKETCH,
        params: peet_model::Extrude::new(operation),
    }))
}

fn form(kind: FormKind) -> FeatureKind {
    FeatureKind::Form(Box::new(FormFeature::new(NO_SKETCH, kind)))
}

fn pattern(def: PatternDef) -> FeatureKind {
    FeatureKind::Pattern(Box::new(PatternFeature {
        seeds: Vec::new(),
        def,
    }))
}

/// Declares the kinds of feature operations add: the variant, its arguments, its word in
/// JSON (`op`, or `op.form` where one operation has several forms), what it does, and the
/// feature it starts from (with placeholders for the references it requires).
macro_rules! feature_ops {
    ( $( $variant:ident($args:ident) $word:literal $help:literal => $blank:expr ),* $(,)? ) => {
        /// The fields of a feature to add or to change, by kind.
        #[derive(Clone, Debug, PartialEq)]
        pub enum FeatureArgs {
            $( #[doc = $help] $variant($args), )*
        }

        /// A kind of feature operations can add: its word, what it does, its fields.
        pub(crate) type FeatureOp = (&'static str, &'static str, fn() -> Map<String, Value>);

        impl FeatureArgs {
            pub(crate) const OPS: &'static [FeatureOp] = &[ $( ($word, $help, $args::describe), )* ];

            /// The feature a new one starts from, or `None` if this kind isn't added
            /// this way.
            pub(crate) fn blank(&self) -> Option<FeatureKind> {
                match self {
                    $( Self::$variant(_) => $blank, )*
                }
            }

            pub(crate) fn word(&self) -> &'static str {
                match self {
                    $( Self::$variant(_) => $word, )*
                }
            }

            /// Sets the given fields on `kind`. `None` if `kind` is another kind of feature.
            pub(crate) fn set(
                &self,
                kind: &mut FeatureKind,
                doc: &Document,
                creating: bool,
            ) -> Option<Result<(), String>> {
                match self {
                    $( Self::$variant(a) => a.set(kind, doc, creating), )*
                }
            }

            /// Reads the arguments of the kind called `word` from a JSON operation.
            pub(crate) fn parse(word: &str, a: &mut Args) -> Option<Result<Self, String>> {
                match word {
                    $( $word => Some($args::from_args(a).map(Self::$variant)), )*
                    _ => None,
                }
            }

            /// Reads the arguments that fit `kind` from a JSON operation (for an edit).
            pub(crate) fn parse_for(
                kind: &mut FeatureKind,
                a: &mut Args,
                doc: &Document,
            ) -> Result<Self, String> {
                $(
                    if $args::get(kind, doc).is_some() {
                        return $args::from_args(a).map(Self::$variant);
                    }
                )*
                Err("That feature has no fields to edit.".to_owned())
            }

            /// The fields of `kind` as JSON, in the form an edit takes them (references
            /// are described in words, since a selector can't be reconstructed from one).
            pub(crate) fn get(kind: &FeatureKind, doc: &Document) -> Map<String, Value> {
                let mut kind = kind.clone();
                $(
                    if let Some(m) = $args::get(&mut kind, doc) {
                        return m;
                    }
                )*
                Map::new()
            }
        }
    };
}

feature_ops! {
    Extrude(Extrude) "extrude"
        "Extrude a sketch's closed regions: a new body, or added to the bodies it touches."
        => Some(extrusion(Operation::Add)),
    Cut(Extrude) "cut"
        "Cut a sketch's closed regions out of the bodies (an extrude with operation \"cut\")."
        => Some(extrusion(Operation::Cut)),
    OffsetPlane(OffsetPlane) "plane.offset"
        "A reference plane parallel to a plane or a flat face, a distance away."
        => Some(FeatureKind::Plane(PlaneDef::Offset {
            from: no_plane(),
            distance: Scalar::new(10.0),
            flip: false,
        })),
    AngledPlane(AngledPlane) "plane.angled"
        "A reference plane turned about an axis."
        => Some(FeatureKind::Plane(PlaneDef::Angled {
            from: no_plane(),
            about: AxisRef::Standard(StdAxis::X),
            angle: Scalar::new(45.0),
        })),
    MidPlane(MidPlane) "plane.midplane"
        "A reference plane halfway between two parallel planes."
        => Some(FeatureKind::Plane(PlaneDef::Midplane { a: no_plane(), b: no_plane() })),
    EdgeAxis(EdgeAxis) "axis.edge"
        "A reference axis along an edge."
        => Some(FeatureKind::Axis(AxisDef::Edge(no_edge()))),
    CylinderAxis(CylinderAxis) "axis.cylinder"
        "The axis of a round face (a hole or a boss)."
        => Some(FeatureKind::Axis(AxisDef::Cylinder(no_face()))),
    PlanesAxis(PlanesAxis) "axis.planes"
        "A reference axis where two planes meet."
        => Some(FeatureKind::Axis(AxisDef::TwoPlanes(no_plane(), no_plane()))),
    CoordinatesPoint(CoordinatesPoint) "point.coordinates"
        "A reference point at coordinates."
        => Some(FeatureKind::Point(PointDef::Coordinates {
            x: Scalar::new(0.0),
            y: Scalar::new(0.0),
            z: Scalar::new(0.0),
        })),
    VertexPoint(VertexPoint) "point.vertex"
        "A reference point at a vertex."
        => Some(FeatureKind::Point(PointDef::Vertex(VertexRef {
            faces: Vec::new(),
            hint: peet_math::DVec3::ZERO,
        }))),
    CoordinateSystem(CoordinateSystem) "coordinate_system"
        "A coordinate system: an origin point with the axes of a plane."
        => Some(FeatureKind::CoordSystem(CoordSystemDef {
            origin: PointRef::Origin,
            orientation: no_plane(),
        })),
    BaseFlange(BaseFlange) "base_flange"
        "Start a sheet metal body: a plate from a closed sketch, or a bent profile from connected lines."
        => Some(FeatureKind::BaseFlange(Box::new(BaseFlangeFeature::new(NO_SKETCH)))),
    EdgeFlange(EdgeFlange) "edge_flange"
        "A flange with a bend on an edge of a sheet metal body ('edges' makes one per edge)."
        => Some(FeatureKind::EdgeFlange(Box::new(EdgeFlangeFeature::new(None)))),
    SheetCut(SheetCut) "sheet_cut"
        "Cut a sketch through the sheet it is drawn on, in the flat pattern."
        => Some(FeatureKind::SheetCut(Box::new(SheetCutFeature { sketch: NO_SKETCH }))),
    Hem(Hem) "hem"
        "Fold an edge of a sheet metal body back over itself ('edges' makes one per edge)."
        => Some(FeatureKind::Hem(Box::new(HemFeature::new(None)))),
    SketchedBend(SketchedBend) "sketched_bend"
        "Bend a flat face along the lines of a sketch drawn on it."
        => Some(FeatureKind::SketchedBend(Box::new(SketchedBendFeature::new(NO_SKETCH)))),
    Jog(Jog) "jog"
        "A step in a flat face: two bends along a sketched line."
        => Some(FeatureKind::Jog(Box::new(JogFeature::new(NO_SKETCH)))),
    MiterFlange(MiterFlange) "miter_flange"
        "A flange with a sketched profile along several edges of one face."
        => Some(FeatureKind::MiterFlange(Box::new(MiterFlangeFeature::new(NO_SKETCH, Vec::new())))),
    Corner(Corner) "corner"
        "How the corners between flanges are treated (every corner if no faces are given)."
        => Some(FeatureKind::Corner(Box::new(CornerFeature::new(Vec::new())))),
    Dimple(Form) "dimple"
        "Round pressed forms from the circles of a sketch on a flat face."
        => Some(form(FormKind::Dimple)),
    Emboss(Form) "emboss"
        "Pressed forms from the closed shapes of a sketch on a flat face."
        => Some(form(FormKind::Emboss)),
    Louver(Form) "louver"
        "Louvers from the closed shapes of a sketch on a flat face."
        => Some(form(FormKind::Louver)),
    LinearPattern(LinearPattern) "linear_pattern"
        "Copies of features in a row (direction, spacing, count) or a grid (add direction2, spacing2, count2)."
        => Some(pattern(PatternDef::Linear {
            first: LinearDirection {
                direction: AxisRef::Standard(StdAxis::X),
                ..second_direction()
            },
            second: None,
        })),
    CircularPattern(CircularPattern) "circular_pattern"
        "Copies of features around an axis."
        => Some(pattern(PatternDef::Circular {
            axis: AxisRef::Standard(StdAxis::Z),
            count: 4,
            angle: Scalar::new(360.0),
            flip: false,
        })),
    Mirror(Mirror) "mirror"
        "Mirror images of features across a plane."
        => Some(FeatureKind::Mirror(Box::new(MirrorFeature {
            seeds: Vec::new(),
            plane: no_plane(),
        }))),
    SketchPlane(SketchPlane) "sketch_plane"
        "Move a sketch to another plane (an edit only: sketches are added with 'sketch')."
        => None,
}

impl FeatureArgs {
    /// The word of the kind a JSON operation means: `plane`, `axis` and `point` have
    /// several forms, told apart by the fields given.
    pub(crate) fn word_for(op: &str, a: &Args) -> String {
        let form = match op {
            "plane" if a.has("about") => "angled",
            "plane" if a.has("a") || a.has("b") => "midplane",
            "plane" => "offset",
            "axis" if a.has("face") => "cylinder",
            "axis" if a.has("a") || a.has("b") => "planes",
            "axis" => "edge",
            "point" if a.has("vertex") => "vertex",
            "point" => "coordinates",
            _ => return op.to_owned(),
        };
        format!("{op}.{form}")
    }

    /// Whether a JSON operation called `op` adds a feature.
    pub(crate) fn adds(op: &str) -> bool {
        Self::OPS
            .iter()
            .any(|(word, ..)| word.split('.').next() == Some(op) && *word != "sketch_plane")
    }
}
