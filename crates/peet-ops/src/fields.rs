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

use crate::value::{HoleStandard, RevolveAxis};
use peet_document::Document;
use peet_model::feature::{AxisDef, CoordSystemDef, FeatureKind, PatternDef, PlaneDef, PointDef};
use peet_model::{
    AxisRef, BaseFlangeFeature, BendModelDef, CornerFeature, EdgeFlangeFeature, EdgeRef,
    EndCondition, ExtrudeFeature, FaceName, FaceRef, FeatureId, FormFeature, HemFeature,
    JogFeature, LinearDirection, MirrorFeature, MiterFlangeFeature, Operation, PatternFeature,
    PlaneRef, PointRef, RegionSelection, Scalar, ScalarKind, SheetCutFeature, SheetSettingsDef,
    SketchFeature, SketchedBendFeature, StdAxis, StdPlane, VertexRef,
};
use peet_model::{
    BlendFeature, BlendKind, ConvertToSheetFeature, DraftFeature, HoleEnd, HoleFeature, HoleFit,
    HoleKind, LoftFeature, METRIC, RevolveAxisRef, RevolveFeature, ShellFeature, SweepFeature,
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
    /// Whether a new feature must be given it (it has no sensible default).
    const REQUIRED: bool = false;
    /// Whether `null` is a value: "the default", or "to be picked afterwards".
    const NULLABLE: bool = false;
    /// What the field takes, for `help`.
    fn describe() -> String;
    fn parse(v: &Value) -> Result<Self::Arg, String>;
    fn assign(arg: &Self::Arg, target: &mut Self::Target, doc: &Document) -> Result<(), String>;
    fn read(target: &Self::Target, doc: &Document) -> Value;
    /// The value that, assigned, gives exactly `target`: what a feature has, as an
    /// operation would set it. `None` if there is nothing to set.
    fn arg(target: &Self::Target, doc: &Document) -> Option<Self::Arg>;
}

pub(crate) fn scalar_out(s: &Scalar, kind: ScalarKind, doc: &Document) -> Value {
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

/// A stored value as an operation's input, exactly.
pub(crate) fn input_of(s: &Scalar) -> Input {
    match &s.expression {
        Some(e) => Input::Expr(e.clone()),
        None => Input::Base(s.value),
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
            fn arg(target: &Scalar, _: &Document) -> Option<Input> {
                Some(input_of(target))
            }
        }
    };
}

scalar_field!(Length, ScalarKind::Length, "length");
scalar_field!(Angle, ScalarKind::Angle, "angle in degrees");
scalar_field!(Number, ScalarKind::Number, "number");
scalar_field!(
    PatternCount,
    ScalarKind::Number,
    "whole number from 2 to 10000 (original included), or an expression returning one"
);

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
    fn arg(target: &Self::Target, _: &Document) -> Option<Self::Arg> {
        Some(target.as_ref().map(input_of))
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
    fn arg(target: &bool, _: &Document) -> Option<bool> {
        Some(*target)
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
    fn arg(target: &u32, _: &Document) -> Option<u32> {
        Some(*target)
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
    FormKind,
    HoleKind,
    HoleFit
);

impl Words for BlendKind {
    fn all() -> &'static [Self] {
        &[BlendKind::Fillet, BlendKind::Chamfer]
    }
}

impl Words for HoleEnd {
    fn all() -> &'static [Self] {
        &[HoleEnd::ThroughAll, HoleEnd::Blind]
    }
}

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
    fn arg(target: &T, _: &Document) -> Option<T> {
        Some(target.clone())
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

/// `null`, or what `parse` reads.
fn or_null<T>(v: &Value, parse: fn(&Value) -> Result<T, String>) -> Result<Option<T>, String> {
    if v.is_null() {
        Ok(None)
    } else {
        parse(v).map(Some)
    }
}

fn a_sketch(sel: &FeatureSel, doc: &Document) -> Result<FeatureId, String> {
    let id = sel.resolve(doc)?;
    if doc.model.sketch(id).is_some() {
        Ok(id)
    } else {
        Err(format!("{} is not a sketch", doc.model.name_of(id)))
    }
}

/// A field described by four functions: how its value is read from JSON, turned into what
/// the feature stores, written as JSON, and recovered from what the feature stores.
macro_rules! reference_field {
    ($name:ident, $arg:ty, $target:ty, required: $required:literal, nullable: $nullable:literal,
     $text:literal,
     parse: $parse:expr, assign: $assign:expr, read: $read:expr, arg: $of:expr) => {
        pub(crate) struct $name;
        impl Field for $name {
            type Arg = $arg;
            type Target = $target;
            const REQUIRED: bool = $required;
            const NULLABLE: bool = $nullable;
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
            fn arg(target: &$target, doc: &Document) -> Option<$arg> {
                let of: fn(&$target, &Document) -> $arg = $of;
                Some(of(target, doc))
            }
        }
    };
}

reference_field!(PlaneF, PlaneSel, PlaneRef, required: true, nullable: false, "plane",
    parse: PlaneSel::parse,
    assign: |a, doc| Ok(select::plane(doc, a)?.0),
    read: plane_out,
    arg: |p, _| p.clone().into());
reference_field!(AxisF, AxisSel, AxisRef, required: true, nullable: false, "axis",
    parse: AxisSel::parse,
    assign: |a, doc| select::axis(doc, a),
    read: axis_out,
    arg: |a, _| a.clone().into());
reference_field!(PointF, PointSel, PointRef, required: true, nullable: false, "point",
    parse: PointSel::parse,
    assign: |a, doc| select::point(doc, a),
    read: |p, doc| match p {
        PointRef::Origin => json!("origin"),
        PointRef::Feature(id) => json!(doc.model.name_of(*id)),
        PointRef::Vertex(v) => vertex_out(v, doc),
    },
    arg: |p, _| p.clone().into());
reference_field!(FaceF, FaceSel, FaceRef, required: true, nullable: false, "face",
    parse: FaceSel::parse,
    assign: |a, doc| select::face_ref(doc, a),
    read: face_out,
    arg: |f, _| FaceSel::Ref(f.clone()));
reference_field!(Faces, Vec<FaceSel>, Vec<FaceRef>, required: false, nullable: false,
    "list of faces",
    parse: |v| each(list(v)?, FaceSel::parse),
    assign: |a, doc| each(a, |f| select::face_ref(doc, f)),
    read: |f, doc| f.iter().map(|f| face_out(f, doc)).collect(),
    arg: |f, _| f.iter().cloned().map(FaceSel::Ref).collect());
reference_field!(SomeFaces, Vec<FaceSel>, Vec<FaceRef>, required: true, nullable: false,
    "list of faces (may be empty: to be picked afterwards)",
    parse: |v| each(list(v)?, FaceSel::parse),
    assign: |a, doc| each(a, |f| select::face_ref(doc, f)),
    read: |f, doc| f.iter().map(|f| face_out(f, doc)).collect(),
    arg: |f, _| f.iter().cloned().map(FaceSel::Ref).collect());
reference_field!(EdgeF, EdgeSel, EdgeRef, required: true, nullable: false, "edge",
    parse: EdgeSel::parse,
    assign: |a, doc| select::edge_ref(doc, a),
    read: edge_out,
    arg: |e, _| EdgeSel::Ref(e.clone()));
reference_field!(OptEdge, Option<EdgeSel>, Option<EdgeRef>, required: true, nullable: true,
    "edge, or null to pick it afterwards",
    parse: |v| or_null(v, EdgeSel::parse),
    assign: |a, doc| a.as_ref().map(|e| select::edge_ref(doc, e)).transpose(),
    read: |e, doc| e.as_ref().map_or(Value::Null, |e| edge_out(e, doc)),
    arg: |e, _| e.clone().map(EdgeSel::Ref));
reference_field!(Edges, Vec<EdgeSel>, Vec<EdgeRef>, required: true, nullable: false,
    "list of edges (may be empty: to be picked afterwards)",
    parse: |v| each(list(v)?, EdgeSel::parse),
    assign: |a, doc| each(a, |e| select::edge_ref(doc, e)),
    read: |e, doc| e.iter().map(|e| edge_out(e, doc)).collect(),
    arg: |e, _| e.iter().cloned().map(EdgeSel::Ref).collect());
reference_field!(VertexF, VertexSel, VertexRef, required: true, nullable: false, "vertex",
    parse: VertexSel::parse,
    assign: |a, doc| select::vertex_ref(doc, a),
    read: vertex_out,
    arg: |v, _| VertexSel::Ref(v.clone()));
reference_field!(SketchF, FeatureSel, FeatureId, required: true, nullable: false,
    "sketch (name or id)",
    parse: FeatureSel::parse,
    assign: a_sketch,
    read: |id, doc| json!(doc.model.name_of(*id)),
    arg: |id, _| FeatureSel::Id(*id));
reference_field!(Sketches, Vec<FeatureSel>, Vec<FeatureId>, required: true, nullable: false,
    "list of sketches (names or ids), in order (more may be picked afterwards)",
    parse: |v| each(list(v)?, FeatureSel::parse),
    assign: |a, doc| each(a, |s| a_sketch(s, doc)),
    read: |ids, doc| ids.iter().map(|id| doc.model.name_of(*id)).collect(),
    arg: |ids, _| ids.iter().copied().map(FeatureSel::Id).collect());
reference_field!(OptFace, Option<FaceSel>, Option<FaceRef>, required: false, nullable: true,
    "face, or null for the largest flat face of the only body",
    parse: |v| or_null(v, FaceSel::parse),
    assign: |a, doc| a.as_ref().map(|f| select::face_ref(doc, f)).transpose(),
    read: |f, doc| f.as_ref().map_or(Value::Null, |f| face_out(f, doc)),
    arg: |f, _| f.clone().map(FaceSel::Ref));
reference_field!(OptSketch, Option<FeatureSel>, Option<FeatureId>, required: true, nullable: true,
    "sketch (name or id), or null to choose it afterwards",
    parse: |v| or_null(v, FeatureSel::parse),
    assign: |a, doc| a.as_ref().map(|s| a_sketch(s, doc)).transpose(),
    read: |id, doc| id.map_or(Value::Null, |id| json!(doc.model.name_of(id))),
    arg: |id, _| id.map(FeatureSel::Id));
reference_field!(Features, Vec<FeatureSel>, Vec<FeatureId>, required: true, nullable: false,
    "list of features (names or ids)",
    parse: |v| each(list(v)?, FeatureSel::parse),
    assign: |a, doc| {
        if a.is_empty() {
            return Err("expected at least one feature".to_owned());
        }
        each(a, |f| f.resolve(doc))
    },
    read: |ids, doc| ids.iter().map(|id| doc.model.name_of(*id)).collect(),
    arg: |ids, _| ids.iter().copied().map(FeatureSel::Id).collect());
reference_field!(OptPlane, Option<PlaneSel>, Option<PlaneRef>, required: true, nullable: true,
    "plane, or null to pick it afterwards",
    parse: |v| or_null(v, PlaneSel::parse),
    assign: |a, doc| Ok(match a {
        Some(p) => Some(select::plane(doc, p)?.0),
        None => None,
    }),
    read: |p, doc| p.as_ref().map_or(Value::Null, |p| plane_out(p, doc)),
    arg: |p, _| p.clone().map(Into::into));
reference_field!(EndF, End, EndCondition, required: false, nullable: false,
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
},
arg: |e, _| match e {
    EndCondition::Blind => End::Blind,
    EndCondition::Symmetric => End::MidPlane,
    EndCondition::ThroughAll => End::ThroughAll,
    EndCondition::UpTo(p) => End::UpTo(p.clone().into()),
});
reference_field!(RegionsF, Regions, RegionSelection, required: false, nullable: false,
"\"auto\", or a list of sketch points [x, y] inside the regions to use",
parse: Regions::parse,
assign: |a, doc| Ok(match a {
    Regions::Auto => RegionSelection::Auto,
    Regions::Points(p) => RegionSelection::Points(
        p.iter().map(|p| mm2(*p, &doc.model.parameters.units)).collect(),
    ),
    Regions::PointsMm(p) => RegionSelection::Points(p.clone()),
}),
read: |r, doc| match r {
    RegionSelection::Auto => json!("auto"),
    RegionSelection::Points(p) => p
        .iter()
        .map(|p| point2_out(*p, &doc.model.parameters.units))
        .collect(),
},
arg: |r, _| match r {
    RegionSelection::Auto => Regions::Auto,
    RegionSelection::Points(p) => Regions::PointsMm(p.clone()),
});
reference_field!(BendF, Bend, BendModelDef, required: false, nullable: false,
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
},
arg: |b, _| match b {
    BendModelDef::KFactor(s) => Bend::KFactor(input_of(s)),
    BendModelDef::Allowance(s) => Bend::Allowance(input_of(s)),
    BendModelDef::Deduction(s) => Bend::Deduction(input_of(s)),
});
reference_field!(RevolveAxisF, RevolveAxis, RevolveAxisRef, required: false, nullable: false,
"sketch_x | sketch_y | {\"line\": id of a line of the sketch} | axis (default: the sketch's first construction line, else sketch_y)",
parse: RevolveAxis::parse,
assign: |a, doc| Ok(match a {
    RevolveAxis::SketchX => RevolveAxisRef::SketchX,
    RevolveAxis::SketchY => RevolveAxisRef::SketchY,
    RevolveAxis::SketchLine(id) => RevolveAxisRef::SketchLine(*id),
    RevolveAxis::Axis(axis) => RevolveAxisRef::Axis(select::axis(doc, axis)?),
}),
read: |a, doc| match a {
    RevolveAxisRef::SketchX => json!("sketch_x"),
    RevolveAxisRef::SketchY => json!("sketch_y"),
    RevolveAxisRef::SketchLine(id) => json!({ "line": id.0 }),
    RevolveAxisRef::Axis(axis) => axis_out(axis, doc),
},
arg: |a, _| match a {
    RevolveAxisRef::SketchX => RevolveAxis::SketchX,
    RevolveAxisRef::SketchY => RevolveAxis::SketchY,
    RevolveAxisRef::SketchLine(id) => RevolveAxis::SketchLine(*id),
    RevolveAxisRef::Axis(axis) => RevolveAxis::Axis(axis.clone().into()),
});

fn metric(name: &str) -> Result<&'static peet_model::MetricSize, String> {
    METRIC
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            let names: Vec<&str> = METRIC.iter().map(|m| m.name).collect();
            format!("'{name}' is not a standard size: use {}", names.join(", "))
        })
}

reference_field!(StandardF, Option<HoleStandard>, Option<(String, HoleFit)>,
    required: false, nullable: true,
    "{\"size\": \"M6\", \"fit\": close | normal | loose | tapped}: sets every size of the hole from a standard screw size; null for a hole of no standard size",
    parse: |v| or_null(v, HoleStandard::parse),
    // The sizes themselves are set by `FeatureArgs::prepare`, before the other fields.
    assign: |a, _| Ok(match a {
        Some(s) => Some((metric(&s.size)?.name.to_owned(), s.fit)),
        None => None,
    }),
    read: |s, _| s.as_ref().map_or(Value::Null, |(size, fit)| {
        json!({ "size": size, "fit": fit.word() })
    }),
    arg: |s, _| s.clone().map(|(size, fit)| HoleStandard { size, fit }));

/// Text that can be absent.
pub(crate) struct OptText;

impl Field for OptText {
    type Arg = Option<String>;
    type Target = Option<String>;
    const NULLABLE: bool = true;
    fn describe() -> String {
        "text, or null for none".to_owned()
    }
    fn parse(v: &Value) -> Result<Self::Arg, String> {
        if v.is_null() {
            Ok(None)
        } else {
            text(v).map(|t| Some(t.to_owned()))
        }
    }
    fn assign(arg: &Self::Arg, target: &mut Self::Target, _: &Document) -> Result<(), String> {
        target.clone_from(arg);
        Ok(())
    }
    fn read(target: &Self::Target, _: &Document) -> Value {
        json!(target)
    }
    fn arg(target: &Self::Target, _: &Document) -> Option<Self::Arg> {
        Some(target.clone())
    }
}

fn second_direction() -> LinearDirection {
    LinearDirection {
        direction: AxisRef::Standard(StdAxis::Y),
        spacing: Scalar::new(20.0),
        count: Scalar::new(2.0),
        flip: false,
    }
}

/// The second direction of a linear pattern: giving it makes the pattern a grid, and
/// `null` makes it a row again.
pub(crate) struct Direction2;

impl Field for Direction2 {
    type Arg = Option<AxisSel>;
    type Target = Option<LinearDirection>;
    const NULLABLE: bool = true;
    fn describe() -> String {
        "axis (the second direction: a grid), or null for a row".to_owned()
    }
    fn parse(v: &Value) -> Result<Self::Arg, String> {
        or_null(v, AxisSel::parse)
    }
    fn assign(arg: &Self::Arg, target: &mut Self::Target, doc: &Document) -> Result<(), String> {
        match arg {
            Some(axis) => {
                target.get_or_insert_with(second_direction).direction = select::axis(doc, axis)?;
            }
            None => *target = None,
        }
        Ok(())
    }
    fn read(target: &Self::Target, doc: &Document) -> Value {
        target
            .as_ref()
            .map_or(Value::Null, |s| axis_out(&s.direction, doc))
    }
    fn arg(target: &Self::Target, _: &Document) -> Option<Self::Arg> {
        Some(target.as_ref().map(|s| s.direction.clone().into()))
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
            fn arg(target: &Self::Target, doc: &Document) -> Option<Self::Arg> {
                target.as_ref().and_then(|s| <$inner>::arg(&s.$member, doc))
            }
        }
    };
}

second_field!(Spacing2, Length, spacing);
second_field!(Count2, PatternCount, count);
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

            /// Every field of `kind`, as the arguments that would set it. `None` if
            /// `kind` is another kind of feature.
            #[allow(irrefutable_let_patterns)]
            pub(crate) fn of(kind: &mut FeatureKind, doc: &Document) -> Option<Self> {
                if let $outer = kind
                    && let $inner = $src
                {
                    Some(Self {
                        $( $field: <$fk as Field>::arg($target, doc), )*
                    })
                } else {
                    None
                }
            }

            /// These arguments without the fields that are the same in `base`.
            pub(crate) fn only_changes(mut self, base: &Self) -> Self {
                $(
                    if self.$field == base.$field {
                        self.$field = None;
                    }
                )*
                self
            }

            /// Whether no field is given.
            pub fn is_empty(&self) -> bool {
                true $( && self.$field.is_none() )*
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
        SketchFeature { plane, placement: _, sketch: _, projections: _ } => {
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
    /// `Some(None)`: to be picked afterwards.
    edge: Option<EdgeSel> as OptEdge = edge,
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
    /// `Some(None)`: to be picked afterwards.
    edge: Option<EdgeSel> as OptEdge = edge,
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
    count: Input as PatternCount = count,
    flip: bool as Flag = flip,
    /// `Some(None)`: a row, not a grid.
    direction2: Option<AxisSel> as Direction2 = second,
    spacing2: Input as Spacing2 = second,
    count2: Input as Count2 = second,
    flip2: bool as Flip2 = second,
    }
}

feature_args! {
    /// Copies of features around an axis.
    CircularPattern: FeatureKind::Pattern(f) => &mut **f,
        PatternFeature { seeds, def: PatternDef::Circular { axis, count, angle, flip } } => {
    features: Vec<FeatureSel> as Features = seeds,
    axis: AxisSel as AxisF = axis,
    count: Input as PatternCount = count,
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

feature_args! {
    /// A sketch's closed regions turned about an axis.
    Revolve: FeatureKind::Revolve(f) => &mut **f,
        RevolveFeature { sketch, axis, angle, symmetric, reverse, operation, regions } => {
    sketch: FeatureSel as SketchF = sketch,
    /// The sketch's first construction line if absent, else its vertical axis.
    axis: RevolveAxis as RevolveAxisF = axis,
    /// Degrees: 360 is all the way round.
    angle: Input as Angle = angle,
    symmetric: bool as Flag = symmetric,
    reverse: bool as Flag = reverse,
    operation: Operation as Choice<Operation> = operation,
    regions: Regions as RegionsF = regions,
    }
}

feature_args! {
    /// A fillet or a chamfer along edges.
    Blend: FeatureKind::Blend(f) => &mut **f, BlendFeature { kind, edges, size, chain } => {
    kind: BlendKind as Choice<BlendKind> = kind,
    edges: Vec<EdgeSel> as Edges = edges,
    /// The fillet's radius, or how far the chamfer is set back on both faces.
    size: Input as Length = size,
    /// Carry on along the edges that continue each edge smoothly.
    chain: bool as Flag = chain,
    }
}

feature_args! {
    /// Bodies hollowed, leaving walls of one thickness.
    Shell: FeatureKind::Shell(f) => &mut **f, ShellFeature { open, thickness } => {
    /// The faces to remove, opening the hollow. None: closed hollows.
    open: Vec<FaceSel> as Faces = open,
    thickness: Input as Length = thickness,
    }
}

feature_args! {
    /// Faces tapered about the lines where they cross a neutral plane.
    Draft: FeatureKind::Draft(f) => &mut **f, DraftFeature { faces, neutral, angle, flip } => {
    faces: Vec<FaceSel> as SomeFaces = faces,
    /// The part keeps its size in this plane; its normal is the direction of pull.
    neutral: Option<PlaneSel> as OptPlane = neutral,
    angle: Input as Angle = angle,
    flip: bool as Flag = flip,
    }
}

feature_args! {
    /// Holes at the points of a sketch.
    Hole: FeatureKind::Hole(f) => &mut **f,
        HoleFeature {
            sketch,
            kind,
            diameter,
            end,
            depth,
            tip_angle,
            counterbore_diameter,
            counterbore_depth,
            countersink_diameter,
            countersink_angle,
            reverse,
            thread,
            standard,
        } => {
    sketch: FeatureSel as SketchF = sketch,
    /// Sets every size from a standard screw size; sizes given as well win.
    standard: Option<HoleStandard> as StandardF = standard,
    kind: HoleKind as Choice<HoleKind> = kind,
    diameter: Input as Length = diameter,
    end: HoleEnd as Choice<HoleEnd> = end,
    /// Blind holes: the depth of the full diameter.
    depth: Input as Length = depth,
    tip_angle: Input as Angle = tip_angle,
    counterbore_diameter: Input as Length = counterbore_diameter,
    counterbore_depth: Input as Length = counterbore_depth,
    countersink_diameter: Input as Length = countersink_diameter,
    countersink_angle: Input as Angle = countersink_angle,
    reverse: bool as Flag = reverse,
    /// A cosmetic thread's designation ("M6x1").
    thread: Option<String> as OptText = thread,
    }
}

feature_args! {
    /// A sketch's closed regions swept along the path drawn in another sketch.
    Sweep: FeatureKind::Sweep(f) => &mut **f, SweepFeature { profile, path, operation, regions } => {
    profile: FeatureSel as SketchF = profile,
    /// `Some(None)`: to be chosen afterwards.
    path: Option<FeatureSel> as OptSketch = path,
    operation: Operation as Choice<Operation> = operation,
    regions: Regions as RegionsF = regions,
    }
}

feature_args! {
    /// A body through the closed profiles of several sketches, one after another.
    Loft: FeatureKind::Loft(f) => &mut **f, LoftFeature { sections, operation } => {
    /// The profiles in order, each with the same number of edges (a circle adapts).
    profiles: Vec<FeatureSel> as Sketches = sections,
    operation: Operation as Choice<Operation> = operation,
    }
}

feature_args! {
    /// A solid of one wall thickness turned into a sheet metal body.
    ConvertToSheet: FeatureKind::ConvertToSheet(f) => &mut **f,
        ConvertToSheetFeature { face, model, relief, relief_ratio } => {
    /// The flat face that stays fixed. Absent: the largest flat face of the only body.
    face: Option<FaceSel> as OptFace = face,
    bend: Bend as BendF = model,
    relief: ReliefType as Choice<ReliefType> = relief,
    relief_ratio: Input as Number = relief_ratio,
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

fn revolve(operation: Operation) -> FeatureKind {
    FeatureKind::Revolve(Box::new(RevolveFeature {
        sketch: NO_SKETCH,
        axis: RevolveAxisRef::SketchY,
        angle: Scalar::new(360.0),
        symmetric: false,
        reverse: false,
        operation,
        regions: RegionSelection::Auto,
    }))
}

fn blend(kind: BlendKind) -> FeatureKind {
    FeatureKind::Blend(Box::new(BlendFeature::new(kind, Vec::new())))
}

fn sweep(operation: Operation) -> FeatureKind {
    FeatureKind::Sweep(Box::new(SweepFeature::new(NO_SKETCH, None, operation)))
}

fn loft(operation: Operation) -> FeatureKind {
    FeatureKind::Loft(Box::new(LoftFeature::new(Vec::new(), operation)))
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

            /// Every field of a feature, as the arguments that would set it: what to
            /// add to make a feature like it. `None` for a kind with no fields (an
            /// imported body).
            pub fn of(kind: &FeatureKind, doc: &Document) -> Option<Self> {
                let mut kind = kind.clone();
                $(
                    if let Some(args) = $args::of(&mut kind, doc) {
                        return Some(Self::$variant(args));
                    }
                )*
                None
            }

            /// The arguments that turn the feature `old` into `new`: only the fields that
            /// differ, or every field if `new` is another form of the feature (an offset
            /// plane made an angled one). `None` for a kind with no fields.
            pub fn changes(old: &FeatureKind, new: &FeatureKind, doc: &Document) -> Option<Self> {
                let (mut old, mut new) = (old.clone(), new.clone());
                $(
                    if let Some(after) = $args::of(&mut new, doc) {
                        return Some(Self::$variant(match $args::of(&mut old, doc) {
                            Some(before) => after.only_changes(&before),
                            None => after,
                        }));
                    }
                )*
                None
            }

            /// Whether no field is given.
            pub fn is_empty(&self) -> bool {
                match self {
                    $( Self::$variant(a) => a.is_empty(), )*
                }
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
            count: Scalar::new(4.0),
            angle: Scalar::new(360.0),
            flip: false,
        })),
    Mirror(Mirror) "mirror"
        "Mirror images of features across a plane."
        => Some(FeatureKind::Mirror(Box::new(MirrorFeature {
            seeds: Vec::new(),
            plane: no_plane(),
        }))),
    Revolve(Revolve) "revolve"
        "Turn a sketch's closed regions about an axis: a new body, or added to the bodies it touches."
        => Some(revolve(Operation::Add)),
    CutRevolve(Revolve) "cut_revolve"
        "Cut a sketch's closed regions, turned about an axis, out of the bodies."
        => Some(revolve(Operation::Cut)),
    Sweep(Sweep) "sweep"
        "Sweep a sketch's closed regions along the path drawn in another sketch: lines and arcs (corners between lines are mitred), or a spline."
        => Some(sweep(Operation::Add)),
    CutSweep(Sweep) "cut_sweep"
        "Cut a sketch's closed regions, swept along a path, out of the bodies."
        => Some(sweep(Operation::Cut)),
    Loft(Loft) "loft"
        "Join the closed profiles of two or more sketches, in order, into a body."
        => Some(loft(Operation::Add)),
    CutLoft(Loft) "cut_loft"
        "Cut the shape through the profiles of two or more sketches out of the bodies."
        => Some(loft(Operation::Cut)),
    ConvertToSheet(ConvertToSheet) "convert_to_sheet"
        "Turn a solid of one wall thickness (flat walls, rounded bends) into a sheet metal body."
        => Some(FeatureKind::ConvertToSheet(Box::new(ConvertToSheetFeature::new(None)))),
    Fillet(Blend) "fillet"
        "Round edges with a radius ('size')."
        => Some(blend(BlendKind::Fillet)),
    Chamfer(Blend) "chamfer"
        "Bevel edges, set back by 'size' on both faces."
        => Some(blend(BlendKind::Chamfer)),
    Shell(Shell) "shell"
        "Hollow the bodies, leaving walls of one thickness; 'open' faces are removed."
        => Some(FeatureKind::Shell(Box::new(ShellFeature::new(Vec::new())))),
    Draft(Draft) "draft"
        "Taper faces about a neutral plane: flat ones, round ones along the pull, or freeform ones."
        => Some(FeatureKind::Draft(Box::new(DraftFeature::new(Vec::new(), None)))),
    Hole(Hole) "hole"
        "Drill a hole at every point of a sketch: plain, counterbored or countersunk, to a standard size or given ones."
        => Some(FeatureKind::Hole(Box::new(HoleFeature::new(NO_SKETCH)))),
    SketchPlane(SketchPlane) "sketch_plane"
        "Move a sketch to another plane (an edit only: sketches are added with 'sketch')."
        => None,
}

/// The word of the arguments that fit a feature. Every kind of feature is listed, so a
/// new kind doesn't compile until it is given arguments here (or is said to have none).
fn word_of(kind: &FeatureKind) -> Option<&'static str> {
    Some(match kind {
        FeatureKind::Sketch(_) => "sketch_plane",
        FeatureKind::Extrude(_) => "extrude",
        FeatureKind::Plane(PlaneDef::Offset { .. }) => "plane.offset",
        FeatureKind::Plane(PlaneDef::Angled { .. }) => "plane.angled",
        FeatureKind::Plane(PlaneDef::Midplane { .. }) => "plane.midplane",
        FeatureKind::Axis(AxisDef::Edge(_)) => "axis.edge",
        FeatureKind::Axis(AxisDef::Cylinder(_)) => "axis.cylinder",
        FeatureKind::Axis(AxisDef::TwoPlanes(..)) => "axis.planes",
        FeatureKind::Point(PointDef::Vertex(_)) => "point.vertex",
        FeatureKind::Point(PointDef::Coordinates { .. }) => "point.coordinates",
        FeatureKind::CoordSystem(_) => "coordinate_system",
        FeatureKind::BaseFlange(_) => "base_flange",
        FeatureKind::EdgeFlange(_) => "edge_flange",
        FeatureKind::SheetCut(_) => "sheet_cut",
        FeatureKind::Hem(_) => "hem",
        FeatureKind::SketchedBend(_) => "sketched_bend",
        FeatureKind::Jog(_) => "jog",
        FeatureKind::MiterFlange(_) => "miter_flange",
        FeatureKind::Corner(_) => "corner",
        FeatureKind::Form(_) => "dimple",
        FeatureKind::Pattern(p) => match p.def {
            PatternDef::Linear { .. } => "linear_pattern",
            PatternDef::Circular { .. } => "circular_pattern",
        },
        FeatureKind::Mirror(_) => "mirror",
        FeatureKind::Revolve(_) => "revolve",
        FeatureKind::Blend(_) => "fillet",
        FeatureKind::Shell(_) => "shell",
        FeatureKind::Draft(_) => "draft",
        FeatureKind::Hole(_) => "hole",
        FeatureKind::Sweep(_) => "sweep",
        FeatureKind::Loft(_) => "loft",
        FeatureKind::ConvertToSheet(_) => "convert_to_sheet",
        // An imported body is what its file says: it has nothing to set.
        FeatureKind::Import(_) => return None,
    })
}

impl FeatureArgs {
    /// Reads the arguments that fit `kind` from a JSON operation (for an edit).
    pub(crate) fn parse_for(kind: &FeatureKind, a: &mut Args) -> Result<Self, String> {
        word_of(kind)
            .and_then(|word| Self::parse(word, a))
            .unwrap_or_else(|| Err("That feature has no fields to edit.".to_owned()))
    }

    /// What must happen before the fields are set: a hole's standard size sets its other
    /// sizes, a new revolve turns about its sketch's centreline, and the first body of a
    /// part is a new body.
    pub(crate) fn prepare(
        &self,
        kind: &mut FeatureKind,
        doc: &Document,
        creating: bool,
    ) -> Result<(), String> {
        let first_body = creating && doc.evaluation().bodies.is_empty();
        match (self, kind) {
            (Self::Hole(args), FeatureKind::Hole(hole)) => {
                if let Some(Some(standard)) = &args.standard {
                    hole.set_standard(metric(&standard.size)?, standard.fit);
                }
            }
            (Self::Revolve(args) | Self::CutRevolve(args), FeatureKind::Revolve(r)) if creating => {
                let sketch = args.sketch.as_ref().and_then(|s| s.resolve(doc).ok());
                if args.axis.is_none()
                    && let Some(s) = sketch.and_then(|id| doc.model.sketch(id))
                {
                    r.axis = peet_model::default_axis(&s.sketch);
                }
                if first_body && args.operation.is_none() && r.operation == Operation::Add {
                    r.operation = Operation::NewBody;
                }
            }
            (Self::Sweep(args) | Self::CutSweep(args), FeatureKind::Sweep(s))
                if first_body && args.operation.is_none() && s.operation == Operation::Add =>
            {
                s.operation = Operation::NewBody;
            }
            (Self::Loft(args) | Self::CutLoft(args), FeatureKind::Loft(l))
                if first_body && args.operation.is_none() && l.operation == Operation::Add =>
            {
                l.operation = Operation::NewBody;
            }
            _ => {}
        }
        Ok(())
    }

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
