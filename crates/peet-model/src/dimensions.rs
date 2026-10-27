//! Dimension names: every numeric value of a part has an address `NAME@FEATURE`, as in
//! SolidWorks (`Width@Sketch1`, `D1@LPattern2`), by which operations read and set it and
//! expressions read it.
//!
//! - A sketch's dimension has its name in the sketch (`d1`, or one it was given).
//! - Any other feature's numeric value (an extrusion's depth, a flange's length, a
//!   pattern's spacing and count) is `d1`, `d2`, … in the order the feature lists its values
//!   ([`FeatureKind::slots`]), as SolidWorks numbers a feature's dimensions D1, D2, …,
//!   unless it was given a name ([`Feature::dimension_names`]). Its field's name is an
//!   address of it too (`length@Edge-Flange1`).
//! - A name with an `@` is a whole address on its own. SolidWorks keeps a flange's length
//!   in the flange's profile sketch (`D2@Sketch10`), which PeetCAD's flange doesn't have:
//!   imported, the flange's `length` is named `D2@Sketch10`.
//!
//! Names are the same in every configuration; the values are each configuration's own
//! ([`crate::config`]), so an address reads and sets the value of the configuration in
//! question.
//!
//! Expressions read a dimension by its address (`"Width@Sketch1" / 2`). Before each
//! rebuild the model works out every dimension's value into [`Parameters::dimensions`]
//! ([`Model::refresh_dimension_values`]); a sketch's reference dimension has the value the
//! last rebuild measured.

use std::collections::BTreeMap;

use peet_sketch::expr::{self, DimensionValues, Quantity, QuantityKind};

use crate::Model;
use crate::config::Slot;
use crate::feature::{Feature, FeatureId, FeatureKind, Scalar, ScalarKind};

/// One dimension of the part, as it is in the active configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct Dimension {
    pub feature: FeatureId,
    pub slot: Slot,
    /// Its name: `Width`, `d1`, or a whole address.
    pub name: String,
    /// `NAME@FEATURE`, or the name itself if it is a whole address.
    pub address: String,
    /// The feature's field it is, for a value of a feature that is not a sketch.
    pub field: Option<&'static str>,
    pub kind: ScalarKind,
    pub value: Scalar,
    /// False for a sketch's reference dimension, which measures the sketch and can't be
    /// set.
    pub driving: bool,
}

/// The address of a dimension called `name` in a feature called `feature`.
pub fn address(name: &str, feature: &str) -> String {
    if name.contains('@') {
        name.to_owned()
    } else {
        format!("{name}@{feature}")
    }
}

fn quantity_kind(kind: ScalarKind) -> QuantityKind {
    match kind {
        ScalarKind::Length => QuantityKind::Length,
        ScalarKind::Angle => QuantityKind::Angle,
        ScalarKind::Number => QuantityKind::Number,
    }
}

/// The most rounds of working out dimensions that read each other through parameters.
const ROUNDS: usize = 8;

impl Feature {
    /// The names of the feature's numeric values (not a sketch's dimensions), by field:
    /// the name it was given, or `d1`, `d2`, … by its place among the values.
    pub fn value_names(&self) -> Vec<(&'static str, String)> {
        self.kind
            .slots()
            .into_iter()
            .enumerate()
            .map(|(i, (field, ..))| {
                let name = self
                    .dimension_names
                    .get(field)
                    .cloned()
                    .or_else(|| self.mate_value_name(field))
                    .unwrap_or_else(|| format!("d{}", i + 1));
                (field, name)
            })
            .collect()
    }

    /// A moved body's mate value's own name: `D1@` the mate's name, as SolidWorks calls
    /// a mate's dimension (`D1@Distance2`), if the mate has a name that can be one.
    fn mate_value_name(&self, field: &str) -> Option<String> {
        let FeatureKind::MoveBody(m) = &self.kind else {
            return None;
        };
        let name = m.mate_of_field(field)?.name.trim();
        let whole = format!("D1@{name}");
        (!name.is_empty() && expr::check_dimension_name(&whole, true).is_ok()).then_some(whole)
    }
}

impl Model {
    /// The dimensions of a feature: a sketch's, or its numeric values.
    pub fn dimensions_of(&self, id: FeatureId) -> Vec<Dimension> {
        let Some(f) = self.feature(id) else {
            return Vec::new();
        };
        match &f.kind {
            FeatureKind::Sketch(s) => s
                .sketch
                .constraints()
                .filter_map(|(cid, c)| {
                    let d = c.dimension.as_ref()?;
                    Some(Dimension {
                        feature: id,
                        slot: Slot::Dimension(cid),
                        name: d.name.clone(),
                        address: address(&d.name, &f.name),
                        field: None,
                        kind: if c.kind.is_angular() {
                            ScalarKind::Angle
                        } else {
                            ScalarKind::Length
                        },
                        value: Scalar {
                            value: d.value,
                            expression: d.expression.clone(),
                        },
                        driving: d.driving,
                    })
                })
                .collect(),
            kind => {
                let names = f.value_names();
                kind.slots()
                    .into_iter()
                    .zip(names)
                    .map(|((field, kind, value), (_, name))| Dimension {
                        feature: id,
                        slot: Slot::Field(field.to_owned()),
                        address: address(&name, &f.name),
                        name,
                        field: Some(field),
                        kind,
                        value: value.clone(),
                        driving: true,
                    })
                    .collect()
            }
        }
    }

    /// Every dimension of the part, in tree order.
    pub fn dimensions(&self) -> Vec<Dimension> {
        self.features()
            .flat_map(|f| self.dimensions_of(f.id))
            .collect()
    }

    /// The dimension at an address: `NAME@FEATURE` (`Width@Sketch1`, `d1@Extrude1`), a
    /// whole name (`D2@Sketch10`), or a feature's field (`depth@Extrude1`).
    ///
    /// As in SolidWorks, the case of the letters doesn't matter where only one dimension
    /// has the address in any case (`Width@Sketch1` finds `width@Sketch1`).
    pub fn find_dimension(&self, address: &str) -> Option<Dimension> {
        let address = address.trim();
        let all = self.dimensions();
        if let Some(d) = all.iter().find(|d| d.address == address) {
            return Some(d.clone());
        }
        let mut alike = all
            .iter()
            .filter(|d| d.address.eq_ignore_ascii_case(address));
        if let (Some(d), None) = (alike.next(), alike.next()) {
            return Some(d.clone());
        }
        let (field, feature) = address.split_once('@')?;
        all.into_iter()
            .find(|d| d.field == Some(field) && self.name_of(d.feature) == feature)
    }

    /// The dimension of a feature called `which`: by its name, or by its field.
    pub fn dimension_named(&self, feature: FeatureId, which: &str) -> Option<Dimension> {
        let all = self.dimensions_of(feature);
        all.iter()
            .find(|d| d.name == which)
            .or_else(|| all.iter().find(|d| d.field == Some(which)))
            .cloned()
    }

    /// Names a dimension of a feature (found by its name or its field): a sketch's
    /// dimension in the sketch, any other value in the feature. A name with `@` is a
    /// whole address, for a value of a feature that isn't a sketch.
    ///
    /// Expressions that read the dimension by its old name are not changed.
    pub fn name_dimension(
        &mut self,
        feature: FeatureId,
        which: &str,
        name: &str,
    ) -> Result<(), String> {
        let name = name.trim();
        let Some(d) = self.dimension_named(feature, which) else {
            let names: Vec<String> = self
                .dimensions_of(feature)
                .into_iter()
                .map(|d| d.name)
                .collect();
            return Err(format!(
                "{} has no dimension '{which}'. Its dimensions are: {}.",
                self.name_of(feature),
                if names.is_empty() {
                    "(none)".to_owned()
                } else {
                    names.join(", ")
                }
            ));
        };
        let sketch = d.field.is_none();
        expr::check_dimension_name(name, !sketch).map_err(|e| e.message)?;
        if d.name == name {
            return Ok(());
        }
        if self
            .dimensions_of(feature)
            .iter()
            .any(|o| o.slot != d.slot && o.name == name)
        {
            return Err(format!(
                "{} already has a dimension called '{name}'.",
                self.name_of(feature)
            ));
        }
        let whole = address(name, self.name_of(feature));
        if let Some(other) = self
            .dimensions()
            .into_iter()
            .find(|o| o.address == whole && o.slot != d.slot)
        {
            return Err(format!(
                "'{whole}' is already the address of a dimension of {}.",
                self.name_of(other.feature)
            ));
        }
        let Some(f) = self.feature_mut(feature) else {
            return Err("The feature no longer exists.".to_owned());
        };
        match (&d.slot, &mut f.kind) {
            (Slot::Dimension(id), FeatureKind::Sketch(s)) => {
                if let Some(dim) = s
                    .sketch
                    .constraint_mut(*id)
                    .and_then(|c| c.dimension.as_mut())
                {
                    name.clone_into(&mut dim.name);
                }
            }
            (Slot::Field(field), _) => {
                f.dimension_names.insert(field.clone(), name.to_owned());
            }
            _ => {}
        }
        Ok(())
    }

    /// The value of every dimension as it is now, worked out against the parameters (and
    /// the dimension values they have now).
    fn dimension_table(&self, components: &[(String, Quantity)]) -> DimensionValues {
        let mut table: BTreeMap<String, Quantity> = components.iter().cloned().collect();
        // An assembly's own: its mates' values and its reference dimensions.
        for (a, q) in self.assembly_values() {
            table.entry(a).or_insert(q);
        }
        for f in self.features() {
            match &f.kind {
                FeatureKind::Sketch(s) => {
                    for (cid, q) in expr::dimension_values(&s.sketch, &self.parameters) {
                        if let Some(d) = s.sketch.constraint(cid).and_then(|c| c.dimension.as_ref())
                        {
                            table.entry(address(&d.name, &f.name)).or_insert(q);
                        }
                    }
                }
                kind => {
                    for ((field, kind, value), (_, name)) in
                        kind.slots().into_iter().zip(f.value_names())
                    {
                        let v = value
                            .evaluate(kind, &self.parameters)
                            .unwrap_or(value.value);
                        let q = Quantity::new(v, quantity_kind(kind));
                        table.entry(address(&name, &f.name)).or_insert(q);
                        table.entry(format!("{field}@{}", f.name)).or_insert(q);
                    }
                }
            }
        }
        DimensionValues(table)
    }

    /// Works out the parameters and the value of every dimension, which expressions read
    /// by address. Dimensions and parameters may read each other: they are worked out
    /// again until nothing changes (a few rounds at most).
    ///
    /// In an assembly they are its components' dimensions and parameters (see
    /// [`crate::equations`]), its mates' values and its reference dimensions.
    pub fn refresh_dimension_values(&mut self) {
        let components = self.component_values();
        for round in 0.. {
            self.parameters.evaluate();
            let next = self.dimension_table(&components);
            let settled = next.same(&self.parameters.dimensions);
            self.parameters.dimensions = next;
            if settled {
                break;
            }
            if round == ROUNDS {
                self.parameters.evaluate();
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{Scalar, ScalarKind};
    use crate::{Operation, PlaneRef, StdPlane};
    use peet_math::DVec2;
    use peet_sketch::ConstraintKind;

    /// A sketch with a 100 × 50 rectangle whose width is `Width` and height `d2`, and an
    /// extrusion of it.
    fn part() -> (Model, FeatureId, FeatureId) {
        let mut m = Model::new();
        let s = m.add_sketch(PlaneRef::Standard(StdPlane::Top), StdPlane::Top.plane());
        {
            let sk = &mut m.feature_mut(s).unwrap().sketch_mut().unwrap().sketch;
            let a = sk.add_point(DVec2::new(0.0, 0.0));
            let b = sk.add_point(DVec2::new(100.0, 0.0));
            let c = sk.add_point(DVec2::new(100.0, 50.0));
            let w = sk
                .add_dimension(ConstraintKind::Distance(a, b), 100.0)
                .unwrap();
            sk.constraint_mut(w)
                .unwrap()
                .dimension
                .as_mut()
                .unwrap()
                .name = "Width".to_owned();
            sk.add_dimension(ConstraintKind::Distance(b, c), 50.0)
                .unwrap();
        }
        let e = m.add_extrude(s, Operation::NewBody);
        (m, s, e)
    }

    #[test]
    fn every_value_has_an_address() {
        let (m, s, e) = part();
        let all: Vec<String> = m.dimensions().into_iter().map(|d| d.address).collect();
        assert!(all.contains(&"Width@Sketch1".to_owned()), "{all:?}");
        assert!(all.contains(&"d2@Sketch1".to_owned()), "{all:?}");
        assert!(all.contains(&"d1@Extrude1".to_owned()), "{all:?}");
        assert_eq!(m.find_dimension("Width@Sketch1").unwrap().feature, s);
        // A field's name is an address too.
        let depth = m.find_dimension("depth@Extrude1").unwrap();
        assert_eq!((depth.feature, depth.name.as_str()), (e, "d1"));
        assert!(m.find_dimension("Height@Sketch1").is_none());
        // As in SolidWorks, in any case where only one has the address
        assert_eq!(m.find_dimension("width@sketch1").unwrap().feature, s);
        assert_eq!(m.find_dimension("D2@SKETCH1").unwrap().name, "d2");
    }

    #[test]
    fn a_name_can_be_a_whole_address() {
        let (mut m, _, e) = part();
        m.name_dimension(e, "depth", "D2@Sketch10").unwrap();
        let d = m.find_dimension("D2@Sketch10").unwrap();
        assert_eq!((d.feature, d.field), (e, Some("depth")));
        // A sketch's dimension has a name of its own, never a whole address.
        let s = m.find_dimension("Width@Sketch1").unwrap().feature;
        assert!(m.name_dimension(s, "Width", "a@b").is_err());
        assert!(m.name_dimension(s, "Width", "d2").is_err(), "taken");
        m.name_dimension(s, "Width", "Dia.").unwrap();
        assert!(m.find_dimension("Dia.@Sketch1").is_some());
    }

    #[test]
    fn expressions_read_dimensions_by_address() {
        let (mut m, _, e) = part();
        m.refresh_dimension_values();
        m.parameters.set("half", "\"Width@Sketch1\" / 2").unwrap();
        assert_eq!(m.parameters.get("half"), Some(50.0));
        // A feature's value follows another's.
        if let FeatureKind::Extrude(x) = &mut m.feature_mut(e).unwrap().kind {
            x.params.depth = Scalar {
                value: 0.0,
                expression: Some("d2@Sketch1 + half".to_owned()),
            };
        }
        m.refresh_dimension_values();
        let depth = m.find_dimension("d1@Extrude1").unwrap();
        assert_eq!(
            depth.value.evaluate(ScalarKind::Length, &m.parameters),
            Ok(100.0)
        );
        assert_eq!(
            m.parameters
                .dimensions
                .get("depth@Extrude1")
                .map(|q| q.value),
            Some(100.0)
        );
    }
}
