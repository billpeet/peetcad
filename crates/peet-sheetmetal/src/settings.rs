//! Sheet metal settings and bend math.
//!
//! **Terms** (the usual ones in sheet metal handbooks), for a bend of angle `A` (the angle
//! the flange turns through: 90° for a right-angle flange), inner radius `R` and thickness
//! `t`:
//!
//! - *K-factor* `K`: where the neutral axis (the layer that neither stretches nor
//!   compresses) lies, as a fraction of the thickness from the inside of the bend.
//! - *Bend allowance* `BA = A · (R + K·t)`: the length of the neutral axis through the
//!   bend, which is the width of the bend region in the flat pattern.
//! - *Outside setback* `OSSB = (R + t) · tan(A/2)`: from the tangent line of the bend to
//!   the *outer virtual sharp*, where the outer faces of the two flanges would meet if
//!   there were no bend.
//! - *Bend deduction* `BD = 2·OSSB − BA`: how much shorter the flat pattern is than the
//!   sum of the outside flange lengths.

use std::f64::consts::{PI, TAU};

use serde::{Deserialize, Serialize};

/// How the flat length of a bend is worked out.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum BendModel {
    /// The neutral axis at this fraction of the thickness (typically 0.3 to 0.5).
    KFactor(f64),
    /// A fixed bend allowance in mm, for every bend.
    Allowance(f64),
    /// A fixed bend deduction in mm, for every bend.
    Deduction(f64),
}

impl BendModel {
    pub fn label(&self) -> &'static str {
        match self {
            Self::KFactor(_) => "K-factor",
            Self::Allowance(_) => "Bend allowance",
            Self::Deduction(_) => "Bend deduction",
        }
    }
}

/// The cut made where a bend ends partway along an edge, so the bend can form without
/// tearing the flat material next to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReliefType {
    /// A rectangular slot.
    Rectangular,
    /// A slot with a round end.
    Obround,
    /// No material removed: the sheet is ripped along the side of the bend.
    Tear,
}

impl ReliefType {
    pub const ALL: [Self; 3] = [Self::Rectangular, Self::Obround, Self::Tear];

    pub fn label(self) -> &'static str {
        match self {
            Self::Rectangular => "Rectangular",
            Self::Obround => "Obround",
            Self::Tear => "Tear",
        }
    }
}

/// Where an edge flange sits relative to the edge it is added to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FlangePosition {
    /// The flange's outer face is flush with the original edge: the part keeps its
    /// outside size (the usual choice for enclosures).
    MaterialInside,
    /// The flange's inner face is flush with the original edge.
    MaterialOutside,
    /// The bend starts at the original edge: the base keeps its full size.
    BendOutside,
}

impl FlangePosition {
    pub const ALL: [Self; 3] = [
        Self::MaterialInside,
        Self::MaterialOutside,
        Self::BendOutside,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::MaterialInside => "Material inside",
            Self::MaterialOutside => "Material outside",
            Self::BendOutside => "Bend outside",
        }
    }
}

/// The settings of a sheet metal body, with every value evaluated (mm, unitless).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SheetSettings {
    pub thickness: f64,
    /// The default inner bend radius.
    pub radius: f64,
    pub model: BendModel,
    pub relief: ReliefType,
    /// Relief size as a multiple of the thickness: the slot is `ratio · t` wide and
    /// reaches `ratio · t` past the bend.
    pub relief_ratio: f64,
}

impl Default for SheetSettings {
    fn default() -> Self {
        Self {
            thickness: 1.5,
            radius: 1.5,
            model: BendModel::KFactor(0.44),
            relief: ReliefType::Rectangular,
            relief_ratio: 0.5,
        }
    }
}

impl SheetSettings {
    /// Checks the values, with a message for the user.
    pub fn check(&self) -> Result<(), String> {
        let positive = |v: f64| v.is_finite() && v > peet_math::tolerance::LINEAR;
        if !positive(self.thickness) {
            return Err("The thickness must be greater than zero.".to_owned());
        }
        if !(self.radius.is_finite() && self.radius >= 0.0) {
            return Err("The bend radius can't be negative.".to_owned());
        }
        match self.model {
            BendModel::KFactor(k) if !(k.is_finite() && (0.0..=1.0).contains(&k)) => {
                return Err("The K-factor must be between 0 and 1.".to_owned());
            }
            BendModel::Allowance(v) | BendModel::Deduction(v) if !v.is_finite() => {
                return Err("The bend allowance or deduction is not a number.".to_owned());
            }
            _ => {}
        }
        if !(self.relief_ratio.is_finite() && self.relief_ratio > 0.0) {
            return Err("The relief ratio must be greater than zero.".to_owned());
        }
        Ok(())
    }
}

/// The numbers of one bend.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BendValues {
    /// The angle the flange turns through, in radians (`0 < angle < π`).
    pub angle: f64,
    /// Inner radius.
    pub radius: f64,
    pub thickness: f64,
    /// Bend allowance: the width of the bend region in the flat pattern.
    pub allowance: f64,
}

impl BendValues {
    /// Works out the allowance of a bend from the bend model.
    pub fn new(model: BendModel, angle: f64, radius: f64, thickness: f64) -> Result<Self, String> {
        if !(angle.is_finite() && angle > 1e-6 && angle < PI - 1e-6) {
            return Err(format!(
                "A bend angle must be between 0° and 180° (this one is {:.3}°).",
                angle.to_degrees()
            ));
        }
        Self::make(model, angle, radius, thickness)
    }

    /// A bend that may turn through 180° or more (a hem or a roll): up to just short of a
    /// full turn. Bend deduction has no meaning past 180°, so the deduction model can't be
    /// used for those.
    pub fn hem(model: BendModel, angle: f64, radius: f64, thickness: f64) -> Result<Self, String> {
        if !(angle.is_finite() && angle > 1e-6 && angle < TAU - 1e-3) {
            return Err(format!(
                "A hem or roll must turn through less than 360° (this one is {:.3}°).",
                angle.to_degrees()
            ));
        }
        if angle >= PI - 1e-6 && matches!(model, BendModel::Deduction(_)) {
            return Err(
                "A bend deduction can't give the flat length of a bend of 180° or more. Use a K-factor or a bend allowance for this body."
                    .to_owned(),
            );
        }
        if !(radius.is_finite() && radius > 0.0) {
            return Err("The bend radius of a hem must be greater than zero.".to_owned());
        }
        Self::make(model, angle, radius, thickness)
    }

    fn make(model: BendModel, angle: f64, radius: f64, thickness: f64) -> Result<Self, String> {
        let ossb = (radius + thickness) * (angle / 2.0).tan();
        let allowance = match model {
            BendModel::KFactor(k) => angle * (radius + k * thickness),
            BendModel::Allowance(ba) => ba,
            BendModel::Deduction(bd) => 2.0 * ossb - bd,
        };
        if !(allowance.is_finite() && allowance > peet_math::tolerance::LINEAR) {
            return Err(format!(
                "The bend allowance works out at {allowance:.4} mm; it must be greater than zero. Check the bend model's value."
            ));
        }
        Ok(Self {
            angle,
            radius,
            thickness,
            allowance,
        })
    }

    /// Whether the bend turns through 180° or more (a hem or a roll), where there are no
    /// virtual sharps.
    pub fn is_hem(&self) -> bool {
        self.angle >= PI - 1e-6
    }

    /// Outside setback: tangent line to the outer virtual sharp. Not a number for hems.
    pub fn outside_setback(&self) -> f64 {
        if self.is_hem() {
            return f64::NAN;
        }
        (self.radius + self.thickness) * (self.angle / 2.0).tan()
    }

    /// Inside setback: tangent line to the inner virtual sharp. Not a number for hems.
    pub fn inside_setback(&self) -> f64 {
        if self.is_hem() {
            return f64::NAN;
        }
        self.radius * (self.angle / 2.0).tan()
    }

    /// How far the outside of the bend reaches past its tangent line, square to it.
    pub fn outside_reach(&self) -> f64 {
        let r = self.radius + self.thickness;
        if self.angle >= std::f64::consts::FRAC_PI_2 {
            r
        } else {
            r * self.angle.sin()
        }
    }

    /// Bend deduction. Not a number for hems.
    pub fn deduction(&self) -> f64 {
        2.0 * self.outside_setback() - self.allowance
    }

    /// The K-factor this bend effectively uses (whatever model gave the allowance).
    pub fn k_factor(&self) -> f64 {
        (self.allowance / self.angle - self.radius) / self.thickness
    }

    /// Radius of the neutral axis.
    pub fn neutral_radius(&self) -> f64 {
        self.allowance / self.angle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::FRAC_PI_2;

    #[test]
    fn right_angle_numbers() {
        // t = 2, R = 3, K = 0.5: BA = π/2 · 4 = 2π, OSSB = 5, BD = 10 − 2π.
        let b = BendValues::new(BendModel::KFactor(0.5), FRAC_PI_2, 3.0, 2.0).unwrap();
        assert!((b.allowance - std::f64::consts::TAU).abs() < 1e-12);
        assert!((b.outside_setback() - 5.0).abs() < 1e-12);
        assert!((b.deduction() - (10.0 - std::f64::consts::TAU)).abs() < 1e-12);
        assert!((b.k_factor() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn models_agree() {
        let k = BendValues::new(BendModel::KFactor(0.4), 1.0, 2.0, 1.5).unwrap();
        let ba = BendValues::new(BendModel::Allowance(k.allowance), 1.0, 2.0, 1.5).unwrap();
        let bd = BendValues::new(BendModel::Deduction(k.deduction()), 1.0, 2.0, 1.5).unwrap();
        assert!((ba.allowance - k.allowance).abs() < 1e-12);
        assert!((bd.allowance - k.allowance).abs() < 1e-12);
        assert!((bd.k_factor() - 0.4).abs() < 1e-12);
    }

    #[test]
    fn bad_values() {
        assert!(BendValues::new(BendModel::KFactor(0.5), 0.0, 1.0, 1.0).is_err());
        assert!(BendValues::new(BendModel::KFactor(0.5), 3.2, 1.0, 1.0).is_err());
        assert!(BendValues::new(BendModel::Deduction(100.0), 1.0, 1.0, 1.0).is_err());
        let mut s = SheetSettings::default();
        assert!(s.check().is_ok());
        s.thickness = 0.0;
        assert!(s.check().is_err());
    }
}
