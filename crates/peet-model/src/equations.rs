//! An assembly's equations, and what they read: its components' dimensions, as SolidWorks
//! has them.
//!
//! **Reading.** An expression in an assembly (a parameter, a mate's distance, an equation)
//! reads a dimension of a component's part by the part's address followed by the
//! component's name: `"Height@Sketch1@Door-1"`, and a parameter (global variable) of it by
//! its name and the component's, `"Thickness@Box Roof Top-1"`. SolidWorks' own forms read
//! the same (`"Height@Sketch1@Door<1>.Part"`, `Door-1.Part`: see
//! [`peet_sketch::expr::solidworks_address`]). A part inside a sub-assembly is reached
//! through it: `Height@Sketch1@Door-1@Door Assembly-1`. The assembly's mates' values are
//! `D1@` the mate's name, and a reference dimension's `RD1@Annotations`.
//!
//! **Setting.** A component's part is shared by every instance of it, so setting one of its
//! dimensions from the assembly changes the part and so every instance, as SolidWorks
//! changes the part's document. An [`Equation`] does so at every rebuild, from an expression
//! worked out in the assembly. Another sets whether a component is suppressed.
//!
//! **Reference dimensions** ([`ReferenceDimension`]) measure a distance between two
//! components' geometry once the mates are solved, for equations to read. A part changed by
//! an equation that reads one is rebuilt, and the mates solved again, until the values hold
//! (a few rounds at most).

use std::collections::HashMap;
use std::sync::Arc;

use peet_math::DVec3;
use peet_sketch::expr::{self, Quantity, QuantityKind};
use serde::{Deserialize, Serialize};

use crate::Model;
use crate::assembly::CompId;
use crate::config::Slot;
use crate::feature::{FeatureKind, Scalar, ScalarKind};
use crate::mate::{Located, MateEnd};

/// What an assembly equation sets. An assembly has at most one equation per target in each
/// configuration.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EquationTarget {
    /// A dimension or a parameter of a component's part, by its address in the part
    /// (`Height@Sketch1`, `Thickness`), or in a sub-assembly by its address there.
    Dimension { component: CompId, address: String },
    /// Whether the component is suppressed: nonzero suppresses, 0 unsuppresses (as
    /// SolidWorks' `suppressed` and `unsuppressed` are 1 and 0).
    Suppression(CompId),
}

impl EquationTarget {
    /// The component of the assembly it sets something of.
    pub fn component(&self) -> CompId {
        match self {
            Self::Dimension { component, .. } | Self::Suppression(component) => *component,
        }
    }
}

/// `target = expression`, worked out in the assembly at every rebuild.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Equation {
    pub target: EquationTarget,
    pub expression: String,
}

/// A distance measured in an assembly between geometry of two components: two parallel
/// planes, a point and a plane, two points, a point and a line, or two lines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReferenceDimension {
    /// Unique in the assembly: `RD1`. Expressions read it as `RD1@Annotations`.
    pub name: String,
    pub a: MateEnd,
    pub b: MateEnd,
    /// What it measured at the last rebuild (mm), if it could be measured.
    pub value: Option<f64>,
}

/// The second part of a reference dimension's address, as in SolidWorks: `RD1@Annotations`.
pub const ANNOTATIONS: &str = "Annotations";

/// The most rounds of setting parts from equations that read each other.
const ROUNDS: usize = 4;

/// The distance between two located ends (mm), or why there is none.
pub(crate) fn measure(a: &Located, b: &Located) -> Result<f64, String> {
    const PARALLEL: f64 = 1e-6;
    let off_line = |q: DVec3, p: DVec3, d: DVec3| (q - p).cross(d.normalize_or_zero()).length();
    Ok(match (a, b) {
        (Located::Plane(p, n), Located::Plane(q, m)) => {
            if n.normalize_or_zero().cross(m.normalize_or_zero()).length() > PARALLEL {
                return Err(
                    "The two planes are not parallel: there is no one distance between them."
                        .to_owned(),
                );
            }
            (*q - *p).dot(n.normalize_or_zero()).abs()
        }
        (Located::Plane(p, n), Located::Point(q)) | (Located::Point(q), Located::Plane(p, n)) => {
            (*q - *p).dot(n.normalize_or_zero()).abs()
        }
        (Located::Plane(p, n), Located::Line(q, d))
        | (Located::Line(q, d), Located::Plane(p, n)) => {
            if n.normalize_or_zero().dot(d.normalize_or_zero()).abs() > PARALLEL {
                return Err(
                    "The line crosses the plane: there is no one distance between them.".to_owned(),
                );
            }
            (*q - *p).dot(n.normalize_or_zero()).abs()
        }
        (Located::Point(p), Located::Point(q)) => (*q - *p).length(),
        (Located::Line(p, d), Located::Point(q)) | (Located::Point(q), Located::Line(p, d)) => {
            off_line(*q, *p, *d)
        }
        (Located::Line(p, d), Located::Line(q, e)) => {
            let across = d.normalize_or_zero().cross(e.normalize_or_zero());
            if across.length() <= PARALLEL {
                off_line(*q, *p, *d)
            } else {
                (*q - *p).dot(across.normalize()).abs()
            }
        }
    })
}

fn quantity_kind(kind: ScalarKind) -> QuantityKind {
    match kind {
        ScalarKind::Length => QuantityKind::Length,
        ScalarKind::Angle => QuantityKind::Angle,
        ScalarKind::Number => QuantityKind::Number,
    }
}

fn scalar_kind(kind: QuantityKind) -> ScalarKind {
    match kind {
        QuantityKind::Length => ScalarKind::Length,
        QuantityKind::Angle => ScalarKind::Angle,
        QuantityKind::Number => ScalarKind::Number,
    }
}

/// A value as a literal input of its kind, in base units: `308mm`, `30deg`, `4`.
fn literal(value: f64, kind: ScalarKind) -> String {
    match kind {
        ScalarKind::Length => format!("{value}mm"),
        ScalarKind::Angle => format!("{value}deg"),
        ScalarKind::Number => format!("{value}"),
    }
}

impl Model {
    /// The component of this assembly an address reaches into, and the address in the
    /// component's part (or sub-assembly): `Height@Sketch1@Door-1` is `Door-1` and
    /// `Height@Sketch1`. The component is its last part, named as PeetCAD names it or as
    /// SolidWorks writes it (`Door<1>.Part`), in any case where only one component has the
    /// name in any case. `None` if the model is not an assembly or no component has the
    /// name.
    pub fn component_address(&self, address: &str) -> Option<(CompId, String)> {
        let assembly = self.assembly()?;
        let (rest, last) = address.trim().rsplit_once('@')?;
        let rest = rest.trim();
        if rest.is_empty() {
            return None;
        }
        let name = expr::solidworks_component(last);
        let exact = assembly.components().find(|c| c.name == name);
        let id = match exact {
            Some(c) => c.id,
            None => {
                let mut alike = assembly
                    .components()
                    .filter(|c| c.name.eq_ignore_ascii_case(&name));
                match (alike.next(), alike.next()) {
                    (Some(c), None) => c.id,
                    _ => return None,
                }
            }
        };
        Some((id, rest.to_owned()))
    }

    /// The kind of value at an address: a dimension or parameter of this part, or of a
    /// component's part through the components it names.
    pub fn kind_at(&self, address: &str) -> Option<ScalarKind> {
        if self.is_assembly() {
            let (c, rest) = self.component_address(address)?;
            return self.assembly()?.component_model(c)?.kind_at(&rest);
        }
        let name = address.trim().trim_matches('"').trim();
        if let Some(p) = self.parameters.entries.iter().find(|p| p.name == name) {
            return Some(p.quantity().map_or(ScalarKind::Number, |q| {
                q.kind().map_or(ScalarKind::Number, scalar_kind)
            }));
        }
        self.find_dimension(name).map(|d| d.kind)
    }

    /// Sets the dimension or parameter at `address` to a value in base units (mm, degrees
    /// or the plain number), as `set` does with a number. In an assembly the address
    /// reaches into a component (see [`Model::component_address`]), and the component's
    /// part is changed, and so every instance of it. Returns whether anything changed.
    pub fn set_value_at(&mut self, address: &str, value: f64) -> Result<bool, String> {
        if !value.is_finite() {
            return Err(format!("{address}: the value is not a finite number."));
        }
        if self.is_assembly() {
            let (c, rest) = self.component_address(address).ok_or_else(|| {
                format!("No component of the assembly is named at the end of '{address}'.")
            })?;
            let assembly = self.assembly().expect("an assembly");
            let def = assembly
                .definition_of(c)
                .ok_or_else(|| "The component's part is gone.".to_owned())?;
            let id = def.id;
            let mut part = (*def.model).clone();
            // In the configuration of its part the component shows, as SolidWorks sets it.
            let shown = assembly
                .component(c)
                .and_then(|x| assembly.configuration_of(x));
            let active = part.active_configuration().id;
            if let Some(shown) = shown {
                part.activate_configuration(shown);
            }
            let changed = part
                .set_value_at(&rest, value)
                .map_err(|e| format!("{} (in {}): {e}", rest, assembly.name_of(c)));
            if shown.is_some() {
                part.activate_configuration(active);
            }
            if !changed? {
                return Ok(false);
            }
            self.assembly_mut()
                .expect("an assembly")
                .set_model(id, Arc::new(part));
            return Ok(true);
        }
        let name = address.trim().trim_matches('"').trim();
        if let Some(p) = self.parameters.entries.iter().find(|p| p.name == name) {
            let kind = p.quantity().map_or(ScalarKind::Number, |q| {
                q.kind().map_or(ScalarKind::Number, scalar_kind)
            });
            if p.expression.trim() == literal(value, kind) {
                return Ok(false);
            }
            self.set_parameter(
                name,
                &literal(value, kind),
                &crate::config::Scope::default(),
            )
            .map_err(|e| format!("{name}: {e}"))?;
            return Ok(true);
        }
        let d = self.find_dimension(name).ok_or_else(|| {
            format!(
                "The part {} has no dimension or parameter '{name}'.",
                self.name
            )
        })?;
        if !d.driving {
            return Err(format!(
                "{} is a reference dimension: it measures the sketch and can't be set.",
                d.address
            ));
        }
        if d.value.expression.is_none() && d.value.value.to_bits() == value.to_bits() {
            return Ok(false);
        }
        let units = self.parameters.clone();
        let feature = self
            .feature_mut(d.feature)
            .ok_or_else(|| "The feature is gone.".to_owned())?;
        match (&d.slot, &mut feature.kind) {
            (Slot::Dimension(id), FeatureKind::Sketch(s)) => {
                expr::set_dimension_input(&mut s.sketch, &units, *id, &literal(value, d.kind))
                    .map_err(|e| format!("{}: {}", d.address, e.message))?;
            }
            (Slot::Field(field), kind) => {
                if let Some((.., s)) = kind.slots_mut().into_iter().find(|(n, ..)| n == field) {
                    *s = Scalar::new(value);
                }
            }
            _ => return Err(format!("{} can't be set.", d.address)),
        }
        Ok(true)
    }

    /// The values an assembly's expressions read of its components: each dimension of a
    /// component's part at its address followed by the component's name, and each of its
    /// parameters by its name and the component's.
    pub(crate) fn component_values(&self) -> Vec<(String, Quantity)> {
        let Some(assembly) = self.assembly() else {
            return Vec::new();
        };
        type Table = Vec<(String, Quantity)>;
        let mut tables: HashMap<(crate::DefId, Option<crate::ConfigId>), Table> = HashMap::new();
        let mut out = Vec::new();
        for c in assembly.components() {
            // In the configuration the component shows.
            let key = (c.definition, assembly.configuration_of(c));
            let Some(model) = assembly.model_of(c) else {
                continue;
            };
            let table = tables.entry(key).or_insert_with(|| {
                let mut part = (*model).clone();
                part.refresh_dimension_values();
                let mut table: Vec<(String, Quantity)> = part
                    .parameters
                    .entries
                    .iter()
                    .filter_map(|p| Some((p.name.clone(), p.quantity()?)))
                    .collect();
                table.extend(
                    part.parameters
                        .dimensions
                        .0
                        .iter()
                        .map(|(a, q)| (a.clone(), *q)),
                );
                table
            });
            out.extend(table.iter().map(|(a, q)| (format!("{a}@{}", c.name), *q)));
        }
        out
    }

    /// The values an assembly's own expressions read of it: each mate's distance or
    /// angle as `D1@` the mate's name, each reference dimension as `RD1@Annotations`.
    pub(crate) fn assembly_values(&self) -> Vec<(String, Quantity)> {
        let Some(assembly) = self.assembly() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for m in assembly.mates() {
            let (value, kind) = match &m.kind {
                crate::MateKind::Distance(s) => (s, ScalarKind::Length),
                crate::MateKind::Angle(s) => (s, ScalarKind::Angle),
                _ => continue,
            };
            if let Ok(v) = value.evaluate(kind, &self.parameters) {
                out.push((
                    format!("D1@{}", m.name),
                    Quantity::new(v, quantity_kind(kind)),
                ));
            }
        }
        // A component pattern's counts and spacings (or angle), by SolidWorks' names:
        // `D1@LocalLPattern1` (count), `D3@LocalLPattern1` (spacing).
        for p in assembly.patterns() {
            for (name, value, kind) in p.kind.values() {
                let v = if kind == ScalarKind::Number {
                    // The count the copies are made for.
                    Ok(f64::from(crate::assembly::pattern_count(value)))
                } else {
                    value.evaluate(kind, &self.parameters)
                };
                if let Ok(v) = v {
                    out.push((
                        format!("{name}@{}", p.name),
                        Quantity::new(v, quantity_kind(kind)),
                    ));
                }
            }
        }
        for r in assembly.references() {
            if let Some(v) = r.value {
                out.push((
                    format!("{}@{ANNOTATIONS}", r.name),
                    Quantity::new(v, QuantityKind::Length),
                ));
            }
        }
        out
    }

    /// Works out the assembly's equations and sets what they set: its components' parts'
    /// dimensions, and whether components are suppressed. Equations that read what others
    /// set are worked out again until nothing changes (a few rounds at most). Returns the
    /// equations that can't be worked out (by their place in the list), with why.
    pub fn apply_equations(&mut self) -> Vec<(usize, String)> {
        let mut failures = Vec::new();
        let Some(assembly) = self.assembly() else {
            return failures;
        };
        if assembly.equations().is_empty() {
            return failures;
        }
        for _ in 0..ROUNDS {
            failures.clear();
            let mut changed = false;
            let equations = self
                .assembly()
                .map(|a| a.equations().to_vec())
                .unwrap_or_default();
            for (i, e) in equations.iter().enumerate() {
                match self.apply_equation(e) {
                    Ok(c) => changed |= c,
                    Err(why) => failures.push((i, why)),
                }
            }
            if !changed {
                break;
            }
            self.refresh_dimension_values();
        }
        failures
    }

    /// Works out the counts of the assembly's component patterns that are expressions,
    /// and makes or removes copies to match. Returns whether any pattern changed.
    pub fn sync_pattern_counts(&mut self) -> bool {
        let driven = self.assembly().is_some_and(|a| {
            a.patterns().any(|p| {
                p.kind
                    .counts()
                    .into_iter()
                    .flatten()
                    .any(|c| c.expression.is_some())
            })
        });
        if !driven {
            return false;
        }
        let params = self.parameters.clone();
        let changed = self
            .assembly_mut()
            .is_some_and(|a| a.sync_pattern_counts(&params));
        if changed {
            self.refresh_dimension_values();
        }
        changed
    }

    fn apply_equation(&mut self, e: &Equation) -> Result<bool, String> {
        let assembly = self.assembly().ok_or("not an assembly")?;
        let name = assembly.name_of(e.target.component()).to_owned();
        match &e.target {
            EquationTarget::Suppression(c) => {
                let q = self
                    .parameters
                    .evaluate_expression(&e.expression)
                    .map_err(|err| format!("{name}: {err}"))?;
                if !q.dim.is_none() || !q.value.is_finite() {
                    return Err(format!(
                        "{name}: a suppression equation must give a plain number: 0 unsuppresses, anything else suppresses."
                    ));
                }
                let on = q.value != 0.0;
                // An equation of some configurations only suppresses in the active one, as
                // SolidWorks keeps a component's suppression per configuration; one of all
                // of them suppresses where the component's state is shared.
                if self.equation_differs(&e.target) {
                    let was = self
                        .assembly()
                        .and_then(|a| a.component(*c))
                        .map(|x| x.suppressed);
                    if was == Some(on) || was.is_none() {
                        return Ok(false);
                    }
                    self.set_component(*c, &crate::config::Scope::This, |s| s.suppressed = on)?;
                    return Ok(true);
                }
                let assembly = self.assembly_mut().expect("an assembly");
                match assembly.component_mut(*c) {
                    Some(comp) if comp.suppressed != on => {
                        comp.suppressed = on;
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            }
            EquationTarget::Dimension { component, address } => {
                let def = assembly
                    .definition_of(*component)
                    .ok_or_else(|| format!("{name}'s part is gone."))?;
                let kind = def.model.kind_at(address).ok_or_else(|| {
                    format!("{name}'s part has no dimension or parameter '{address}'.")
                })?;
                let value = Scalar {
                    value: 0.0,
                    expression: Some(e.expression.clone()),
                }
                .evaluate(kind, &self.parameters)
                .or_else(|err| {
                    // A count worked out from lengths is the number SolidWorks has in
                    // its units.
                    match (kind, self.parameters.evaluate_expression(&e.expression)) {
                        (ScalarKind::Number, Ok(q)) if q.value.is_finite() => Ok(q.value),
                        _ => Err(err),
                    }
                })
                .map_err(|err| format!("{address}@{name}: {err}"))?;
                let target = format!("{address}@{name}");
                self.set_value_at(&target, value)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Operation, PlaneRef, StdPlane};
    use peet_math::{DVec2, Frame};
    use peet_sketch::ConstraintKind;

    /// A plate whose sketch has `Width` (100) and `Height` (50), extruded 5.
    fn plate() -> Model {
        let mut m = Model::new();
        m.name = "Plate".to_owned();
        let s = m.add_sketch(PlaneRef::Standard(StdPlane::Top), StdPlane::Top.plane());
        {
            let sk = &mut m.feature_mut(s).unwrap().sketch_mut().unwrap().sketch;
            let a = sk.add_point(DVec2::new(0.0, 0.0));
            let b = sk.add_point(DVec2::new(100.0, 0.0));
            let c = sk.add_point(DVec2::new(100.0, 50.0));
            for (from, to, value, name) in [(a, b, 100.0, "Width"), (b, c, 50.0, "Height")] {
                let id = sk
                    .add_dimension(ConstraintKind::Distance(from, to), value)
                    .unwrap();
                sk.constraint_mut(id)
                    .unwrap()
                    .dimension
                    .as_mut()
                    .unwrap()
                    .name = name.to_owned();
            }
        }
        m.add_extrude(s, Operation::NewBody);
        m.parameters.set("Thickness", "3mm").unwrap();
        m
    }

    fn assembly() -> Model {
        let mut m = Model::new_assembly();
        let a = m.assembly_mut().unwrap();
        let first = a.define(Arc::new(plate()));
        a.insert(first, Frame::WORLD).unwrap();
        a.insert(first, Frame::WORLD).unwrap();
        let other = {
            let mut p = plate();
            p.name = "Cover".to_owned();
            a.define(Arc::new(p))
        };
        a.insert(other, Frame::WORLD).unwrap();
        m
    }

    #[test]
    fn an_assembly_reads_its_components_dimensions() {
        let mut m = assembly();
        m.refresh_dimension_values();
        let read = |m: &Model, e: &str| m.parameters.evaluate_expression(e).map(|q| q.value);
        assert_eq!(read(&m, "\"Width@Sketch1@Plate-1\""), Ok(100.0));
        assert_eq!(read(&m, "\"Height@Sketch1@Plate<2>.Part\""), Ok(50.0));
        assert_eq!(read(&m, "\"Thickness@Cover-1.Part\""), Ok(3.0));
        assert!(read(&m, "\"Width@Sketch1@Plate-9\"").is_err());
        assert_eq!(
            m.component_address("Width@Sketch1@Plate<2>.Part")
                .map(|(_, a)| a),
            Some("Width@Sketch1".to_owned())
        );
    }

    #[test]
    fn setting_a_components_dimension_changes_its_part() {
        let mut m = assembly();
        assert_eq!(m.set_value_at("Width@Sketch1@Plate-2", 140.0), Ok(true));
        assert_eq!(m.set_value_at("Width@Sketch1@Plate-2", 140.0), Ok(false));
        m.refresh_dimension_values();
        let read = |m: &Model, e: &str| m.parameters.evaluate_expression(e).map(|q| q.value);
        // Both instances share the part, as in SolidWorks; the other part is not touched.
        assert_eq!(read(&m, "\"Width@Sketch1@Plate-1\""), Ok(140.0));
        assert_eq!(read(&m, "\"Width@Sketch1@Cover-1\""), Ok(100.0));
        assert_eq!(m.set_value_at("Thickness@Cover-1", 2.0), Ok(true));
        m.refresh_dimension_values();
        assert_eq!(read(&m, "\"Thickness@Cover-1\""), Ok(2.0));
        assert!(m.set_value_at("Nothing@Sketch1@Cover-1", 2.0).is_err());
    }

    #[test]
    fn equations_set_dimensions_and_suppression() {
        let mut m = assembly();
        let plate = m.component_address("x@Plate-1").unwrap().0;
        let cover = m.component_address("x@Cover-1").unwrap().0;
        let a = m.assembly_mut().unwrap();
        a.equations_mut().push(Equation {
            target: EquationTarget::Dimension {
                component: cover,
                address: "Width@Sketch1".to_owned(),
            },
            expression: "\"Width@Sketch1@Plate<1>.Part\" - 50".to_owned(),
        });
        a.equations_mut().push(Equation {
            target: EquationTarget::Suppression(plate),
            expression: "iif(\"Width@Sketch1@Cover-1\" > 60, 1, 0)".to_owned(),
        });
        m.refresh_dimension_values();
        assert!(m.apply_equations().is_empty());
        let read = |m: &Model, e: &str| m.parameters.evaluate_expression(e).map(|q| q.value);
        assert_eq!(read(&m, "\"Width@Sketch1@Cover-1\""), Ok(50.0));
        let suppressed = |m: &Model| m.assembly().unwrap().component(plate).unwrap().suppressed;
        assert!(!suppressed(&m));
        m.set_value_at("Width@Sketch1@Plate-1", 200.0).unwrap();
        m.refresh_dimension_values();
        assert!(m.apply_equations().is_empty());
        assert_eq!(read(&m, "\"Width@Sketch1@Cover-1\""), Ok(150.0));
        assert!(suppressed(&m));
        // An equation that can't be worked out says so, and sets nothing.
        m.assembly_mut().unwrap().equations_mut()[0].expression = "\"Nothing@Plate-1\"".to_owned();
        let failures = m.apply_equations();
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert_eq!(read(&m, "\"Width@Sketch1@Cover-1\""), Ok(150.0));
    }

    #[test]
    fn equations_differ_between_configurations() {
        use crate::config::Scope;
        let mut m = assembly();
        let plate = m.component_address("x@Plate-1").unwrap().0;
        let cover = m.component_address("x@Cover-1").unwrap().0;
        let width = EquationTarget::Dimension {
            component: cover,
            address: "Width@Sketch1".to_owned(),
        };
        let suppress = EquationTarget::Suppression(plate);
        let first = m.active_configuration().id;
        // Everywhere, then another value in a second configuration, and an equation there only.
        m.set_equation(&width, Some("80".to_owned()), &Scope::All)
            .unwrap();
        let second = m.add_configuration("Wide", None).unwrap();
        assert!(!m.equation_differs(&width));
        m.set_equation(&width, Some("120".to_owned()), &Scope::Only(vec![second]))
            .unwrap();
        m.set_equation(&suppress, Some("1".to_owned()), &Scope::Only(vec![second]))
            .unwrap();
        assert!(m.equation_differs(&width) && m.equation_differs(&suppress));
        assert_eq!(m.equation_in(&width, first).as_deref(), Some("80"));
        assert_eq!(m.equation_in(&width, second).as_deref(), Some("120"));
        assert_eq!(m.equation_in(&suppress, first), None);
        assert_eq!(m.equation_targets(), vec![width.clone(), suppress.clone()]);
        assert_eq!(m.assembly().unwrap().equations().len(), 1);
        let read = |m: &Model, e: &str| m.parameters.evaluate_expression(e).map(|q| q.value);
        let suppressed = |m: &Model| m.assembly().unwrap().component(plate).unwrap().suppressed;
        let rebuild = |m: &mut Model| {
            m.refresh_dimension_values();
            assert!(m.apply_equations().is_empty());
            m.refresh_dimension_values();
        };
        rebuild(&mut m);
        assert_eq!(read(&m, "\"Width@Sketch1@Cover-1\""), Ok(80.0));
        assert!(!suppressed(&m));
        // Each configuration works out its own.
        m.activate_configuration(second);
        assert_eq!(m.assembly().unwrap().equations().len(), 2);
        rebuild(&mut m);
        assert_eq!(read(&m, "\"Width@Sketch1@Cover-1\""), Ok(120.0));
        assert!(suppressed(&m));
        m.activate_configuration(first);
        rebuild(&mut m);
        assert_eq!(read(&m, "\"Width@Sketch1@Cover-1\""), Ok(80.0));
        // The same everywhere again: nothing differs.
        m.set_equation(&width, Some("90".to_owned()), &Scope::All)
            .unwrap();
        m.set_equation(&suppress, None, &Scope::All).unwrap();
        assert!(!m.equation_differs(&width) && !m.equation_differs(&suppress));
        assert_eq!(m.equation_in(&width, second).as_deref(), Some("90"));
        // Deleting the component forgets its equations in every configuration.
        m.set_equation(&suppress, Some("0".to_owned()), &Scope::This)
            .unwrap();
        m.assembly_mut().unwrap().remove(plate);
        m.tidy_configurations();
        assert!(!m.equation_differs(&suppress));
    }

    #[test]
    fn distances_are_measured_between_ends() {
        let z = DVec3::Z;
        let plane = |h: f64| Located::Plane(DVec3::new(1.0, 2.0, h), z);
        assert_eq!(measure(&plane(0.0), &plane(308.0)), Ok(308.0));
        assert_eq!(
            measure(&plane(5.0), &Located::Point(DVec3::new(9.0, 9.0, -3.0))),
            Ok(8.0)
        );
        assert!(measure(&plane(0.0), &Located::Plane(DVec3::ZERO, DVec3::X)).is_err());
        let line = Located::Line(DVec3::ZERO, DVec3::Z);
        assert_eq!(
            measure(&line, &Located::Point(DVec3::new(3.0, 4.0, 7.0))),
            Ok(5.0)
        );
        assert_eq!(
            measure(&line, &Located::Line(DVec3::new(0.0, 6.0, 0.0), DVec3::Z)),
            Ok(6.0)
        );
    }
}
