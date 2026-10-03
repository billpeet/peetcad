//! Assemblies: parts placed relative to each other.
//!
//! An assembly is a [`Model`] with no features of its own and an [`Assembly`]: a list of
//! *definitions* (the parts it uses, each a whole [`Model`], kept in the assembly) and a
//! list of *components* (each an instance of a definition, with a placement). Any number
//! of components share one definition, so a part used ten times is stored, rebuilt and
//! drawn once. A definition can itself be an assembly: a sub-assembly, placed as one
//! rigid thing.
//!
//! The design is recorded in `docs/adr/0009-assemblies.md`.

use std::sync::Arc;

use peet_math::Frame;
use serde::{Deserialize, Serialize};

use crate::Model;

/// Identifies a definition within its assembly. Ids are not reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DefId(pub u32);

/// Identifies a component within its assembly. Ids are not reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CompId(pub u32);

/// A part (or a sub-assembly) an assembly uses. Its name is its model's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Definition {
    pub id: DefId,
    pub model: Arc<Model>,
}

impl Definition {
    pub fn name(&self) -> &str {
        &self.model.name
    }
}

/// One placed instance of a definition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Component {
    pub id: CompId,
    /// Unique in the assembly: "Bracket-2".
    pub name: String,
    pub definition: DefId,
    /// Where the part's origin and axes are in the assembly.
    pub placement: Frame,
    /// Held where it is: mates move the other components, not this one.
    pub fixed: bool,
    pub visible: bool,
    /// Left out of the assembly (not drawn, not counted) without being deleted.
    pub suppressed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Assembly {
    /// Shared pointers make snapshots (for undo) cheap: a snapshot copies only what
    /// changes afterwards.
    definitions: Vec<Arc<Definition>>,
    /// In the order they were inserted: the order of the tree.
    components: Vec<Arc<Component>>,
    next_definition: u32,
    next_component: u32,
}

impl Assembly {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- Reading ----

    pub fn definitions(&self) -> impl ExactSizeIterator<Item = &Definition> {
        self.definitions.iter().map(|d| &**d)
    }

    pub fn definition(&self, id: DefId) -> Option<&Definition> {
        self.definitions.iter().find(|d| d.id == id).map(|d| &**d)
    }

    pub fn components(&self) -> impl ExactSizeIterator<Item = &Component> {
        self.components.iter().map(|c| &**c)
    }

    pub fn component(&self, id: CompId) -> Option<&Component> {
        self.components.iter().find(|c| c.id == id).map(|c| &**c)
    }

    /// The component's name, or a placeholder if it no longer exists.
    pub fn name_of(&self, id: CompId) -> &str {
        self.component(id)
            .map_or("a deleted component", |c| &c.name)
    }

    /// How many components are instances of the definition.
    pub fn uses(&self, id: DefId) -> usize {
        self.components
            .iter()
            .filter(|c| c.definition == id)
            .count()
    }

    /// The definition a component is an instance of.
    pub fn definition_of(&self, id: CompId) -> Option<&Definition> {
        self.definition(self.component(id)?.definition)
    }

    // ---- Editing ----

    /// The definition for `model`: the one that already has exactly this model, or a new
    /// one. So the same part inserted twice is stored once.
    pub fn define(&mut self, model: Arc<Model>) -> DefId {
        if let Some(d) = self
            .definitions
            .iter()
            .find(|d| Arc::ptr_eq(&d.model, &model) || *d.model == *model)
        {
            return d.id;
        }
        self.next_definition += 1;
        let id = DefId(self.next_definition);
        self.definitions.push(Arc::new(Definition { id, model }));
        id
    }

    /// A name no component has: the definition's name and the first free number.
    fn component_name(&self, definition: DefId) -> String {
        let base = self
            .definition(definition)
            .map_or("Component", Definition::name);
        (1..)
            .map(|n| format!("{base}-{n}"))
            .find(|name| !self.components.iter().any(|c| c.name == *name))
            .unwrap_or_default()
    }

    /// Places an instance of a definition. The first component of an assembly is fixed:
    /// something has to hold the rest in place.
    pub fn insert(&mut self, definition: DefId, placement: Frame) -> Option<CompId> {
        self.definition(definition)?;
        self.next_component += 1;
        let id = CompId(self.next_component);
        let name = self.component_name(definition);
        let fixed = self.components.is_empty();
        self.components.push(Arc::new(Component {
            id,
            name,
            definition,
            placement,
            fixed,
            visible: true,
            suppressed: false,
        }));
        Some(id)
    }

    /// The component, for changing it. Other snapshots of the assembly are not affected.
    pub fn component_mut(&mut self, id: CompId) -> Option<&mut Component> {
        self.components
            .iter_mut()
            .find(|c| c.id == id)
            .map(Arc::make_mut)
    }

    /// Removes a component, and its definition if nothing else uses it.
    pub fn remove(&mut self, id: CompId) -> Option<Component> {
        let index = self.components.iter().position(|c| c.id == id)?;
        let removed = self.components.remove(index);
        self.prune();
        Some(Arc::unwrap_or_clone(removed))
    }

    /// Makes a component an instance of another definition, where it is.
    pub fn replace(&mut self, id: CompId, definition: DefId) -> bool {
        if self.definition(definition).is_none() {
            return false;
        }
        let Some(c) = self.component_mut(id) else {
            return false;
        };
        c.definition = definition;
        self.prune();
        true
    }

    /// Replaces a definition's model: how an edit to a part reaches every instance of it.
    pub fn set_model(&mut self, id: DefId, model: Arc<Model>) -> bool {
        match self.definitions.iter_mut().find(|d| d.id == id) {
            Some(d) => {
                Arc::make_mut(d).model = model;
                true
            }
            None => false,
        }
    }

    /// Drops the definitions no component uses.
    fn prune(&mut self) {
        let components = &self.components;
        self.definitions
            .retain(|d| components.iter().any(|c| c.definition == d.id));
    }

    /// Checks what a file can get wrong, and repairs the counters.
    pub(crate) fn validate(&mut self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for d in &self.definitions {
            if !seen.insert(d.id.0) {
                return Err(format!(
                    "The assembly is damaged: two parts have the id {}.",
                    d.id.0
                ));
            }
        }
        let mut seen = std::collections::HashSet::new();
        for c in &self.components {
            if !seen.insert(c.id.0) {
                return Err(format!(
                    "The assembly is damaged: two components have the id {}.",
                    c.id.0
                ));
            }
            if self.definition(c.definition).is_none() {
                return Err(format!(
                    "The assembly is damaged: {} is an instance of a part that is not in the file.",
                    c.name
                ));
            }
            let p = &c.placement;
            if !(p.origin.is_finite() && p.rotation.is_finite() && p.rotation.is_normalized()) {
                return Err(format!(
                    "The assembly is damaged: {} has a placement that is not a position and a rotation.",
                    c.name
                ));
            }
        }
        let max = |ids: &mut dyn Iterator<Item = u32>| ids.max().unwrap_or(0);
        self.next_definition = self
            .next_definition
            .max(max(&mut self.definitions.iter().map(|d| d.id.0)));
        self.next_component = self
            .next_component
            .max(max(&mut self.components.iter().map(|c| c.id.0)));
        for d in &mut self.definitions {
            let mut model = (*d.model).clone();
            model.validate()?;
            if model != *d.model {
                Arc::make_mut(d).model = Arc::new(model);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peet_math::DVec3;

    fn part(name: &str) -> Arc<Model> {
        let mut m = Model::new();
        name.clone_into(&mut m.name);
        Arc::new(m)
    }

    fn at(x: f64) -> Frame {
        Frame {
            origin: DVec3::new(x, 0.0, 0.0),
            ..Frame::WORLD
        }
    }

    #[test]
    fn components_are_instances_of_definitions() {
        let mut a = Assembly::new();
        let plate = a.define(part("Plate"));
        // The same part again is the same definition, by pointer or by content.
        assert_eq!(a.define(part("Plate")), plate);
        let screw = a.define(part("Screw"));
        assert_ne!(screw, plate);

        let base = a.insert(plate, Frame::WORLD).unwrap();
        let s1 = a.insert(screw, at(10.0)).unwrap();
        let s2 = a.insert(screw, at(20.0)).unwrap();
        assert!(a.insert(DefId(99), Frame::WORLD).is_none());
        let names: Vec<&str> = a.components().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Plate-1", "Screw-1", "Screw-2"]);
        // Only the first is fixed.
        assert!(a.component(base).unwrap().fixed && !a.component(s1).unwrap().fixed);
        assert_eq!((a.uses(plate), a.uses(screw)), (1, 2));
        assert_eq!(a.definition_of(s2).unwrap().name(), "Screw");

        // A removed component's number is free again; its definition goes with the last
        // instance.
        a.remove(s1).unwrap();
        assert_eq!(a.definitions().count(), 2);
        let s3 = a.insert(screw, at(30.0)).unwrap();
        assert_eq!(a.name_of(s3), "Screw-1");
        assert!(s3 > s2, "ids are not reused");
        a.remove(s2);
        a.remove(s3);
        assert_eq!(a.definitions().count(), 1);
        assert_eq!(a.name_of(s3), "a deleted component");

        // Replacing keeps the component and its place.
        let nut = a.define(part("Nut"));
        a.component_mut(base).unwrap().placement = at(5.0);
        assert!(a.replace(base, nut));
        assert_eq!(a.component(base).unwrap().placement, at(5.0));
        assert_eq!(a.definitions().count(), 1, "the plate is gone");
        assert!(!a.replace(base, plate));

        // An edit to a part is a new model for its definition.
        assert!(a.set_model(nut, part("Lock nut")));
        assert_eq!(a.definition_of(base).unwrap().name(), "Lock nut");
        assert!(!a.set_model(plate, part("Plate")));
    }

    #[test]
    fn snapshots_share_what_did_not_change() {
        let mut a = Assembly::new();
        let plate = a.define(part("Plate"));
        let c1 = a.insert(plate, Frame::WORLD).unwrap();
        let c2 = a.insert(plate, at(50.0)).unwrap();
        let before = a.clone();
        a.component_mut(c2).unwrap().placement = at(60.0);
        assert_ne!(a, before);
        assert_eq!(before.component(c2).unwrap().placement, at(50.0));
        assert!(Arc::ptr_eq(&a.components[0], &before.components[0]));
        assert!(Arc::ptr_eq(&a.definitions[0], &before.definitions[0]));
        assert_eq!(a.component(c1), before.component(c1));
    }

    #[test]
    fn damaged_assemblies_are_refused_and_counters_repaired() {
        let mut a = Assembly::new();
        let plate = a.define(part("Plate"));
        let c = a.insert(plate, Frame::WORLD).unwrap();
        let mut ok = a.clone();
        ok.next_component = 0;
        ok.next_definition = 0;
        ok.validate().unwrap();
        assert!(ok.insert(plate, Frame::WORLD).unwrap() > c);

        let mut twice = a.clone();
        let copy = twice.components[0].clone();
        twice.components.push(copy);
        assert!(twice.validate().unwrap_err().contains("two components"));

        let mut orphan = a.clone();
        orphan.definitions.clear();
        assert!(orphan.validate().unwrap_err().contains("not in the file"));

        let mut bent = a.clone();
        bent.component_mut(c).unwrap().placement.rotation =
            peet_math::DQuat::from_xyzw(0.0, 0.0, 0.0, 3.0);
        assert!(bent.validate().unwrap_err().contains("placement"));
    }
}
