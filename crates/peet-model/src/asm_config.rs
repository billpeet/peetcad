//! Configurations of an assembly, as SolidWorks has them.
//!
//! An assembly's configurations are the model's ([`crate::config`]): the same list, the
//! same active one, the same parameters that differ. What an assembly adds is what
//! SolidWorks lets differ per configuration of an assembly:
//!
//! - per component ([`ComponentState`]): whether it is suppressed, which configuration of
//!   its part it shows (its *referenced configuration*), whether it is fixed, and where it
//!   is (its placement, and the poses of a flexible sub-assembly);
//! - per mate ([`MateState`]): whether it is suppressed, and its kind with its values (a
//!   distance, an angle, limits).
//!
//! As for a part, **the model holds the active configuration**: the components and mates
//! are as it has them, so the rebuild, the queries and the exports see an ordinary
//! assembly. What differs is kept on the side ([`Varying`]): a component or mate that is
//! listed has its state in every configuration other than the active one; one that is not
//! is the same in all of them. Making another configuration active swaps those states with
//! the components' and mates' own.
//!
//! **Where a component is belongs to the configuration.** A solve that moves a component,
//! and `place`, change it in the active configuration only, as SolidWorks keeps a
//! component's position per configuration; so the left and right hinged doors of one
//! assembly each keep their own.

use std::collections::BTreeMap;
use std::sync::Arc;

use peet_math::Frame;
use serde::{Deserialize, Serialize};

use crate::Model;
use crate::assembly::{Assembly, CompId, Component, Pose};
use crate::config::{ConfigId, Scope};
use crate::mate::{Mate, MateId, MateKind};

/// What an assembly's configuration holds of a component.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComponentState {
    pub suppressed: bool,
    /// The configuration of its part it shows (`None`: the part's active one).
    pub configuration: Option<ConfigId>,
    pub fixed: bool,
    pub placement: Frame,
    /// For a flexible sub-assembly: where what moves in it is.
    pub poses: Vec<Pose>,
}

impl ComponentState {
    pub fn of(c: &Component) -> Self {
        Self {
            suppressed: c.suppressed,
            configuration: c.configuration,
            fixed: c.fixed,
            placement: c.placement,
            poses: c.poses.clone(),
        }
    }

    fn apply(&self, c: &mut Component) {
        c.suppressed = self.suppressed;
        c.configuration = self.configuration;
        c.fixed = self.fixed;
        c.placement = self.placement;
        c.poses.clone_from(&self.poses);
    }
}

/// What an assembly's configuration holds of a mate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MateState {
    pub suppressed: bool,
    /// The kind, with its values: a distance's or an angle's, a limit's range, a width
    /// or slot mate's mode.
    pub kind: MateKind,
}

impl MateState {
    pub fn of(m: &Mate) -> Self {
        Self {
            suppressed: m.suppressed,
            kind: m.kind.clone(),
        }
    }

    fn apply(&self, m: &mut Mate) {
        m.suppressed = self.suppressed;
        m.kind.clone_from(&self.kind);
    }
}

/// The states of the components and mates that differ between an assembly's
/// configurations, in every configuration but the active one.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Varying {
    components: BTreeMap<CompId, BTreeMap<ConfigId, ComponentState>>,
    mates: BTreeMap<MateId, BTreeMap<ConfigId, MateState>>,
}

impl Varying {
    /// Forgets the components and mates that are gone.
    pub(crate) fn forget(&mut self, components: &[Arc<Component>], mates: &[Arc<Mate>]) {
        self.components
            .retain(|id, _| components.iter().any(|c| c.id == *id));
        self.mates.retain(|id, _| mates.iter().any(|m| m.id == *id));
    }

    /// A component made an instance of another part: the configurations it showed were
    /// that part's, so in every configuration it shows the new part's active one.
    pub(crate) fn forget_part(&mut self, id: CompId) {
        if let Some(states) = self.components.get_mut(&id) {
            for s in states.values_mut() {
                s.configuration = None;
            }
        }
    }
}

/// Changes `key`'s value in the configurations `targets` (`None`: all): in the table for
/// the others, and returns the active one's new value if it is one of them. `now` is the
/// active one's value. With other configurations, an entry is made first (each of them
/// holding `now`), so a change to some leaves the rest as they were.
fn vary<K: Ord, V: Clone>(
    table: &mut BTreeMap<K, BTreeMap<ConfigId, V>>,
    key: K,
    now: &V,
    others: &[ConfigId],
    active: ConfigId,
    targets: Option<&[ConfigId]>,
    change: &dyn Fn(&mut V),
) -> Option<V> {
    let mut here = now.clone();
    match targets {
        None => {
            if let Some(states) = table.get_mut(&key) {
                states.values_mut().for_each(change);
            }
            change(&mut here);
            Some(here)
        }
        Some(targets) => {
            if !others.is_empty() {
                let states = table.entry(key).or_default();
                for c in others {
                    states.entry(*c).or_insert_with(|| now.clone());
                }
                for t in targets.iter().filter(|t| **t != active) {
                    if let Some(s) = states.get_mut(t) {
                        change(s);
                    }
                }
            }
            targets.contains(&active).then(|| {
                change(&mut here);
                here
            })
        }
    }
}

impl Assembly {
    /// The configuration of its part a component shows, if it is not the one the part
    /// has active: `None` for that one, and for one the part no longer has.
    pub fn configuration_of(&self, c: &Component) -> Option<ConfigId> {
        let id = c.configuration?;
        let model = &self.definition(c.definition)?.model;
        (model.active_configuration().id != id && model.configuration(id).is_some()).then_some(id)
    }

    /// The model a component shows: its part (or sub-assembly) in the configuration the
    /// component uses.
    pub fn model_of(&self, c: &Component) -> Option<Arc<Model>> {
        let definition = self.definition(c.definition)?;
        Some(match self.configuration_of(c) {
            Some(id) => Arc::new(definition.model.with_configuration(id)),
            None => definition.model.clone(),
        })
    }

    /// [`Assembly::model_of`] by the component's id.
    pub fn component_model(&self, id: CompId) -> Option<Arc<Model>> {
        self.model_of(self.component(id)?)
    }

    /// The name of the configuration of its part a component shows.
    pub fn configuration_name(&self, c: &Component) -> Option<&str> {
        let model = &self.definition(c.definition)?.model;
        let id = self
            .configuration_of(c)
            .unwrap_or(model.active_configuration().id);
        model.configuration(id).map(|x| x.name.as_str())
    }
}

impl Model {
    /// A component's state in a configuration of this assembly.
    pub fn component_in(&self, id: CompId, config: ConfigId) -> Option<ComponentState> {
        let assembly = self.assembly()?;
        let now = ComponentState::of(assembly.component(id)?);
        if config == self.active_configuration().id {
            return Some(now);
        }
        Some(
            assembly
                .varying
                .components
                .get(&id)
                .and_then(|v| v.get(&config))
                .cloned()
                .unwrap_or(now),
        )
    }

    /// A mate's state in a configuration of this assembly.
    pub fn mate_in(&self, id: MateId, config: ConfigId) -> Option<MateState> {
        let assembly = self.assembly()?;
        let now = MateState::of(assembly.mate(id)?);
        if config == self.active_configuration().id {
            return Some(now);
        }
        Some(
            assembly
                .varying
                .mates
                .get(&id)
                .and_then(|v| v.get(&config))
                .cloned()
                .unwrap_or(now),
        )
    }

    /// Whether anything of a component differs between the configurations.
    pub fn component_differs(&self, id: CompId) -> bool {
        self.assembly()
            .is_some_and(|a| a.varying.components.contains_key(&id))
    }

    /// Whether anything of a mate differs between the configurations.
    pub fn mate_differs(&self, id: MateId) -> bool {
        self.assembly()
            .is_some_and(|a| a.varying.mates.contains_key(&id))
    }

    /// Changes a component of this assembly in the configurations of `scope`.
    pub fn set_component(
        &mut self,
        id: CompId,
        scope: &Scope,
        change: impl Fn(&mut ComponentState),
    ) -> Result<(), String> {
        let targets = self.scope_targets(scope)?;
        let active = self.active_configuration().id;
        let others = self.other_configurations();
        let assembly = self
            .assembly_mut()
            .ok_or_else(|| "The document is not an assembly.".to_owned())?;
        let now = ComponentState::of(
            assembly
                .component(id)
                .ok_or_else(|| "The component no longer exists.".to_owned())?,
        );
        let here = vary(
            &mut assembly.varying.components,
            id,
            &now,
            &others,
            active,
            targets.as_deref(),
            &change,
        );
        if let Some(here) = here.filter(|h| *h != now)
            && let Some(c) = assembly.component_mut(id)
        {
            here.apply(c);
        }
        self.tidy_configurations();
        Ok(())
    }

    /// Changes a mate of this assembly in the configurations of `scope`.
    pub fn set_mate(
        &mut self,
        id: MateId,
        scope: &Scope,
        change: impl Fn(&mut MateState),
    ) -> Result<(), String> {
        let targets = self.scope_targets(scope)?;
        let active = self.active_configuration().id;
        let others = self.other_configurations();
        let assembly = self
            .assembly_mut()
            .ok_or_else(|| "The document is not an assembly.".to_owned())?;
        let now = MateState::of(
            assembly
                .mate(id)
                .ok_or_else(|| "The mate no longer exists.".to_owned())?,
        );
        let here = vary(
            &mut assembly.varying.mates,
            id,
            &now,
            &others,
            active,
            targets.as_deref(),
            &change,
        );
        if let Some(here) = here.filter(|h| *h != now)
            && let Some(m) = assembly.mate_mut(id)
        {
            here.apply(m);
        }
        self.tidy_configurations();
        Ok(())
    }

    /// Says which configurations a change made to a component in the model (the active
    /// configuration) applies to: `before` is how it was. Used where the change was made
    /// directly, as the rebuild writes solved placements back.
    pub(crate) fn rescope_component(&mut self, id: CompId, before: ComponentState, scope: &Scope) {
        let Some(after) = self
            .assembly()
            .and_then(|a| a.component(id))
            .map(ComponentState::of)
        else {
            return;
        };
        if after == before {
            return;
        }
        if let Some(c) = self.assembly_mut().and_then(|a| a.component_mut(id)) {
            before.apply(c);
        }
        let _ = self.set_component(id, scope, |s| *s = after.clone());
    }

    /// Swaps the states of the components and mates that differ: `old` was active, `new`
    /// is now.
    pub(crate) fn swap_assembly(&mut self, old: ConfigId, new: ConfigId) {
        let Some(assembly) = self.assembly_mut() else {
            return;
        };
        let mut components = std::mem::take(&mut assembly.varying.components);
        for (id, states) in &mut components {
            let Some(c) = assembly.component(*id) else {
                continue;
            };
            let now = ComponentState::of(c);
            let next = states.remove(&new).unwrap_or_else(|| now.clone());
            if next != now
                && let Some(c) = assembly.component_mut(*id)
            {
                next.apply(c);
            }
            states.insert(old, now);
        }
        assembly.varying.components = components;
        let mut mates = std::mem::take(&mut assembly.varying.mates);
        for (id, states) in &mut mates {
            let Some(m) = assembly.mate(*id) else {
                continue;
            };
            let now = MateState::of(m);
            let next = states.remove(&new).unwrap_or_else(|| now.clone());
            if next != now
                && let Some(m) = assembly.mate_mut(*id)
            {
                next.apply(m);
            }
            states.insert(old, now);
        }
        assembly.varying.mates = mates;
    }

    /// A new configuration `id` starts as `source` has its components and mates.
    pub(crate) fn copy_assembly(&mut self, source: ConfigId, id: ConfigId) {
        let Some(assembly) = self.assembly() else {
            return;
        };
        let components: Vec<(CompId, ComponentState)> = assembly
            .varying
            .components
            .keys()
            .filter_map(|c| Some((*c, self.component_in(*c, source)?)))
            .collect();
        let mates: Vec<(MateId, MateState)> = assembly
            .varying
            .mates
            .keys()
            .filter_map(|m| Some((*m, self.mate_in(*m, source)?)))
            .collect();
        let Some(assembly) = self.assembly_mut() else {
            return;
        };
        for (c, s) in components {
            assembly
                .varying
                .components
                .entry(c)
                .or_default()
                .insert(id, s);
        }
        for (m, s) in mates {
            assembly.varying.mates.entry(m).or_default().insert(id, s);
        }
    }

    /// Drops what says nothing: components and mates that are gone, configurations that
    /// are gone, and states that are the same in every configuration.
    pub(crate) fn tidy_assembly(&mut self, others: &[ConfigId]) {
        let Some(assembly) = self.assembly() else {
            return;
        };
        if assembly.varying == Varying::default() {
            return;
        }
        let mut varying = assembly.varying.clone();
        crate::config::tidy(
            &mut varying.components,
            others,
            |id| assembly.component(*id).map(ComponentState::of),
            |a, b| a == b,
        );
        crate::config::tidy(
            &mut varying.mates,
            others,
            |id| assembly.mate(*id).map(MateState::of),
            |a, b| a == b,
        );
        if varying != assembly.varying
            && let Some(assembly) = self.assembly_mut()
        {
            assembly.varying = varying;
        }
    }
}
