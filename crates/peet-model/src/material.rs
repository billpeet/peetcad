//! What a part is made of.
//!
//! A part has one material: a name for the bill of materials and a density for its mass.
//! It is part of the [`crate::Model`] (and so of the file), not of the application's
//! settings: a part weighs the same wherever it is opened.

use serde::{Deserialize, Serialize};

/// The density of mild steel, kg/m³.
pub const STEEL_DENSITY: f64 = 7850.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Material {
    /// As the bill of materials shows it: "Mild steel".
    pub name: String,
    /// In kg/m³.
    pub density: f64,
}

impl Material {
    /// Densities a material can have: anything between aerogel and osmium.
    pub const DENSITY_RANGE: std::ops::RangeInclusive<f64> = 1.0..=30_000.0;

    /// A material, with its name trimmed. Fails, with a message for the user, if the
    /// name is empty or the density is not one a material can have.
    pub fn new(name: &str, density: f64) -> Result<Self, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("A material needs a name.".to_owned());
        }
        if !density.is_finite() || !Self::DENSITY_RANGE.contains(&density) {
            return Err(format!(
                "A density of {density} kg/m³ is not one a material can have: give it in kg/m³ (steel is {STEEL_DENSITY})."
            ));
        }
        Ok(Self {
            name: name.to_owned(),
            density,
        })
    }

    /// The mass in kg of `volume` mm³ of the material.
    pub fn mass_kg(&self, volume_mm3: f64) -> f64 {
        volume_mm3 * 1e-9 * self.density
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_its_values_and_weighs() {
        let steel = Material::new(" Mild steel ", STEEL_DENSITY).unwrap();
        assert_eq!(steel.name, "Mild steel");
        // A litre of steel.
        assert!((steel.mass_kg(1e6) - 7.85).abs() < 1e-12);
        assert!(Material::new("", 1000.0).is_err());
        assert!(Material::new("Foam", 0.0).is_err());
        assert!(Material::new("Foam", f64::NAN).is_err());
        assert!(Material::new("Steel", 7.85).is_ok_and(|m| m.density == 7.85));
    }
}
