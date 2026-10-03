//! Configurations: named versions of one part that differ in which features are
//! suppressed and in the values of its parameters.
//!
//! **The model always holds the active configuration.** A feature's `suppressed` flag and
//! the parameter table are what the active configuration says, so everything that reads
//! the model (the rebuild engine, the queries, the exports, the application's panels)
//! sees one ordinary part and needs to know nothing about configurations.
//!
//! **What differs is kept on the side.** For each feature whose suppression differs
//! between configurations, and each parameter whose expression does, [`Configurations`]
//! keeps the value in every configuration *other than the active one*. Making another
//! configuration active swaps those values with the model's. A value that is not listed
//! is the same in every configuration.
//!
//! **A change has a [`Scope`]**: this configuration, all of them, or the ones named. With
//! a single configuration the scopes are the same thing.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::Model;
use crate::feature::FeatureId;

/// Stable identifier of a configuration within its model. Never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ConfigId(pub u32);

/// A named version of the part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Configuration {
    pub id: ConfigId,
    /// Unique in the model.
    pub name: String,
    /// What the configuration is for, in the user's words.
    pub comment: String,
}

/// Which configurations a change applies to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    /// The active configuration only.
    #[default]
    This,
    /// Every configuration: the value no longer differs between them.
    All,
    /// These configurations.
    Only(Vec<ConfigId>),
}

/// The name of the configuration every part starts with.
pub const DEFAULT_NAME: &str = "Default";

/// A part's configurations, and what differs between them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Configurations {
    /// In the order they are listed. Never empty.
    list: Vec<Configuration>,
    active: ConfigId,
    next_id: u32,
    /// Features whose suppression differs: whether each is suppressed in every
    /// configuration other than the active one.
    suppressed: BTreeMap<FeatureId, BTreeMap<ConfigId, bool>>,
    /// Parameters whose expression differs: the expression in every configuration other
    /// than the active one.
    parameters: BTreeMap<String, BTreeMap<ConfigId, String>>,
}

impl Default for Configurations {
    fn default() -> Self {
        Self {
            list: vec![Configuration {
                id: ConfigId(1),
                name: DEFAULT_NAME.to_owned(),
                comment: String::new(),
            }],
            active: ConfigId(1),
            next_id: 2,
            suppressed: BTreeMap::new(),
            parameters: BTreeMap::new(),
        }
    }
}

impl Configurations {
    fn others(&self) -> Vec<ConfigId> {
        self.list
            .iter()
            .map(|c| c.id)
            .filter(|c| *c != self.active)
            .collect()
    }

    fn contains(&self, id: ConfigId) -> bool {
        self.list.iter().any(|c| c.id == id)
    }
}

/// Drops the entries of `table` that say nothing: for things that no longer exist, and
/// those whose value is the same in every configuration. Entries are completed first, so
/// every one has a value for each of `others`.
fn tidy<K: Ord + Clone, V: Clone + PartialEq>(
    table: &mut BTreeMap<K, BTreeMap<ConfigId, V>>,
    others: &[ConfigId],
    current: impl Fn(&K) -> Option<V>,
) {
    table.retain(|key, values| {
        let Some(now) = current(key) else {
            return false;
        };
        values.retain(|c, _| others.contains(c));
        for c in others {
            values.entry(*c).or_insert_with(|| now.clone());
        }
        values.values().any(|v| *v != now)
    });
}

impl Model {
    // ---- Reading ----

    /// The part's configurations, in the order they are listed. There is always one.
    pub fn configurations(&self) -> &[Configuration] {
        &self.configurations.list
    }

    /// The configuration the model holds: the one that is built and shown.
    pub fn active_configuration(&self) -> &Configuration {
        let c = &self.configurations;
        c.list
            .iter()
            .find(|x| x.id == c.active)
            .unwrap_or(&c.list[0])
    }

    pub fn configuration(&self, id: ConfigId) -> Option<&Configuration> {
        self.configurations.list.iter().find(|c| c.id == id)
    }

    pub fn configuration_named(&self, name: &str) -> Option<&Configuration> {
        self.configurations.list.iter().find(|c| c.name == name)
    }

    /// Whether the feature's suppression differs between configurations.
    pub fn suppression_differs(&self, feature: FeatureId) -> bool {
        self.configurations.suppressed.contains_key(&feature)
    }

    /// Whether the feature is suppressed in a configuration.
    pub fn suppressed_in(&self, feature: FeatureId, config: ConfigId) -> Option<bool> {
        let now = self.feature(feature)?.suppressed;
        if config == self.configurations.active {
            return Some(now);
        }
        Some(
            self.configurations
                .suppressed
                .get(&feature)
                .and_then(|v| v.get(&config))
                .copied()
                .unwrap_or(now),
        )
    }

    /// The features whose suppression differs between configurations, in tree order.
    pub fn features_that_differ(&self) -> Vec<FeatureId> {
        self.features()
            .map(|f| f.id)
            .filter(|id| self.suppression_differs(*id))
            .collect()
    }

    /// The parameters whose expression differs between configurations, in table order.
    pub fn parameters_that_differ(&self) -> Vec<&str> {
        self.parameters
            .entries
            .iter()
            .map(|p| p.name.as_str())
            .filter(|n| self.parameter_differs(n))
            .collect()
    }

    /// Whether the parameter's expression differs between configurations.
    pub fn parameter_differs(&self, name: &str) -> bool {
        self.configurations.parameters.contains_key(name)
    }

    /// The parameter's expression in a configuration.
    pub fn parameter_in(&self, name: &str, config: ConfigId) -> Option<&str> {
        let now = self
            .parameters
            .entries
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.expression.as_str())?;
        if config == self.configurations.active {
            return Some(now);
        }
        Some(
            self.configurations
                .parameters
                .get(name)
                .and_then(|v| v.get(&config))
                .map_or(now, String::as_str),
        )
    }

    /// The model with another configuration active: that configuration as an ordinary
    /// part, to build, measure or export without changing which one is shown. Cheap: the
    /// features are shared.
    pub fn with_configuration(&self, id: ConfigId) -> Self {
        let mut m = self.clone();
        m.activate_configuration(id);
        m
    }

    // ---- Editing ----

    fn check_configuration_name(&self, name: &str, except: Option<ConfigId>) -> Result<(), String> {
        if name.is_empty() {
            return Err("A configuration needs a name.".to_owned());
        }
        if self
            .configurations
            .list
            .iter()
            .any(|c| Some(c.id) != except && c.name == name)
        {
            return Err(format!("Another configuration is already called '{name}'."));
        }
        Ok(())
    }

    /// Adds a configuration that starts as a copy of `copy_of` (the active one if `None`).
    /// It is not made active.
    pub fn add_configuration(
        &mut self,
        name: &str,
        copy_of: Option<ConfigId>,
    ) -> Result<ConfigId, String> {
        let name = name.trim();
        self.check_configuration_name(name, None)?;
        let source = copy_of.unwrap_or(self.configurations.active);
        if !self.configurations.contains(source) {
            return Err("The configuration to copy no longer exists.".to_owned());
        }
        let id = ConfigId(self.configurations.next_id);
        // What differs takes the source's value in the new configuration.
        let flags: Vec<(FeatureId, bool)> = self
            .configurations
            .suppressed
            .keys()
            .filter_map(|f| Some((*f, self.suppressed_in(*f, source)?)))
            .collect();
        let exprs: Vec<(String, String)> = self
            .configurations
            .parameters
            .keys()
            .filter_map(|n| Some((n.clone(), self.parameter_in(n, source)?.to_owned())))
            .collect();
        let c = &mut self.configurations;
        for (f, v) in flags {
            c.suppressed.entry(f).or_default().insert(id, v);
        }
        for (n, v) in exprs {
            c.parameters.entry(n).or_default().insert(id, v);
        }
        c.next_id += 1;
        c.list.push(Configuration {
            id,
            name: name.to_owned(),
            comment: String::new(),
        });
        Ok(id)
    }

    pub fn rename_configuration(&mut self, id: ConfigId, name: &str) -> Result<(), String> {
        let name = name.trim();
        self.check_configuration_name(name, Some(id))?;
        match self.configurations.list.iter_mut().find(|c| c.id == id) {
            Some(c) => {
                name.clone_into(&mut c.name);
                Ok(())
            }
            None => Err("The configuration no longer exists.".to_owned()),
        }
    }

    pub fn set_configuration_comment(&mut self, id: ConfigId, comment: &str) {
        if let Some(c) = self.configurations.list.iter_mut().find(|c| c.id == id) {
            comment.trim().clone_into(&mut c.comment);
        }
    }

    /// Removes a configuration. A part keeps at least one; removing the active one makes
    /// its neighbour in the list active.
    pub fn remove_configuration(&mut self, id: ConfigId) -> Result<(), String> {
        let Some(index) = self.configurations.list.iter().position(|c| c.id == id) else {
            return Err("The configuration no longer exists.".to_owned());
        };
        if self.configurations.list.len() == 1 {
            return Err(
                "A part has at least one configuration: this is the only one, so it stays."
                    .to_owned(),
            );
        }
        if self.configurations.active == id {
            let next = if index > 0 { index - 1 } else { 1 };
            let next = self.configurations.list[next].id;
            self.activate_configuration(next);
        }
        self.configurations.list.remove(index);
        self.tidy_configurations();
        Ok(())
    }

    /// Makes a configuration the one the model holds. Returns whether that changed
    /// anything (false if it is active already, or doesn't exist).
    pub fn activate_configuration(&mut self, id: ConfigId) -> bool {
        let old = self.configurations.active;
        if id == old || !self.configurations.contains(id) {
            return false;
        }
        let mut tables = std::mem::take(&mut self.configurations.suppressed);
        for (feature, values) in &mut tables {
            let Some(index) = self.index_of(*feature) else {
                continue;
            };
            let now = self.features[index].suppressed;
            let next = values.remove(&id).unwrap_or(now);
            values.insert(old, now);
            if next != now {
                Arc::make_mut(&mut self.features[index]).suppressed = next;
            }
        }
        self.configurations.suppressed = tables;
        for (name, values) in &mut self.configurations.parameters {
            let Some(p) = self.parameters.entries.iter_mut().find(|p| p.name == *name) else {
                continue;
            };
            let next = values.remove(&id).unwrap_or_else(|| p.expression.clone());
            values.insert(old, std::mem::replace(&mut p.expression, next));
        }
        self.configurations.active = id;
        self.parameters.evaluate();
        true
    }

    /// The configurations a scope means, or `None` for all of them.
    fn targets(&self, scope: &Scope) -> Result<Option<Vec<ConfigId>>, String> {
        match scope {
            Scope::All => Ok(None),
            Scope::This => Ok(Some(vec![self.configurations.active])),
            Scope::Only(ids) => {
                if ids.is_empty() {
                    return Err("No configuration was named.".to_owned());
                }
                if ids.iter().any(|id| !self.configurations.contains(*id)) {
                    return Err("One of the configurations no longer exists.".to_owned());
                }
                Ok(Some(ids.clone()))
            }
        }
    }

    /// Suppresses or unsuppresses a feature in the configurations of `scope`.
    pub fn set_suppressed(
        &mut self,
        feature: FeatureId,
        on: bool,
        scope: &Scope,
    ) -> Result<(), String> {
        let Some(index) = self.index_of(feature) else {
            return Err("The feature no longer exists.".to_owned());
        };
        let now = self.features[index].suppressed;
        let active = self.configurations.active;
        let others = self.configurations.others();
        let here = match self.targets(scope)? {
            None => {
                self.configurations.suppressed.remove(&feature);
                true
            }
            Some(targets) => {
                if !others.is_empty() {
                    let values = self
                        .configurations
                        .suppressed
                        .entry(feature)
                        .or_insert_with(|| others.iter().map(|c| (*c, now)).collect());
                    for t in targets.iter().filter(|t| **t != active) {
                        values.insert(*t, on);
                    }
                }
                targets.contains(&active)
            }
        };
        if here && on != now {
            Arc::make_mut(&mut self.features[index]).suppressed = on;
        }
        self.tidy_configurations();
        Ok(())
    }

    /// Adds a parameter, or changes its expression in the configurations of `scope`. A new
    /// parameter exists in every configuration, with the same expression. Returns the
    /// value in the active configuration. Nothing changes on error.
    pub fn set_parameter(
        &mut self,
        name: &str,
        expression: &str,
        scope: &Scope,
    ) -> Result<f64, String> {
        let expression = expression.trim();
        let active = self.configurations.active;
        let others = self.configurations.others();
        let existing = self
            .parameters
            .entries
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.expression.clone());
        let targets = match (&existing, self.targets(scope)?) {
            (Some(_), Some(t)) if !others.is_empty() => t,
            // New, in every configuration, or the only configuration there is.
            (_, all) => {
                let before = self.clone();
                let result = self.set_parameter_everywhere(name, expression, all.is_none());
                if result.is_err() {
                    *self = before;
                }
                return result;
            }
        };
        let now = existing.unwrap_or_default();
        // Checked in each configuration first, so that an error changes nothing.
        for t in targets.iter().filter(|t| **t != active) {
            let mut probe = self.with_configuration(*t);
            probe.parameters.set(name, expression).map_err(|e| {
                format!(
                    "{} (in configuration {})",
                    e.message,
                    probe.active_configuration().name
                )
            })?;
        }
        if targets.contains(&active) {
            self.parameters
                .set(name, expression)
                .map_err(|e| e.message)?;
        }
        let values = self
            .configurations
            .parameters
            .entry(name.to_owned())
            .or_insert_with(|| others.iter().map(|c| (*c, now.clone())).collect());
        for t in targets.iter().filter(|t| **t != active) {
            values.insert(*t, expression.to_owned());
        }
        self.tidy_configurations();
        Ok(self.parameters.get(name).unwrap_or(f64::NAN))
    }

    /// Sets a parameter in the model and, with `all`, makes it the same everywhere.
    fn set_parameter_everywhere(
        &mut self,
        name: &str,
        expression: &str,
        all: bool,
    ) -> Result<f64, String> {
        let value = self
            .parameters
            .set(name, expression)
            .map_err(|e| e.message)?;
        if all && self.configurations.parameters.remove(name).is_some() {
            // It must still evaluate where the parameters it uses differ.
            for c in self.configurations.others() {
                let mut probe = self.with_configuration(c);
                probe.parameters.set(name, expression).map_err(|e| {
                    format!(
                        "{} (in configuration {})",
                        e.message,
                        probe.active_configuration().name
                    )
                })?;
            }
        }
        self.tidy_configurations();
        Ok(value)
    }

    /// Removes a parameter from every configuration.
    pub fn remove_parameter(&mut self, name: &str) {
        self.parameters.remove(name);
        self.tidy_configurations();
    }

    /// Brings what is kept about the configurations in step with the model: forgets
    /// features and parameters that no longer exist, and values that are the same in
    /// every configuration. Editing through [`Model`]'s own methods does this already; it
    /// is for after the features or the parameter table were changed directly.
    pub fn tidy_configurations(&mut self) {
        let others = self.configurations.others();
        let mut suppressed = std::mem::take(&mut self.configurations.suppressed);
        tidy(&mut suppressed, &others, |id| {
            self.feature(*id).map(|f| f.suppressed)
        });
        self.configurations.suppressed = suppressed;
        let mut parameters = std::mem::take(&mut self.configurations.parameters);
        tidy(&mut parameters, &others, |name| {
            self.parameters
                .entries
                .iter()
                .find(|p| p.name == *name)
                .map(|p| p.expression.clone())
        });
        self.configurations.parameters = parameters;
    }

    /// Restores the invariants of the configurations after loading a model from a file.
    pub(crate) fn validate_configurations(&mut self) -> Result<(), String> {
        let c = &mut self.configurations;
        if c.list.is_empty() {
            *c = Configurations::default();
        }
        {
            let mut ids = std::collections::HashSet::new();
            let mut names = std::collections::HashSet::new();
            for x in &c.list {
                if !ids.insert(x.id) || !names.insert(x.name.as_str()) {
                    return Err(format!(
                        "The model is damaged: two configurations are called '{}' or have the id {}.",
                        x.name, x.id.0
                    ));
                }
            }
        }
        if !c.contains(c.active) {
            c.active = c.list[0].id;
        }
        let max = c.list.iter().map(|x| x.id.0).max().unwrap_or(0);
        c.next_id = c.next_id.max(max.saturating_add(1));
        self.tidy_configurations();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{PlaneRef, StdPlane};
    use peet_math::Plane;

    fn model() -> (Model, FeatureId, FeatureId) {
        let mut m = Model::new();
        let a = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
        let b = m.add_sketch(PlaneRef::Standard(StdPlane::Front), Plane::front());
        m.parameters.set("t", "2").unwrap();
        m.parameters.set("w", "10 * t").unwrap();
        (m, a, b)
    }

    fn expr<'a>(m: &'a Model, name: &str) -> &'a str {
        m.parameter_in(name, m.active_configuration().id).unwrap()
    }

    #[test]
    fn a_part_starts_with_one_configuration() {
        let m = Model::new();
        assert_eq!(m.configurations().len(), 1);
        assert_eq!(m.active_configuration().name, DEFAULT_NAME);
    }

    #[test]
    fn with_one_configuration_every_scope_is_the_same() {
        for scope in [Scope::This, Scope::All] {
            let (mut m, a, _) = model();
            m.set_suppressed(a, true, &scope).unwrap();
            m.set_parameter("t", "3", &scope).unwrap();
            assert!(m.feature(a).unwrap().suppressed);
            assert_eq!(m.parameters.get("w"), Some(30.0));
            assert_eq!(m.configurations, Configurations::default());
        }
    }

    #[test]
    fn values_differ_per_configuration_and_swap_on_activation() {
        let (mut m, a, b) = model();
        let default = m.active_configuration().id;
        let thick = m.add_configuration("Thick", None).unwrap();
        assert!(m.activate_configuration(thick));
        m.set_parameter("t", "3", &Scope::This).unwrap();
        m.set_suppressed(a, true, &Scope::This).unwrap();
        assert_eq!(m.parameters.get("w"), Some(30.0));
        assert!(m.parameter_differs("t") && !m.parameter_differs("w"));
        assert!(m.suppression_differs(a) && !m.suppression_differs(b));
        assert_eq!(m.parameter_in("t", default), Some("2"));
        assert_eq!(m.suppressed_in(a, default), Some(false));

        assert!(m.activate_configuration(default));
        assert_eq!(m.parameters.get("w"), Some(20.0));
        assert!(!m.feature(a).unwrap().suppressed);
        assert_eq!(m.parameter_in("t", thick), Some("3"));
        assert_eq!(m.suppressed_in(a, thick), Some(true));

        // Without making it active.
        let other = m.with_configuration(thick);
        assert_eq!(other.parameters.get("w"), Some(30.0));
        assert!(other.feature(a).unwrap().suppressed);
        assert_eq!(m.active_configuration().id, default);
        // There and back is where it started.
        assert_eq!(other.with_configuration(default), m);
    }

    #[test]
    fn scopes() {
        let (mut m, a, _) = model();
        let default = m.active_configuration().id;
        let b = m.add_configuration("B", None).unwrap();
        let c = m.add_configuration("C", None).unwrap();
        // Named configurations, without making them active.
        m.set_parameter("t", "5", &Scope::Only(vec![b, c])).unwrap();
        m.set_suppressed(a, true, &Scope::Only(vec![c])).unwrap();
        assert_eq!(expr(&m, "t"), "2");
        assert_eq!(m.parameter_in("t", b), Some("5"));
        assert_eq!(m.suppressed_in(a, b), Some(false));
        assert_eq!(m.suppressed_in(a, c), Some(true));
        // All: no longer differs.
        m.set_parameter("t", "4", &Scope::All).unwrap();
        m.set_suppressed(a, false, &Scope::All).unwrap();
        assert!(!m.parameter_differs("t") && !m.suppression_differs(a));
        assert_eq!(m.parameter_in("t", c), Some("4"));
        // Setting them all to the same value one by one doesn't differ either.
        m.set_parameter("t", "6", &Scope::Only(vec![b])).unwrap();
        assert!(m.parameter_differs("t"));
        m.set_parameter("t", "6", &Scope::Only(vec![default, c]))
            .unwrap();
        assert!(!m.parameter_differs("t"));
        assert_eq!(expr(&m, "t"), "6");
    }

    #[test]
    fn a_new_configuration_copies_and_a_new_parameter_is_everywhere() {
        let (mut m, a, _) = model();
        let b = m.add_configuration("B", None).unwrap();
        m.set_parameter("t", "5", &Scope::Only(vec![b])).unwrap();
        m.set_suppressed(a, true, &Scope::Only(vec![b])).unwrap();
        let c = m.add_configuration("C", Some(b)).unwrap();
        assert_eq!(m.parameter_in("t", c), Some("5"));
        assert_eq!(m.suppressed_in(a, c), Some(true));
        let d = m.add_configuration("D", None).unwrap();
        assert_eq!(m.parameter_in("t", d), Some("2"));
        m.set_parameter("gap", "1", &Scope::This).unwrap();
        assert_eq!(m.parameter_in("gap", c), Some("1"));
        assert!(!m.parameter_differs("gap"));
        assert!(m.add_configuration("B", None).is_err());
        assert!(m.add_configuration(" ", None).is_err());
    }

    #[test]
    fn a_bad_expression_changes_nothing() {
        let (mut m, _, _) = model();
        let b = m.add_configuration("B", None).unwrap();
        let before = m.clone();
        let e = m
            .set_parameter("t", "2 * nothing", &Scope::Only(vec![b]))
            .unwrap_err();
        assert!(e.contains("configuration B"), "{e}");
        assert!(m.set_parameter("t", "2 *", &Scope::This).is_err());
        assert!(m.set_parameter("t", "w", &Scope::All).is_err());
        assert_eq!(m, before);
    }

    #[test]
    fn removing() {
        let (mut m, a, _) = model();
        let default = m.active_configuration().id;
        let b = m.add_configuration("B", None).unwrap();
        m.set_parameter("t", "5", &Scope::Only(vec![b])).unwrap();
        m.set_suppressed(a, true, &Scope::Only(vec![b])).unwrap();
        // The active one: its neighbour takes over.
        m.remove_configuration(default).unwrap();
        assert_eq!(m.active_configuration().id, b);
        assert_eq!(expr(&m, "t"), "5");
        assert!(m.feature(a).unwrap().suppressed);
        assert!(!m.parameter_differs("t") && !m.suppression_differs(a));
        assert!(m.remove_configuration(b).is_err(), "the last one stays");

        // Features and parameters that go take what was kept about them along.
        let (mut m, a, _) = model();
        let b = m.add_configuration("B", None).unwrap();
        m.set_parameter("w", "50", &Scope::Only(vec![b])).unwrap();
        m.set_suppressed(a, true, &Scope::Only(vec![b])).unwrap();
        m.remove(a);
        m.remove_parameter("w");
        assert_eq!(m.configurations.suppressed.len(), 0);
        assert_eq!(m.configurations.parameters.len(), 0);
    }

    #[test]
    fn direct_edits_are_tidied() {
        let (mut m, a, _) = model();
        let b = m.add_configuration("B", None).unwrap();
        m.set_suppressed(a, true, &Scope::Only(vec![b])).unwrap();
        // The same value as everywhere else, set on the feature itself.
        m.feature_mut(a).unwrap().suppressed = true;
        m.tidy_configurations();
        assert!(!m.suppression_differs(a));
    }
}
