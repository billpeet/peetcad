//! Convert to sheet metal: a plain solid of constant wall thickness becomes a sheet metal
//! body.
//!
//! The solid may come from anywhere: extruded and filleted, shelled, or imported from a
//! STEP file. [`peet_sheetmetal::recognize`] reads its flat walls and cylindrical bends
//! into a layout, and the body is replaced by the sheet metal body built from that layout:
//! the same shape, now with a flat pattern, and ready for edge flanges, hems and sheet
//! metal cuts. The thickness and the bend radii come from the solid; the bend model (the
//! K-factor) can't be measured from a shape and is the feature's.
//!
//! **Names.** The new body's faces are named like any sheet metal body's, after this
//! feature: its walls' sides ([`FaceRole::SheetTop`], [`FaceRole::SheetBottom`]), its
//! bends' ([`FaceRole::BendTop`], [`FaceRole::BendBottom`]), its outline edges
//! ([`FaceRole::Wall`]) and the edges of its holes ([`FaceRole::Side`]). References made
//! to the solid's own faces before the conversion (by features above it) are untouched;
//! features below it refer to the new names.
//!
//! [`FaceRole::SheetTop`]: crate::FaceRole::SheetTop
//! [`FaceRole::SheetBottom`]: crate::FaceRole::SheetBottom
//! [`FaceRole::BendTop`]: crate::FaceRole::BendTop
//! [`FaceRole::BendBottom`]: crate::FaceRole::BendBottom
//! [`FaceRole::Wall`]: crate::FaceRole::Wall
//! [`FaceRole::Side`]: crate::FaceRole::Side

use std::sync::Arc;

use peet_sheetmetal::{BendModel, ReliefType, SheetSettings, recognize};
use peet_sketch::expr::Parameters;
use serde::{Deserialize, Serialize};

use crate::extrude::stamp;
use crate::feature::{BendModelDef, FeatureId, Scalar, ScalarKind, SheetSettingsDef};
use crate::naming::{Body, FaceRef, find_face};
use crate::sheet::{Applied, face_names};
use crate::{FeatureError, Model};

/// Turns a solid body of constant wall thickness into a sheet metal body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConvertToSheetFeature {
    /// The flat face that stays in place when the part is unfolded; it becomes the top
    /// side of the first flange, and its body is the one converted. `None`: the largest
    /// flat face of the only body.
    pub face: Option<FaceRef>,
    /// How the flat length of the bends is worked out (the solid can't tell).
    pub model: BendModelDef,
    /// Reliefs for flanges added to the body later.
    pub relief: ReliefType,
    pub relief_ratio: Scalar,
}

impl ConvertToSheetFeature {
    pub fn new(face: Option<FaceRef>) -> Self {
        let d = SheetSettingsDef::default();
        Self {
            face,
            model: d.model,
            relief: d.relief,
            relief_ratio: d.relief_ratio,
        }
    }

    /// Every value that can hold an expression.
    pub fn scalars(&self) -> Vec<&Scalar> {
        let model = match &self.model {
            BendModelDef::KFactor(v) | BendModelDef::Allowance(v) | BendModelDef::Deduction(v) => v,
        };
        vec![model, &self.relief_ratio]
    }
}

/// The settings a conversion hands to recognition: the bend model and the reliefs. The
/// thickness and the radius are placeholders, replaced by what the solid has.
pub(crate) fn convert_settings(
    def: &ConvertToSheetFeature,
    params: &Parameters,
) -> Result<SheetSettings, String> {
    let value = |s: &Scalar, kind, label: &str| {
        s.evaluate(kind, params)
            .map_err(|m| format!("{label}: {m}."))
    };
    let d = SheetSettings::default();
    let settings = SheetSettings {
        thickness: d.thickness,
        radius: d.radius,
        model: match &def.model {
            BendModelDef::KFactor(k) => {
                BendModel::KFactor(value(k, ScalarKind::Number, "K-factor")?)
            }
            BendModelDef::Allowance(v) => {
                BendModel::Allowance(value(v, ScalarKind::Length, "Bend allowance")?)
            }
            BendModelDef::Deduction(v) => {
                BendModel::Deduction(value(v, ScalarKind::Length, "Bend deduction")?)
            }
        },
        relief: def.relief,
        relief_ratio: value(&def.relief_ratio, ScalarKind::Number, "Relief ratio")?,
    };
    settings.check()?;
    Ok(settings)
}

/// Replaces the body the fixed face is on (or the only body) with the sheet metal body
/// recognised in it.
pub(crate) fn apply_convert(
    feature: FeatureId,
    model: &Model,
    bodies: &[Arc<Body>],
    def: &ConvertToSheetFeature,
    settings: &SheetSettings,
    seed: u64,
) -> Result<Applied, FeatureError> {
    let (index, fixed) = match &def.face {
        Some(face) => {
            let found = find_face(bodies, face).ok_or_else(|| {
                FeatureError(format!(
                    "The fixed face no longer exists ({}). Edit the feature and pick a flat face of the body to convert.",
                    model.describe_face(&face.name)
                ))
            })?;
            (found.body, Some(found.id))
        }
        None => match bodies.len() {
            0 => {
                return Err(FeatureError(
                    "There is no body to convert. Make or import a solid first.".to_owned(),
                ));
            }
            1 => (0, None),
            n => {
                return Err(FeatureError(format!(
                    "There are {n} bodies: pick a flat face of the one to convert (the face that stays in place when the part is unfolded)."
                )));
            }
        },
    };
    let body = &bodies[index];
    if body.sheet.is_some() {
        return Err(FeatureError(
            "This body is already sheet metal: it has a flat pattern and takes flanges as it is."
                .to_owned(),
        ));
    }
    let found = recognize(&body.solid, fixed, settings, feature.0)
        .map_err(|e| FeatureError(e.message(|o| model.name_of(FeatureId(o)).to_owned())))?;
    let mut out = bodies.to_vec();
    out[index] = Arc::new(Body {
        face_names: face_names(&found.body),
        solid: found.solid,
        origin: body.origin,
        stamp: stamp(seed, index),
        sheet: Some(Arc::new(found.body)),
    });
    Ok((out, None))
}
