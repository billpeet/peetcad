//! Assemblies: parts placed relative to each other.
//!
//! An assembly is a [`Model`] with no features of its own and an [`Assembly`]: a list of
//! *definitions* (the parts it uses, each a whole [`Model`], kept in the assembly) and a
//! list of *components* (each an instance of a definition, with a placement). Any number
//! of components share one definition, so a part used ten times is stored, rebuilt and
//! drawn once. A definition can itself be an assembly: a sub-assembly, placed as one
//! rigid thing.
//!
//! *Mates* ([`crate::Mate`]) hold components together: they are solved when the assembly
//! is rebuilt, and the placements they come to are stored in the components.
//!
//! The design is recorded in `docs/adr/0009-assemblies.md`.

use std::path::{Component as Step, Path, PathBuf};
use std::sync::Arc;

use peet_math::{DVec3, Frame};
use serde::{Deserialize, Serialize};

use crate::Model;
use crate::mate::{Mate, MateEnd, MateId, MateKind};

/// Identifies a definition within its assembly. Ids are not reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DefId(pub u32);

/// Identifies a component within its assembly. Ids are not reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CompId(pub u32);

/// Where a part that is not the assembly's own comes from: a file of its own, which
/// other assemblies can use too.
///
/// While an assembly is open, `path` is the file's full path. In the assembly's file a
/// `relative` link is written relative to the folder the file is in (`../parts/pin.peet`),
/// so the assembly and its parts can be moved or copied together, keeping their layout:
/// see [`Model::for_file`] and [`Model::from_file`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub path: String,
    /// Written relative to the assembly's folder (the default), not as a full path.
    #[serde(default)]
    pub relative: bool,
}

/// A path without its `.` and `..` steps, as far as that can be told from the text.
fn tidy(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Step::CurDir => {}
            Step::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// `path` as seen from `folder`, with forward slashes: `../parts/pin.peet`. `None` if
/// there is no way from one to the other (they are on different drives), or either is
/// not a full path.
pub fn relative_to(path: &Path, folder: &Path) -> Option<String> {
    if !path.is_absolute() || !folder.is_absolute() {
        return None;
    }
    let (path, folder) = (tidy(path), tidy(folder));
    let same = |a: &Step<'_>, b: &Step<'_>| {
        if cfg!(windows) {
            a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
        } else {
            a == b
        }
    };
    let (to, from): (Vec<_>, Vec<_>) = (path.components().collect(), folder.components().collect());
    let shared = to.iter().zip(&from).take_while(|(a, b)| same(a, b)).count();
    // Nothing in common, or only the root of two drives: no way across.
    if shared == 0 || matches!(to.first(), Some(Step::Prefix(_))) && shared < 2 {
        return None;
    }
    let mut steps: Vec<String> = from[shared..].iter().map(|_| "..".to_owned()).collect();
    steps.extend(
        to[shared..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    Some(steps.join("/"))
}

impl Model {
    /// The model as it is written to a file in `folder`: its relative links (and those
    /// of the assemblies in it) relative to that folder. With no folder (a file that is
    /// nowhere yet), or where there is no way across, they stay full paths.
    #[must_use]
    pub fn for_file(&self, folder: Option<&Path>) -> Self {
        self.with_links(&|link| {
            let relative = folder
                .filter(|_| link.relative)
                .and_then(|folder| relative_to(Path::new(&link.path), folder));
            relative.map(|path| Link {
                path,
                relative: true,
            })
        })
    }

    /// Makes the links of a model read from a file in `folder` full paths again.
    pub fn from_file(&mut self, folder: Option<&Path>) {
        *self = self.with_links(&|link| {
            let path = Path::new(&link.path);
            let folder = folder.filter(|_| !path.is_absolute())?;
            Some(Link {
                path: tidy(&folder.join(path)).to_string_lossy().into_owned(),
                relative: link.relative,
            })
        });
    }

    /// The model with each link (here and in the assemblies in it) replaced by what
    /// `change` gives for it, if anything. What doesn't change is shared, not copied.
    fn with_links(&self, change: &dyn Fn(&Link) -> Option<Link>) -> Self {
        let mut out = self.clone();
        let Some(assembly) = self.assembly() else {
            return out;
        };
        for d in assembly.definitions() {
            let link = d.link.as_ref().and_then(change);
            let inner = d.model.is_assembly().then(|| d.model.with_links(change));
            let Some(assembly) = out.assembly_mut() else {
                break;
            };
            if let Some(link) = link {
                assembly.set_link(d.id, Some(link));
            }
            if let Some(inner) = inner.filter(|inner| *inner != *d.model) {
                assembly.set_model(d.id, Arc::new(inner));
            }
        }
        out
    }
}

/// A part (or a sub-assembly) an assembly uses. Its name is its model's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Definition {
    pub id: DefId,
    /// The part. For a linked part: as it was when its file was last read, so the
    /// assembly still opens when the file can't be found.
    pub model: Arc<Model>,
    /// The file the part is read from, if it is linked and not the assembly's own.
    #[serde(default)]
    pub link: Option<Link>,
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
    /// A colour of its own, in place of its part's: for telling instances apart. A
    /// sub-assembly's goes for everything in it.
    #[serde(default)]
    pub color: Option<[u8; 3]>,
}

/// Identifies a step of an assembly's exploded view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ExplodeId(pub u32);

/// One step of an assembly's exploded view: some components moved away from where
/// they are, to show how the assembly goes together. A view only: the components'
/// placements and the mates are not touched.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExplodeStep {
    pub id: ExplodeId,
    /// Unique in the assembly: "Explode1".
    pub name: String,
    /// The components of the assembly it moves.
    pub components: Vec<CompId>,
    /// How far, in the assembly's coordinates (mm). A component in several steps is
    /// moved by the sum of them.
    pub offset: DVec3,
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
    /// In the order they were added: when they can't all hold, the earlier ones win.
    mates: Vec<Arc<Mate>>,
    next_mate: u32,
    /// The steps of the exploded view, in the order they were added.
    #[serde(default)]
    explode: Vec<Arc<ExplodeStep>>,
    #[serde(default)]
    next_explode: u32,
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

    pub fn mates(&self) -> impl ExactSizeIterator<Item = &Mate> {
        self.mates.iter().map(|m| &**m)
    }

    pub fn mate(&self, id: MateId) -> Option<&Mate> {
        self.mates.iter().find(|m| m.id == id).map(|m| &**m)
    }

    /// The mates with an end on the component.
    pub fn mates_of(&self, id: CompId) -> impl Iterator<Item = &Mate> {
        self.mates()
            .filter(move |m| m.a.component() == Some(id) || m.b.component() == Some(id))
    }

    pub fn explode_steps(&self) -> impl ExactSizeIterator<Item = &ExplodeStep> {
        self.explode.iter().map(|s| &**s)
    }

    pub fn explode_step(&self, id: ExplodeId) -> Option<&ExplodeStep> {
        self.explode.iter().find(|s| s.id == id).map(|s| &**s)
    }

    /// How far the exploded view moves a component: the sum of its steps.
    pub fn explode_offset(&self, id: CompId) -> DVec3 {
        self.explode
            .iter()
            .filter(|s| s.components.contains(&id))
            .map(|s| s.offset)
            .sum()
    }

    /// The definition a component is an instance of.
    pub fn definition_of(&self, id: CompId) -> Option<&Definition> {
        self.definition(self.component(id)?.definition)
    }

    // ---- Editing ----

    /// The definition for `model`, as a part of the assembly's own: the one that already
    /// has exactly this model, or a new one. So the same part inserted twice is stored
    /// once.
    pub fn define(&mut self, model: Arc<Model>) -> DefId {
        if let Some(d) = self
            .definitions
            .iter()
            .find(|d| d.link.is_none() && (Arc::ptr_eq(&d.model, &model) || *d.model == *model))
        {
            return d.id;
        }
        self.new_definition(model, None)
    }

    /// The definition for the part in the file `link`, which is `model` now: the one
    /// already linked to that file (given this model), or a new one.
    pub fn define_linked(&mut self, model: Arc<Model>, link: Link) -> DefId {
        let found = self
            .definitions
            .iter()
            .find(|d| d.link.as_ref() == Some(&link))
            .map(|d| d.id);
        match found {
            Some(id) => {
                self.set_model(id, model);
                id
            }
            None => self.new_definition(model, Some(link)),
        }
    }

    fn new_definition(&mut self, model: Arc<Model>, link: Option<Link>) -> DefId {
        self.next_definition += 1;
        let id = DefId(self.next_definition);
        self.definitions
            .push(Arc::new(Definition { id, model, link }));
        id
    }

    /// Links a definition to a file, or (with `None`) makes it the assembly's own.
    pub fn set_link(&mut self, id: DefId, link: Option<Link>) -> bool {
        match self.definitions.iter_mut().find(|d| d.id == id) {
            Some(d) => {
                if d.link != link {
                    Arc::make_mut(d).link = link;
                }
                true
            }
            None => false,
        }
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
            color: None,
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

    /// Adds a mate between two ends, with an automatic name (`Coincident1`).
    pub fn add_mate(&mut self, kind: MateKind, a: MateEnd, b: MateEnd) -> MateId {
        self.next_mate += 1;
        let id = MateId(self.next_mate);
        let prefix = kind.name_prefix();
        let name = (1..)
            .map(|n| format!("{prefix}{n}"))
            .find(|name| !self.mates.iter().any(|m| m.name == *name))
            .unwrap_or_default();
        self.mates.push(Arc::new(Mate {
            id,
            name,
            kind,
            a,
            b,
            flip: false,
            suppressed: false,
        }));
        id
    }

    /// The mate, for changing it. Other snapshots of the assembly are not affected.
    pub fn mate_mut(&mut self, id: MateId) -> Option<&mut Mate> {
        self.mates
            .iter_mut()
            .find(|m| m.id == id)
            .map(Arc::make_mut)
    }

    /// Adds a step to the exploded view, with an automatic name (`Explode1`). The
    /// components that are not of this assembly are left out, and each is taken once.
    pub fn add_explode_step(&mut self, components: &[CompId], offset: DVec3) -> ExplodeId {
        self.next_explode += 1;
        let id = ExplodeId(self.next_explode);
        let name = (1..)
            .map(|n| format!("Explode{n}"))
            .find(|name| !self.explode.iter().any(|s| s.name == *name))
            .unwrap_or_default();
        let mut own = Vec::new();
        for c in components {
            if self.component(*c).is_some() && !own.contains(c) {
                own.push(*c);
            }
        }
        self.explode.push(Arc::new(ExplodeStep {
            id,
            name,
            components: own,
            offset,
        }));
        id
    }

    /// The step, for changing it. Other snapshots of the assembly are not affected.
    pub fn explode_step_mut(&mut self, id: ExplodeId) -> Option<&mut ExplodeStep> {
        self.explode
            .iter_mut()
            .find(|s| s.id == id)
            .map(Arc::make_mut)
    }

    pub fn remove_explode_step(&mut self, id: ExplodeId) -> Option<ExplodeStep> {
        let index = self.explode.iter().position(|s| s.id == id)?;
        Some(Arc::unwrap_or_clone(self.explode.remove(index)))
    }

    /// Takes components that no longer exist out of the explode steps, and drops the
    /// steps that are left with none.
    fn prune_explode(&mut self) {
        let components = &self.components;
        let known = |id: &CompId| components.iter().any(|c| c.id == *id);
        for step in &mut self.explode {
            if !step.components.iter().all(known) {
                Arc::make_mut(step).components.retain(known);
            }
        }
        self.explode.retain(|s| !s.components.is_empty());
    }

    pub fn remove_mate(&mut self, id: MateId) -> Option<Mate> {
        let index = self.mates.iter().position(|m| m.id == id)?;
        Some(Arc::unwrap_or_clone(self.mates.remove(index)))
    }

    /// Removes a component, with its mates, and its definition if nothing else uses it.
    pub fn remove(&mut self, id: CompId) -> Option<Component> {
        let index = self.components.iter().position(|c| c.id == id)?;
        let removed = self.components.remove(index);
        self.mates
            .retain(|m| m.a.component() != Some(id) && m.b.component() != Some(id));
        self.prune_explode();
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
                if !Arc::ptr_eq(&d.model, &model) {
                    Arc::make_mut(d).model = model;
                }
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
        let mut seen = std::collections::HashSet::new();
        for m in &self.mates {
            if !seen.insert(m.id.0) {
                return Err(format!(
                    "The assembly is damaged: two mates have the id {}.",
                    m.id.0
                ));
            }
            if m.a.path.is_empty() || m.b.path.is_empty() {
                return Err(format!(
                    "The assembly is damaged: {} is not on two components.",
                    m.name
                ));
            }
        }
        self.next_mate = self
            .next_mate
            .max(max(&mut self.mates.iter().map(|m| m.id.0)));
        let mut seen = std::collections::HashSet::new();
        for s in &self.explode {
            if !seen.insert(s.id.0) {
                return Err(format!(
                    "The assembly is damaged: two explode steps have the id {}.",
                    s.id.0
                ));
            }
            if !s.offset.is_finite() {
                return Err(format!(
                    "The assembly is damaged: {} moves its components by something that is not a distance.",
                    s.name
                ));
            }
        }
        self.prune_explode();
        self.next_explode = self
            .next_explode
            .max(max(&mut self.explode.iter().map(|s| s.id.0)));
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

        // A linked part is not the same definition as the assembly's own copy of it, and
        // the same file linked twice is one definition, with the model last read.
        let own = a.define(part("Washer"));
        let file = |path: &str| Link {
            path: path.to_owned(),
            relative: true,
        };
        let linked = a.define_linked(part("Washer"), file("C:/parts/washer.peet"));
        assert_ne!(linked, own);
        assert_eq!(
            a.define_linked(part("Washer 2"), file("C:/parts/washer.peet")),
            linked
        );
        assert_eq!(a.definition(linked).unwrap().name(), "Washer 2");
        assert_ne!(
            a.define_linked(part("Washer"), file("C:/parts/other.peet")),
            linked
        );
        assert!(a.set_link(linked, None) && a.definition(linked).unwrap().link.is_none());
        assert!(!a.set_link(DefId(99), None));
        // (None of them is used by a component: they go at the next removal.)

        // An edit to a part is a new model for its definition.
        assert!(a.set_model(nut, part("Lock nut")));
        assert_eq!(a.definition_of(base).unwrap().name(), "Lock nut");
        assert!(!a.set_model(plate, part("Plate")));
    }

    #[test]
    fn relative_links_are_written_from_the_assemblys_folder() {
        let (root, other) = if cfg!(windows) {
            ("C:\\work", "D:\\parts\\pin.peet")
        } else {
            ("/work", "")
        };
        let at = |rest: &str| Path::new(root).join(rest);
        let way = |path: &str, folder: &str| relative_to(&at(path), &at(folder));
        assert_eq!(way("job/pin.peet", "job").as_deref(), Some("pin.peet"));
        assert_eq!(
            way("job/parts/pin.peet", "job").as_deref(),
            Some("parts/pin.peet")
        );
        assert_eq!(
            way("lib/pin.peet", "job/asm").as_deref(),
            Some("../../lib/pin.peet")
        );
        assert_eq!(
            way("job/./x/../pin.peet", "job").as_deref(),
            Some("pin.peet")
        );
        assert_eq!(relative_to(Path::new("pin.peet"), &at("job")), None);
        if cfg!(windows) {
            assert_eq!(
                relative_to(Path::new(other), &at("job")),
                None,
                "another drive"
            );
            assert_eq!(way("JOB/pin.peet", "job").as_deref(), Some("pin.peet"));
        }

        // An assembly with a relative and a full link, and a sub-assembly with one.
        // (With the separators of this system throughout, as a full path has them.)
        let full = |rest: &str| tidy(&at(rest)).to_string_lossy().into_owned();
        let link = |rest: &str, relative| Link {
            path: full(rest),
            relative,
        };
        let mut sub = Model::new_assembly();
        sub.name = "Sub".to_owned();
        {
            let a = sub.assembly_mut().unwrap();
            let d = a.define_linked(part("Nut"), link("lib/nut.peet", true));
            a.insert(d, Frame::WORLD);
        }
        let mut model = Model::new_assembly();
        {
            let a = model.assembly_mut().unwrap();
            let pin = a.define_linked(part("Pin"), link("job/parts/pin.peet", true));
            let cap = a.define_linked(part("Cap"), link("lib/cap.peet", false));
            let sub = a.define(Arc::new(sub));
            for d in [pin, cap, sub] {
                a.insert(d, Frame::WORLD);
            }
        }
        let paths = |m: &Model| -> Vec<String> {
            let a = m.assembly().unwrap();
            let mut out: Vec<String> = a
                .definitions()
                .filter_map(|d| d.link.as_ref().map(|l| l.path.clone()))
                .collect();
            for d in a.definitions().filter(|d| d.model.is_assembly()) {
                let inner = d.model.assembly().unwrap();
                out.extend(
                    inner
                        .definitions()
                        .filter_map(|d| d.link.as_ref().map(|l| l.path.clone())),
                );
            }
            out
        };
        // In a file in the job folder: the relative ones from there, the full one as is.
        let written = model.for_file(Some(&at("job")));
        assert_eq!(
            paths(&written),
            [
                "parts/pin.peet".to_owned(),
                full("lib/cap.peet"),
                "../lib/nut.peet".to_owned()
            ]
        );
        // Read back there, it is the model that was written; read in a copy of the
        // folder elsewhere, the relative links point into the copy.
        let mut back = written.clone();
        back.from_file(Some(&at("job")));
        assert_eq!(back, model);
        let mut copy = written.clone();
        copy.from_file(Some(&at("backup/job")));
        assert_eq!(
            paths(&copy),
            [
                full("backup/job/parts/pin.peet"),
                full("lib/cap.peet"),
                full("backup/lib/nut.peet")
            ]
        );
        // With no folder there is nothing to be relative to.
        assert_eq!(model.for_file(None), model);
        let mut nowhere = written;
        nowhere.from_file(None);
        assert_eq!(paths(&nowhere)[0], "parts/pin.peet");
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
