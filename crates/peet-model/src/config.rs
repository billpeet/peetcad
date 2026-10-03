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

use peet_sketch::ConstraintId;

use crate::Model;
use crate::feature::{Feature, FeatureId, FeatureKind, Scalar, ScalarKind};

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

/// A numeric value of a feature that can differ between configurations.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Slot {
    /// A value of the feature itself, by the name its operations give it (`depth`): see
    /// [`FeatureKind::slots`].
    Field(String),
    /// A driving dimension of a sketch.
    Dimension(ConstraintId),
}

/// What a [`Slot`] holds in one configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Held {
    Scalar(Scalar),
    // New kinds go at the end: files store the variant's index.
}

/// Whether two values are the same as entered. An expression's last evaluated value is
/// not part of it: it follows the parameters, which may themselves differ.
fn same_scalar(a: &Scalar, b: &Scalar) -> bool {
    match (&a.expression, &b.expression) {
        (Some(x), Some(y)) => x == y,
        (None, None) => a.value == b.value,
        _ => false,
    }
}

fn same_held(a: &Held, b: &Held) -> bool {
    let (Held::Scalar(a), Held::Scalar(b)) = (a, b);
    same_scalar(a, b)
}

/// A slot's value in a feature, with what it measures.
fn read_slot(feature: &Feature, slot: &Slot) -> Option<(ScalarKind, Scalar)> {
    match (slot, &feature.kind) {
        (Slot::Field(name), kind) => kind
            .slots()
            .into_iter()
            .find(|(n, ..)| n == name)
            .map(|(_, kind, s)| (kind, s.clone())),
        (Slot::Dimension(id), FeatureKind::Sketch(s)) => {
            let c = s.sketch.constraint(*id)?;
            // A driven dimension measures the sketch: it has nothing to set.
            let d = c.dimension.as_ref().filter(|d| d.driving)?;
            let kind = if c.kind.is_angular() {
                ScalarKind::Angle
            } else {
                ScalarKind::Length
            };
            Some((
                kind,
                Scalar {
                    value: d.value,
                    expression: d.expression.clone(),
                },
            ))
        }
        (Slot::Dimension(_), _) => None,
    }
}

/// Sets a slot's value in a feature. Returns whether the feature has the slot.
fn write_slot(feature: &mut Feature, slot: &Slot, value: Scalar) -> bool {
    match (slot, &mut feature.kind) {
        (Slot::Field(name), kind) => match kind.slots_mut().into_iter().find(|(n, ..)| n == name) {
            Some((_, _, s)) => {
                *s = value;
                true
            }
            None => false,
        },
        (Slot::Dimension(id), FeatureKind::Sketch(s)) => {
            match s
                .sketch
                .constraint_mut(*id)
                .and_then(|c| c.dimension.as_mut())
            {
                Some(d) => {
                    d.value = value.value;
                    d.expression = value.expression;
                    true
                }
                None => false,
            }
        }
        (Slot::Dimension(_), _) => false,
    }
}

/// A value of a feature that can differ between configurations, as it is now.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotValue {
    pub slot: Slot,
    /// What it is called: the field's name, or the dimension's (`d1`).
    pub name: String,
    pub kind: ScalarKind,
    pub value: Scalar,
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
    /// Values of features (their numeric fields, and the dimensions of sketches) that
    /// differ: the value in every configuration other than the active one.
    values: BTreeMap<(FeatureId, Slot), BTreeMap<ConfigId, Held>>,
}

/// The configurations as files of model schema 5 hold them: before feature values and
/// sketch dimensions could differ.
#[derive(Deserialize)]
pub struct ConfigurationsV5 {
    list: Vec<Configuration>,
    active: ConfigId,
    next_id: u32,
    suppressed: BTreeMap<FeatureId, BTreeMap<ConfigId, bool>>,
    parameters: BTreeMap<String, BTreeMap<ConfigId, String>>,
}

impl From<ConfigurationsV5> for Configurations {
    fn from(c: ConfigurationsV5) -> Self {
        Self {
            list: c.list,
            active: c.active,
            next_id: c.next_id,
            suppressed: c.suppressed,
            parameters: c.parameters,
            values: BTreeMap::new(),
        }
    }
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
            values: BTreeMap::new(),
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
fn tidy<K: Ord + Clone, V: Clone>(
    table: &mut BTreeMap<K, BTreeMap<ConfigId, V>>,
    others: &[ConfigId],
    current: impl Fn(&K) -> Option<V>,
    same: impl Fn(&V, &V) -> bool,
) {
    table.retain(|key, values| {
        let Some(now) = current(key) else {
            return false;
        };
        values.retain(|c, _| others.contains(c));
        for c in others {
            values.entry(*c).or_insert_with(|| now.clone());
        }
        values.values().any(|v| !same(v, &now))
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

    /// The feature's values that can differ between configurations, as they are in the
    /// active one: its numeric fields, or a sketch's driving dimensions.
    pub fn values(&self, feature: FeatureId) -> Vec<SlotValue> {
        let Some(f) = self.feature(feature) else {
            return Vec::new();
        };
        match &f.kind {
            FeatureKind::Sketch(s) => s
                .sketch
                .constraints()
                .filter_map(|(id, c)| {
                    let slot = Slot::Dimension(id);
                    let (kind, value) = read_slot(f, &slot)?;
                    Some(SlotValue {
                        slot,
                        name: c.dimension.as_ref()?.name.clone(),
                        kind,
                        value,
                    })
                })
                .collect(),
            kind => kind
                .slots()
                .into_iter()
                .map(|(name, kind, s)| SlotValue {
                    slot: Slot::Field(name.to_owned()),
                    name: name.to_owned(),
                    kind,
                    value: s.clone(),
                })
                .collect(),
        }
    }

    /// The slot called `name` in a feature: a numeric field, or a sketch's dimension.
    pub fn slot_named(&self, feature: FeatureId, name: &str) -> Option<SlotValue> {
        self.values(feature).into_iter().find(|v| v.name == name)
    }

    /// A slot's value in the active configuration, with what it measures.
    pub fn value(&self, feature: FeatureId, slot: &Slot) -> Option<(ScalarKind, Scalar)> {
        read_slot(self.feature(feature)?, slot)
    }

    /// Whether the value differs between configurations.
    pub fn value_differs(&self, feature: FeatureId, slot: &Slot) -> bool {
        self.configurations
            .values
            .contains_key(&(feature, slot.clone()))
    }

    /// The value in a configuration.
    pub fn value_in(&self, feature: FeatureId, slot: &Slot, config: ConfigId) -> Option<Scalar> {
        let (_, now) = self.value(feature, slot)?;
        if config == self.configurations.active {
            return Some(now);
        }
        Some(
            match self
                .configurations
                .values
                .get(&(feature, slot.clone()))
                .and_then(|v| v.get(&config))
            {
                Some(Held::Scalar(s)) => s.clone(),
                None => now,
            },
        )
    }

    /// The values that differ between configurations, in tree order.
    pub fn values_that_differ(&self) -> Vec<(FeatureId, Slot)> {
        let mut out: Vec<(FeatureId, Slot)> = self.configurations.values.keys().cloned().collect();
        out.sort_by_key(|(f, _)| self.index_of(*f));
        out
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
        let values: Vec<((FeatureId, Slot), Scalar)> = self
            .configurations
            .values
            .keys()
            .filter_map(|k| Some((k.clone(), self.value_in(k.0, &k.1, source)?)))
            .collect();
        let c = &mut self.configurations;
        for (k, v) in values {
            c.values.entry(k).or_default().insert(id, Held::Scalar(v));
        }
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
        let mut values = std::mem::take(&mut self.configurations.values);
        for ((feature, slot), held) in &mut values {
            let Some(index) = self.index_of(*feature) else {
                continue;
            };
            let Some((_, now)) = read_slot(&self.features[index], slot) else {
                continue;
            };
            let next = match held.remove(&id) {
                Some(Held::Scalar(s)) => s,
                None => now.clone(),
            };
            held.insert(old, Held::Scalar(now.clone()));
            if next != now {
                write_slot(Arc::make_mut(&mut self.features[index]), slot, next);
            }
        }
        self.configurations.values = values;
        self.configurations.active = id;
        self.parameters.evaluate();
        true
    }

    /// Sets a value of a feature (a numeric field, or a sketch's dimension) in the
    /// configurations of `scope`.
    pub fn set_value(
        &mut self,
        feature: FeatureId,
        slot: &Slot,
        value: Scalar,
        scope: &Scope,
    ) -> Result<(), String> {
        let Some((_, before)) = self.value(feature, slot) else {
            return Err("The feature has no such value.".to_owned());
        };
        self.targets(scope)?;
        if let Some(f) = self.feature_mut(feature) {
            write_slot(f, slot, value);
        }
        self.rescope_value(feature, slot, before, scope)
    }

    /// Says which configurations a change that was made to a value in the model applies
    /// to: `before` is what the value was. (The model holds the active configuration, so
    /// the change was made there; this carries it to the others, or keeps it from them.)
    pub fn rescope_value(
        &mut self,
        feature: FeatureId,
        slot: &Slot,
        before: Scalar,
        scope: &Scope,
    ) -> Result<(), String> {
        let Some((_, now)) = self.value(feature, slot) else {
            return Err("The feature has no such value.".to_owned());
        };
        let active = self.configurations.active;
        let others = self.configurations.others();
        let key = (feature, slot.clone());
        match self.targets(scope)? {
            None => {
                self.configurations.values.remove(&key);
            }
            Some(targets) => {
                if !others.is_empty() {
                    let held = self.configurations.values.entry(key).or_insert_with(|| {
                        others
                            .iter()
                            .map(|c| (*c, Held::Scalar(before.clone())))
                            .collect()
                    });
                    for t in targets.iter().filter(|t| **t != active) {
                        held.insert(*t, Held::Scalar(now.clone()));
                    }
                    if !targets.contains(&active)
                        && let Some(f) = self.feature_mut(feature)
                    {
                        write_slot(f, slot, before);
                    }
                }
            }
        }
        self.tidy_configurations();
        Ok(())
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
        tidy(
            &mut suppressed,
            &others,
            |id| self.feature(*id).map(|f| f.suppressed),
            |a, b| a == b,
        );
        self.configurations.suppressed = suppressed;
        let mut parameters = std::mem::take(&mut self.configurations.parameters);
        tidy(
            &mut parameters,
            &others,
            |name| {
                self.parameters
                    .entries
                    .iter()
                    .find(|p| p.name == *name)
                    .map(|p| p.expression.clone())
            },
            |a, b| a == b,
        );
        self.configurations.parameters = parameters;
        let mut values = std::mem::take(&mut self.configurations.values);
        tidy(
            &mut values,
            &others,
            |(feature, slot)| {
                self.value(*feature, slot)
                    .map(|(_, value)| Held::Scalar(value))
            },
            same_held,
        );
        self.configurations.values = values;
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

    /// A plate: a sketch with a dimensioned line, extruded.
    fn plate() -> (Model, FeatureId, FeatureId, Slot) {
        use peet_math::DVec2;
        use peet_sketch::ConstraintKind;
        let mut m = Model::new();
        let sketch = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
        let s = &mut m.feature_mut(sketch).unwrap().sketch_mut().unwrap().sketch;
        let shape = peet_sketch::shapes::rectangle(s, DVec2::ZERO, DVec2::new(80.0, 50.0));
        let width = s
            .add_dimension(ConstraintKind::Length(shape.curves[0]), 80.0)
            .unwrap();
        let extrude = m.add_extrude(sketch, crate::Operation::Add);
        (m, sketch, extrude, Slot::Dimension(width))
    }

    fn depth() -> Slot {
        Slot::Field("depth".to_owned())
    }

    fn plain(m: &Model, feature: FeatureId, slot: &Slot, config: ConfigId) -> f64 {
        m.value_in(feature, slot, config).unwrap().value
    }

    #[test]
    fn a_features_values_and_a_sketchs_dimensions_are_listed() {
        let (m, sketch, extrude, width) = plate();
        let values = m.values(extrude);
        assert_eq!(values.len(), 1);
        assert_eq!(
            (values[0].name.as_str(), values[0].kind),
            ("depth", ScalarKind::Length)
        );
        let dims = m.values(sketch);
        assert_eq!(dims.len(), 1);
        assert_eq!((dims[0].slot.clone(), dims[0].name.as_str()), (width, "d1"));
        assert_eq!(m.slot_named(sketch, "d1").unwrap().value, Scalar::new(80.0));
        assert!(m.slot_named(extrude, "width").is_none());
    }

    #[test]
    fn values_and_dimensions_differ_per_configuration_and_swap() {
        let (mut m, sketch, extrude, width) = plate();
        let default = m.active_configuration().id;
        // With one configuration nothing is kept on the side.
        m.set_value(extrude, &depth(), Scalar::new(12.0), &Scope::This)
            .unwrap();
        assert!(!m.value_differs(extrude, &depth()));

        let long = m.add_configuration("Long", None).unwrap();
        m.activate_configuration(long);
        m.set_value(sketch, &width, Scalar::new(120.0), &Scope::This)
            .unwrap();
        m.set_value(extrude, &depth(), Scalar::new(20.0), &Scope::This)
            .unwrap();
        assert!(m.value_differs(sketch, &width) && m.value_differs(extrude, &depth()));
        assert_eq!(plain(&m, sketch, &width, default), 80.0);
        assert_eq!(plain(&m, extrude, &depth(), default), 12.0);
        assert_eq!(
            m.values_that_differ(),
            [(sketch, width.clone()), (extrude, depth())]
        );

        m.activate_configuration(default);
        assert_eq!(m.value(sketch, &width).unwrap().1.value, 80.0);
        let extruded = m.feature(extrude).unwrap().extrude().unwrap();
        assert_eq!(extruded.params.depth.value, 12.0);
        assert_eq!(plain(&m, sketch, &width, long), 120.0);
        assert_eq!(m.with_configuration(long).with_configuration(default), m);

        // A copy takes the values of what it copies.
        let copy = m.add_configuration("Copy", Some(long)).unwrap();
        assert_eq!(plain(&m, extrude, &depth(), copy), 20.0);
        // Named configurations, then all.
        let both = Scope::Only(vec![long, copy]);
        m.set_value(extrude, &depth(), Scalar::new(30.0), &both)
            .unwrap();
        assert_eq!(plain(&m, extrude, &depth(), default), 12.0);
        assert_eq!(plain(&m, extrude, &depth(), copy), 30.0);
        m.set_value(extrude, &depth(), Scalar::new(15.0), &Scope::All)
            .unwrap();
        assert!(!m.value_differs(extrude, &depth()));
        assert_eq!(plain(&m, extrude, &depth(), long), 15.0);
    }

    #[test]
    fn a_change_made_in_the_model_is_given_its_scope_afterwards() {
        let (mut m, _, extrude, _) = plate();
        let default = m.active_configuration().id;
        let other = m.add_configuration("Other", None).unwrap();
        let set = |m: &mut Model, v: f64| {
            let before = m.value(extrude, &depth()).unwrap().1;
            let e = m.feature_mut(extrude).unwrap().extrude_mut().unwrap();
            e.params.depth = Scalar::new(v);
            before
        };
        // Not rescoped: it doesn't differ, so the change is everywhere.
        set(&mut m, 9.0);
        m.tidy_configurations();
        assert_eq!(plain(&m, extrude, &depth(), other), 9.0);
        // This configuration: the others keep what it was.
        let before = set(&mut m, 11.0);
        m.rescope_value(extrude, &depth(), before, &Scope::This)
            .unwrap();
        assert_eq!(plain(&m, extrude, &depth(), other), 9.0);
        assert_eq!(plain(&m, extrude, &depth(), default), 11.0);
        // Another configuration only: the active one is put back.
        let before = set(&mut m, 14.0);
        m.rescope_value(extrude, &depth(), before, &Scope::Only(vec![other]))
            .unwrap();
        assert_eq!(plain(&m, extrude, &depth(), other), 14.0);
        assert_eq!(plain(&m, extrude, &depth(), default), 11.0);
        // Not rescoped, where it differs: the change is here only.
        set(&mut m, 12.0);
        m.tidy_configurations();
        assert_eq!(plain(&m, extrude, &depth(), other), 14.0);
    }

    #[test]
    fn expressions_are_compared_as_entered_and_stale_values_go() {
        let (mut m, sketch, extrude, width) = plate();
        m.parameters.set("t", "5").unwrap();
        let other = m.add_configuration("Other", None).unwrap();
        m.set_parameter("t", "8", &Scope::Only(vec![other]))
            .unwrap();
        let by_t = |value| Scalar {
            value,
            expression: Some("2 * t".to_owned()),
        };
        m.set_value(extrude, &depth(), by_t(10.0), &Scope::This)
            .unwrap();
        assert!(m.value_differs(extrude, &depth()));
        // The same expression there, though it evaluated to another number.
        m.set_value(extrude, &depth(), by_t(16.0), &Scope::Only(vec![other]))
            .unwrap();
        assert!(!m.value_differs(extrude, &depth()));

        // A dimension that is made driven, and a feature that goes, take what was kept
        // about them along.
        m.set_value(sketch, &width, Scalar::new(90.0), &Scope::This)
            .unwrap();
        m.set_value(extrude, &depth(), Scalar::new(7.0), &Scope::This)
            .unwrap();
        let Slot::Dimension(id) = width.clone() else {
            unreachable!()
        };
        let s = &mut m.feature_mut(sketch).unwrap().sketch_mut().unwrap().sketch;
        let dimension = s.constraint_mut(id).unwrap().dimension.as_mut().unwrap();
        dimension.driving = false;
        m.tidy_configurations();
        assert!(!m.value_differs(sketch, &width));
        m.remove(extrude);
        assert!(m.values_that_differ().is_empty());
        assert!(
            m.set_value(sketch, &width, Scalar::new(1.0), &Scope::This)
                .is_err()
        );
    }

    #[test]
    fn every_numeric_value_of_the_samples_has_a_slot() {
        for (model, _) in [
            crate::samples::bracket(),
            crate::samples::enclosure(),
            crate::samples::chassis(),
            crate::samples::housing(),
        ] {
            for f in model.features() {
                let slots = f.kind.slots();
                let scalars = f.kind.scalars();
                assert_eq!(slots.len(), scalars.len(), "{}", f.name);
                for (name, _, s) in &slots {
                    let found = scalars.iter().any(|x| std::ptr::eq(*x, *s));
                    assert!(found, "{}: {name}", f.name);
                }
                let mut names: Vec<&str> = slots.iter().map(|(n, ..)| *n).collect();
                names.sort_unstable();
                names.dedup();
                assert_eq!(names.len(), slots.len(), "{}: names are unique", f.name);
            }
        }
    }
}
