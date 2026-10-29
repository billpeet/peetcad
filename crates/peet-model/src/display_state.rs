//! Display states of an assembly, as SolidWorks has them.
//!
//! A display state is a named look of the assembly: for each component, whether it is
//! shown, its colour and how transparent it is ([`Look`]). Suppression, configurations of
//! parts and positions belong to configurations, not to display states: a display state
//! changes only how the assembly is drawn.
//!
//! Display states are either **linked to configurations** (SolidWorks' default: each
//! configuration has display states of its own, and shows the one it last had active) or
//! **independent** (every configuration can show every display state, and the one shown
//! stays when the configuration changes).
//!
//! As with configurations, **the components hold the active display state**: a component's
//! `visible`, `color` and `transparency` are its look in the display state shown now, so
//! everything that draws or picks sees an ordinary assembly. Every other display state
//! keeps the looks that are not the default (shown, the part's colour, opaque); showing
//! another swaps them. Until a display state is added the assembly has none, and the
//! components' looks are simply theirs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Model;
use crate::assembly::{CompId, Component};
use crate::config::ConfigId;

/// Identifies a display state within its assembly. Ids are not reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DisplayStateId(pub u32);

/// How a component looks in a display state.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Look {
    /// Shown, or hidden (hidden is not suppressed: see [`Component::visible`]).
    pub visible: bool,
    /// A colour of its own, in place of its part's (sRGB).
    pub color: Option<[u8; 3]>,
    /// How much of what is behind shows through: 0 opaque, 1 not seen at all
    /// (SolidWorks' appearance transparency).
    pub transparency: f32,
}

impl Look {
    /// Shown, in its part's colour, opaque: what a component is in a display state that
    /// says nothing of it.
    pub const DEFAULT: Self = Self {
        visible: true,
        color: None,
        transparency: 0.0,
    };

    pub fn of(c: &Component) -> Self {
        Self {
            visible: c.visible,
            color: c.color,
            transparency: c.transparency,
        }
    }

    fn apply(&self, c: &mut Component) {
        c.visible = self.visible;
        c.color = self.color;
        c.transparency = self.transparency;
    }
}

impl Default for Look {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A transparency as one: a number from 0 to 1, or why not.
pub fn transparency(value: f64) -> Result<f32, String> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(value as f32)
    } else {
        Err(format!(
            "A transparency is from 0 (opaque) to 1 (not seen at all), not {value}."
        ))
    }
}

/// A named look of the assembly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayState {
    pub id: DisplayStateId,
    /// Unique among the display states of its configuration (linked), or of the assembly.
    pub name: String,
    /// With display states linked to configurations: the configuration it belongs to.
    /// `None` when they are independent.
    pub configuration: Option<ConfigId>,
    /// The one shown: of its configuration (linked), or of the assembly.
    pub active: bool,
    /// For a display state that is not shown now, each component's look that is not
    /// [`Look::DEFAULT`]. Empty for the one shown, whose looks are the components' own.
    looks: BTreeMap<CompId, Look>,
}

/// An assembly's display states.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DisplayStates {
    states: Vec<DisplayState>,
    next: u32,
    /// SolidWorks' "Link display states to configurations".
    linked: bool,
}

impl DisplayStates {
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// Every display state, of every configuration, in the order they were added.
    pub fn all(&self) -> impl Iterator<Item = &DisplayState> {
        self.states.iter()
    }

    pub fn get(&self, id: DisplayStateId) -> Option<&DisplayState> {
        self.states.iter().find(|s| s.id == id)
    }

    fn get_mut(&mut self, id: DisplayStateId) -> Option<&mut DisplayState> {
        self.states.iter_mut().find(|s| s.id == id)
    }

    /// Whether each configuration has display states of its own.
    pub fn linked(&self) -> bool {
        self.linked
    }

    /// The display states configuration `config` can show.
    fn of(&self, config: ConfigId) -> impl Iterator<Item = &DisplayState> {
        let linked = self.linked;
        self.states
            .iter()
            .filter(move |s| !linked || s.configuration == Some(config))
    }

    /// The display state configuration `config` shows, if it has any.
    fn active_in(&self, config: ConfigId) -> Option<DisplayStateId> {
        self.of(config).find(|s| s.active).map(|s| s.id)
    }

    /// Forgets the looks of components that are gone.
    pub(crate) fn forget(&mut self, known: impl Fn(CompId) -> bool) {
        for s in &mut self.states {
            s.looks.retain(|c, _| known(*c));
        }
    }

    /// A name for a new display state of `config`'s: "Display State-2", …
    fn new_name(&self, config: ConfigId) -> String {
        (1..)
            .map(|n| format!("Display State-{n}"))
            .find(|name| !self.of(config).any(|s| s.name == *name))
            .unwrap_or_default()
    }
}

impl DisplayState {
    /// A component's look in this display state, if it is not the one shown now (whose
    /// looks are the components' own).
    pub fn stored_look(&self, component: CompId) -> Look {
        self.looks.get(&component).copied().unwrap_or_default()
    }

    /// The components whose look in this display state is not [`Look::DEFAULT`], if it is
    /// not the one shown now.
    pub fn stored_looks(&self) -> impl Iterator<Item = (CompId, Look)> + '_ {
        self.looks.iter().map(|(c, l)| (*c, *l))
    }
}

fn not_an_assembly() -> String {
    "The document is a part: display states are an assembly's.".to_owned()
}

impl Model {
    fn display(&self) -> Option<&DisplayStates> {
        self.assembly().map(|a| &a.display)
    }

    fn display_mut(&mut self) -> Option<&mut DisplayStates> {
        self.assembly_mut().map(|a| &mut a.display)
    }

    /// Whether each configuration has display states of its own.
    pub fn display_states_linked(&self) -> bool {
        self.display().is_some_and(DisplayStates::linked)
    }

    /// The display states the active configuration can show, in order.
    pub fn display_states(&self) -> Vec<&DisplayState> {
        let config = self.active_configuration().id;
        self.display()
            .map(|d| d.of(config).collect())
            .unwrap_or_default()
    }

    /// Every display state, of every configuration.
    pub fn all_display_states(&self) -> Vec<&DisplayState> {
        self.display()
            .map(|d| d.all().collect())
            .unwrap_or_default()
    }

    pub fn display_state(&self, id: DisplayStateId) -> Option<&DisplayState> {
        self.display()?.get(id)
    }

    /// The display states of configuration `config` (linked), or of the assembly.
    pub fn display_states_of(&self, config: ConfigId) -> Vec<&DisplayState> {
        self.display()
            .map(|d| d.of(config).collect())
            .unwrap_or_default()
    }

    /// The display state shown now, if the assembly has any.
    pub fn active_display_state(&self) -> Option<&DisplayState> {
        let d = self.display()?;
        d.get(d.active_in(self.active_configuration().id)?)
    }

    /// A component's look in a display state: its own for the one shown now.
    pub fn look_in(&self, component: CompId, state: DisplayStateId) -> Option<Look> {
        let c = self.assembly()?.component(component)?;
        let s = self.display_state(state)?;
        Some(if self.is_shown(s) {
            Look::of(c)
        } else {
            s.stored_look(component)
        })
    }

    /// Whether `s` is the display state shown now.
    fn is_shown(&self, s: &DisplayState) -> bool {
        self.active_display_state().is_some_and(|a| a.id == s.id)
    }

    /// The components' looks now, as a display state that is not shown keeps them.
    fn live_looks(&self) -> BTreeMap<CompId, Look> {
        self.assembly()
            .map(|a| {
                a.components()
                    .map(|c| (c.id, Look::of(c)))
                    .filter(|(_, l)| *l != Look::DEFAULT)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Keeps the components' looks in `id`, which stops being shown.
    fn stash(&mut self, id: DisplayStateId) {
        let looks = self.live_looks();
        if let Some(s) = self.display_mut().and_then(|d| d.get_mut(id)) {
            s.looks = looks;
        }
    }

    /// Gives the components the looks `id` keeps, and it becomes the one shown.
    fn load(&mut self, id: DisplayStateId) {
        let Some(assembly) = self.assembly_mut() else {
            return;
        };
        let Some(looks) = assembly
            .display
            .get_mut(id)
            .map(|s| std::mem::take(&mut s.looks))
        else {
            return;
        };
        let ids: Vec<CompId> = assembly.components().map(|c| c.id).collect();
        for c in ids {
            let look = looks.get(&c).copied().unwrap_or_default();
            if assembly.component(c).is_some_and(|x| Look::of(x) != look)
                && let Some(x) = assembly.component_mut(c)
            {
                look.apply(x);
            }
        }
    }

    fn check_display_state_name(
        &self,
        name: &str,
        config: Option<ConfigId>,
        except: Option<DisplayStateId>,
    ) -> Result<(), String> {
        if name.is_empty() {
            return Err("A display state needs a name.".to_owned());
        }
        let d = self.display().ok_or_else(not_an_assembly)?;
        let clash = d.all().any(|s| {
            Some(s.id) != except && s.name == name && (!d.linked || s.configuration == config)
        });
        if clash {
            return Err(format!("Another display state is already called '{name}'."));
        }
        Ok(())
    }

    /// Adds a display state: of configuration `config` when display states are linked
    /// (`None`: the active configuration), of the assembly when they are not. It starts as
    /// the components look now, and is made the one shown (as SolidWorks' Add Display
    /// State) if it is for the active configuration; in another configuration it is that
    /// configuration's shown one only if it is its first.
    pub fn add_display_state(
        &mut self,
        name: &str,
        config: Option<ConfigId>,
    ) -> Result<DisplayStateId, String> {
        let name = name.trim();
        let linked = self.display().ok_or_else(not_an_assembly)?.linked;
        let active_config = self.active_configuration().id;
        let config = config.unwrap_or(active_config);
        if self.configuration(config).is_none() {
            return Err("The configuration no longer exists.".to_owned());
        }
        if !linked && config != active_config {
            return Err(
                "Display states are not linked to configurations: every configuration has them all, so leave 'configuration' out."
                    .to_owned(),
            );
        }
        let owner = linked.then_some(config);
        self.check_display_state_name(name, owner, None)?;
        let here = config == active_config;
        let current = self.active_display_state().map(|s| s.id);
        if here && let Some(current) = current {
            self.stash(current);
        }
        let looks = if here {
            BTreeMap::new()
        } else {
            self.live_looks()
        };
        let d = self.display_mut().ok_or_else(not_an_assembly)?;
        let first = d.active_in(config).is_none();
        if here {
            for s in &mut d.states {
                if s.active && (!linked || s.configuration == owner) {
                    s.active = false;
                }
            }
        }
        d.next += 1;
        let id = DisplayStateId(d.next);
        d.states.push(DisplayState {
            id,
            name: name.to_owned(),
            configuration: owner,
            active: here || first,
            looks,
        });
        Ok(id)
    }

    /// Shows display state `id`: the components take its looks (when it is the active
    /// configuration's), and it is the one its configuration shows. Returns whether that
    /// changed anything.
    pub fn activate_display_state(&mut self, id: DisplayStateId) -> bool {
        let Some(target) = self.display_state(id).cloned() else {
            return false;
        };
        if target.active {
            return false;
        }
        let d = self.display().expect("an assembly has it");
        let linked = d.linked;
        let config = target
            .configuration
            .unwrap_or(self.active_configuration().id);
        let here = config == self.active_configuration().id;
        if here && let Some(current) = self.active_display_state().map(|s| s.id) {
            self.stash(current);
        }
        if let Some(d) = self.display_mut() {
            for s in &mut d.states {
                if !linked || s.configuration == target.configuration {
                    s.active = s.id == id;
                }
            }
        }
        if here {
            self.load(id);
        }
        true
    }

    pub fn rename_display_state(&mut self, id: DisplayStateId, name: &str) -> Result<(), String> {
        let name = name.trim();
        let config = self
            .display_state(id)
            .ok_or_else(|| "The display state no longer exists.".to_owned())?
            .configuration;
        self.check_display_state_name(name, config, Some(id))?;
        if let Some(s) = self.display_mut().and_then(|d| d.get_mut(id)) {
            name.clone_into(&mut s.name);
        }
        Ok(())
    }

    /// Deletes a display state. A configuration (or, when they are not linked, the
    /// assembly) keeps at least one, as in SolidWorks; deleting the one shown shows its
    /// neighbour.
    pub fn remove_display_state(&mut self, id: DisplayStateId) -> Result<(), String> {
        let s = self
            .display_state(id)
            .cloned()
            .ok_or_else(|| "The display state no longer exists.".to_owned())?;
        let d = self.display().ok_or_else(not_an_assembly)?;
        let linked = d.linked;
        let siblings: Vec<DisplayStateId> = d
            .all()
            .filter(|o| !linked || o.configuration == s.configuration)
            .map(|o| o.id)
            .collect();
        if siblings.len() <= 1 {
            return Err(format!(
                "'{}' is the only display state {}: one is always shown, so it stays.",
                s.name,
                if linked {
                    "of its configuration"
                } else {
                    "of the assembly"
                }
            ));
        }
        if s.active {
            let at = siblings.iter().position(|o| *o == id).unwrap_or(0);
            let next = if at > 0 {
                siblings[at - 1]
            } else {
                siblings[1]
            };
            self.activate_display_state(next);
        }
        if let Some(d) = self.display_mut() {
            d.states.retain(|o| o.id != id);
        }
        Ok(())
    }

    /// Links display states to configurations (each has its own), or makes them
    /// independent (every configuration has them all). Linking gives every display state to
    /// the active configuration; unlinking keeps the one shown now. Returns whether that
    /// changed anything.
    pub fn link_display_states(&mut self, on: bool) -> bool {
        let config = self.active_configuration().id;
        let current = self.active_display_state().map(|s| s.id);
        let Some(d) = self.display_mut() else {
            return false;
        };
        if d.linked == on {
            return false;
        }
        d.linked = on;
        for s in &mut d.states {
            if on {
                s.configuration = Some(config);
            } else {
                s.configuration = None;
                s.active = Some(s.id) == current;
            }
        }
        if !on && current.is_none() && !d.states.is_empty() {
            // Nothing was shown in this configuration: what the components look like now is
            // one of the display states.
            let name = d.new_name(config);
            d.next += 1;
            let id = DisplayStateId(d.next);
            d.states.push(DisplayState {
                id,
                name,
                configuration: None,
                active: true,
                looks: BTreeMap::new(),
            });
        }
        true
    }

    /// Changes a component's look in the display states `states` (`None`: the one shown
    /// now, or the component itself if the assembly has none).
    pub fn set_look(
        &mut self,
        component: CompId,
        states: Option<&[DisplayStateId]>,
        change: impl Fn(&mut Look),
    ) -> Result<(), String> {
        let assembly = self.assembly().ok_or_else(not_an_assembly)?;
        if assembly.component(component).is_none() {
            return Err("The component no longer exists.".to_owned());
        }
        let shown = self.active_display_state().map(|s| s.id);
        let targets: Vec<Option<DisplayStateId>> = match states {
            None => vec![shown],
            Some(ids) => {
                if ids.is_empty() {
                    return Err("No display state was named.".to_owned());
                }
                for id in ids {
                    if self.display_state(*id).is_none() {
                        return Err("The display state no longer exists.".to_owned());
                    }
                }
                ids.iter().copied().map(Some).collect()
            }
        };
        for target in targets {
            if target == shown {
                if let Some(c) = self.assembly_mut().and_then(|a| a.component_mut(component)) {
                    let mut look = Look::of(c);
                    change(&mut look);
                    look.apply(c);
                }
            } else if let Some(s) = target.and_then(|id| self.display_mut()?.get_mut(id)) {
                let mut look = s.looks.get(&component).copied().unwrap_or_default();
                change(&mut look);
                if look == Look::DEFAULT {
                    s.looks.remove(&component);
                } else {
                    s.looks.insert(component, look);
                }
            }
        }
        Ok(())
    }

    /// With display states linked: the configuration `old` stops being shown and `new`
    /// starts, so the display state shown changes with it. A configuration that has none
    /// yet gets one, as the components look now (SolidWorks gives each configuration one).
    pub(crate) fn swap_display_states(&mut self, old: ConfigId, new: ConfigId) {
        let Some(d) = self.display() else {
            return;
        };
        if !d.linked || d.is_empty() {
            return;
        }
        let leaving = d.active_in(old);
        let first = d.of(new).next().map(|s| s.id);
        let coming = d.active_in(new).or(first);
        if let Some(leaving) = leaving {
            self.stash(leaving);
        }
        let Some(d) = self.display_mut() else {
            return;
        };
        match coming {
            Some(id) => {
                if let Some(s) = d.get_mut(id) {
                    s.active = true;
                }
                self.load(id);
            }
            None => {
                let name = d.new_name(new);
                d.next += 1;
                let id = DisplayStateId(d.next);
                d.states.push(DisplayState {
                    id,
                    name,
                    configuration: Some(new),
                    active: true,
                    looks: BTreeMap::new(),
                });
            }
        }
    }

    /// Drops the display states of configurations that are gone.
    pub(crate) fn forget_display_states(&mut self) {
        let known: Vec<ConfigId> = self.configurations().iter().map(|c| c.id).collect();
        if let Some(d) = self.display_mut() {
            d.states
                .retain(|s| s.configuration.is_none_or(|c| known.contains(&c)));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use peet_math::Frame;

    use super::*;

    /// An assembly of two components of one part.
    fn two() -> (Model, CompId, CompId) {
        let (part, _) = crate::samples::bracket();
        let mut m = Model::new_assembly();
        let a = m.assembly_mut().unwrap();
        let def = a.define(Arc::new(part));
        let one = a.insert(def, Frame::WORLD).unwrap();
        let two = a.insert(def, Frame::WORLD).unwrap();
        (m, one, two)
    }

    fn component(m: &Model, id: CompId) -> &Component {
        m.assembly().unwrap().component(id).unwrap()
    }

    #[test]
    fn each_display_state_keeps_its_own_looks() {
        let (mut m, one, two) = two();
        let a = m.add_display_state("All", None).unwrap();
        let b = m.add_display_state("See through", None).unwrap();
        assert_eq!(m.active_display_state().unwrap().id, b);
        m.set_look(one, None, |l| l.transparency = 0.75).unwrap();
        m.set_look(two, None, |l| {
            l.visible = false;
            l.color = Some([200, 0, 0]);
        })
        .unwrap();
        assert!(m.activate_display_state(a));
        assert_eq!(Look::of(component(&m, one)), Look::DEFAULT);
        assert_eq!(Look::of(component(&m, two)), Look::DEFAULT);
        assert_eq!(m.look_in(one, b).unwrap().transparency, 0.75);
        assert!(m.activate_display_state(b));
        assert_eq!(component(&m, one).transparency, 0.75);
        assert!(!component(&m, two).visible);
        assert_eq!(component(&m, two).color, Some([200, 0, 0]));
        // A look set in a display state that is not shown leaves the components alone.
        m.set_look(one, Some(&[a]), |l| l.color = Some([1, 2, 3]))
            .unwrap();
        assert_eq!(component(&m, one).color, None);
        assert_eq!(m.look_in(one, a).unwrap().color, Some([1, 2, 3]));
        // One always stays.
        m.remove_display_state(b).unwrap();
        assert_eq!(m.active_display_state().unwrap().id, a);
        assert_eq!(component(&m, one).color, Some([1, 2, 3]));
        assert!(m.remove_display_state(a).is_err());
        assert!(m.add_display_state("All", None).is_err());
    }

    #[test]
    fn linked_display_states_change_with_the_configuration() {
        let (mut m, one, _) = two();
        m.link_display_states(true);
        let base = m.active_configuration().id;
        let plain = m.add_display_state("Plain", None).unwrap();
        let other = m.add_configuration("Other", None).unwrap();
        // A display state of another configuration, its first: that one shows it.
        let red = m.add_display_state("Red", Some(other)).unwrap();
        assert_eq!(m.active_display_state().unwrap().id, plain);
        m.set_look(one, Some(&[red]), |l| l.color = Some([255, 0, 0]))
            .unwrap();
        assert!(m.activate_configuration(other));
        assert_eq!(m.active_display_state().unwrap().id, red);
        assert_eq!(component(&m, one).color, Some([255, 0, 0]));
        assert!(m.activate_configuration(base));
        assert_eq!(m.active_display_state().unwrap().id, plain);
        assert_eq!(component(&m, one).color, None);
        // Names are a configuration's own.
        assert!(m.add_display_state("Red", None).is_ok());
        // A configuration that had none gets one.
        let third = m.add_configuration("Third", None).unwrap();
        m.activate_configuration(third);
        assert_eq!(m.display_states().len(), 1);
        // Its display states go with it.
        m.activate_configuration(base);
        m.remove_configuration(other).unwrap();
        assert!(m.display_state(red).is_none());
    }

    #[test]
    fn unlinking_keeps_the_one_shown() {
        let (mut m, one, _) = two();
        let a = m.add_display_state("A", None).unwrap();
        m.set_look(one, None, |l| l.transparency = 0.5).unwrap();
        assert!(m.link_display_states(true));
        assert_eq!(m.active_display_state().unwrap().id, a);
        assert!(m.link_display_states(false));
        assert_eq!(m.active_display_state().unwrap().id, a);
        assert_eq!(component(&m, one).transparency, 0.5);
    }
}
