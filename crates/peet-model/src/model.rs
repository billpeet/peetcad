//! The parametric model: an ordered list of features (the history) plus the parameter
//! table. This is the source of truth that is saved to disk; everything else (bodies,
//! meshes, feature states) is derived from it by [`crate::Engine`].

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use peet_sketch::Sketch;
use peet_sketch::expr::Parameters;
use serde::{Deserialize, Serialize};

use crate::extrude::{Extrude, Operation};
use crate::feature::{
    ExtrudeFeature, Feature, FeatureId, FeatureKind, PlaneRef, SketchFeature, StdPlane,
};
use crate::material::Material;
use crate::naming::{FaceName, FaceRole};

/// The built-in reference geometry every part has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Datum {
    Origin,
    Plane(StdPlane),
}

impl Datum {
    pub const ALL: [Self; 4] = [
        Self::Origin,
        Self::Plane(StdPlane::Front),
        Self::Plane(StdPlane::Top),
        Self::Plane(StdPlane::Right),
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Origin => "Origin",
            Self::Plane(p) => p.label(),
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|d| *d == self).unwrap_or(0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    /// The history, in build order. Shared pointers make snapshots (for undo) cheap: a
    /// snapshot copies only the features that change afterwards.
    features: Vec<Arc<Feature>>,
    /// Named values usable in every expression, and the document's units.
    pub parameters: Parameters,
    /// How many features are built: the rollback bar sits below this many. `None` means
    /// all of them.
    rollback: Option<usize>,
    /// Whether the origin and the three standard planes are shown.
    datums_visible: [bool; 4],
    next_id: u32,
    /// The next number for automatic names, per name prefix.
    name_counters: BTreeMap<String, u32>,
    /// What the part is made of: its name in a bill of materials and the density its
    /// mass is worked out with. `None` until one is chosen.
    #[serde(default)]
    pub material: Option<Material>,
    /// The colour the part is drawn in (sRGB). `None` is the application's colour for
    /// parts.
    #[serde(default)]
    pub color: Option<[u8; 3]>,
}

/// The model as files up to model schema 4 have it: before a part had a material and a
/// colour. The file format is not self-describing, so an older model is read as this
/// and converted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelV4 {
    name: String,
    features: Vec<Arc<Feature>>,
    parameters: Parameters,
    rollback: Option<usize>,
    datums_visible: [bool; 4],
    next_id: u32,
    name_counters: BTreeMap<String, u32>,
}

impl From<ModelV4> for Model {
    fn from(m: ModelV4) -> Self {
        Self {
            name: m.name,
            features: m.features,
            parameters: m.parameters,
            rollback: m.rollback,
            datums_visible: m.datums_visible,
            next_id: m.next_id,
            name_counters: m.name_counters,
            material: None,
            color: None,
        }
    }
}

impl ModelV4 {
    /// `model` as it would have been saved before: without its material and colour.
    pub fn of(model: &Model) -> Self {
        Self {
            name: model.name.clone(),
            features: model.features.clone(),
            parameters: model.parameters.clone(),
            rollback: model.rollback,
            datums_visible: model.datums_visible,
            next_id: model.next_id,
            name_counters: model.name_counters.clone(),
        }
    }
}

impl Default for Model {
    fn default() -> Self {
        Self {
            name: "Part1".to_owned(),
            features: Vec::new(),
            parameters: Parameters::default(),
            rollback: None,
            datums_visible: [true; 4],
            next_id: 1,
            name_counters: BTreeMap::new(),
            material: None,
            color: None,
        }
    }
}

impl Model {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- Reading ----

    pub fn features(&self) -> impl DoubleEndedIterator<Item = &Feature> + ExactSizeIterator {
        self.features.iter().map(|f| &**f)
    }

    pub fn len(&self) -> usize {
        self.features.len()
    }

    pub fn is_empty(&self) -> bool {
        self.features.is_empty()
    }

    pub fn index_of(&self, id: FeatureId) -> Option<usize> {
        self.features.iter().position(|f| f.id == id)
    }

    pub fn feature(&self, id: FeatureId) -> Option<&Feature> {
        self.features.iter().find(|f| f.id == id).map(|f| &**f)
    }

    pub(crate) fn feature_arc(&self, index: usize) -> Arc<Feature> {
        self.features[index].clone()
    }

    /// The feature's name, or a placeholder if it no longer exists.
    pub fn name_of(&self, id: FeatureId) -> &str {
        self.feature(id).map_or("a deleted feature", |f| &f.name)
    }

    pub fn sketch(&self, id: FeatureId) -> Option<&SketchFeature> {
        self.feature(id)?.sketch()
    }

    pub fn datum_visible(&self, datum: Datum) -> bool {
        self.datums_visible[datum.index()]
    }

    pub fn set_datum_visible(&mut self, datum: Datum, visible: bool) {
        self.datums_visible[datum.index()] = visible;
    }

    /// How many features are built (those above the rollback bar).
    pub fn rollback_index(&self) -> usize {
        self.rollback
            .map_or(self.features.len(), |r| r.min(self.features.len()))
    }

    pub fn is_rolled_back(&self) -> bool {
        self.rollback_index() < self.features.len()
    }

    /// Words for a face name, for messages: "the end face of Extrude1".
    pub fn describe_face(&self, name: &FaceName) -> String {
        let parts: Vec<String> = name
            .origins()
            .iter()
            .map(|o| {
                let role = match o.role {
                    FaceRole::NearCap => "start face",
                    FaceRole::FarCap => "end face",
                    FaceRole::Side(_) => "side face",
                    FaceRole::SheetTop(_) => "top face",
                    FaceRole::SheetBottom(_) => "bottom face",
                    FaceRole::BendTop(_) => "bend's top face",
                    FaceRole::BendBottom(_) => "bend's bottom face",
                    FaceRole::Wall(_, i) => match i {
                        peet_sheetmetal::layout::wall::TIP => "end face",
                        _ => "edge face",
                    },
                    FaceRole::Instance(_) => "copy",
                    FaceRole::Blend(_) => "blend face",
                    FaceRole::BlendEnd(_) => "blend's end face",
                    FaceRole::BlendCorner(_) => "rounded corner",
                    FaceRole::Inner => "inside",
                    FaceRole::Imported(_) => "face",
                };
                format!("the {role} of {}", self.name_of(o.feature))
            })
            .collect();
        parts.join(" joined with ")
    }

    // ---- Editing ----

    /// The feature, for changing it. Other snapshots of the model are not affected.
    pub fn feature_mut(&mut self, id: FeatureId) -> Option<&mut Feature> {
        self.features
            .iter_mut()
            .find(|f| f.id == id)
            .map(Arc::make_mut)
    }

    fn next_name(&mut self, prefix: &str) -> String {
        let n = self.name_counters.entry(prefix.to_owned()).or_insert(1);
        let name = format!("{prefix}{n}");
        *n += 1;
        name
    }

    /// Adds a feature with an automatic name, at the rollback bar (the end of the built
    /// part of the tree), and returns its id.
    pub fn add(&mut self, kind: FeatureKind) -> FeatureId {
        let id = FeatureId(self.next_id);
        self.next_id += 1;
        let name = self.next_name(kind.name_prefix());
        let at = self.rollback_index();
        self.features.insert(
            at,
            Arc::new(Feature {
                id,
                name,
                suppressed: false,
                visible: true,
                kind,
            }),
        );
        if let Some(r) = &mut self.rollback {
            *r = at + 1;
        }
        id
    }

    /// Adds an empty sketch on `plane`.
    pub fn add_sketch(&mut self, plane: PlaneRef, placement: peet_math::Plane) -> FeatureId {
        self.add(FeatureKind::Sketch(Box::new(SketchFeature {
            plane,
            placement,
            sketch: Sketch::new(),
        })))
    }

    /// Adds an extrusion of `sketch`. A sketch used by a feature is hidden, as in other
    /// CAD tools.
    pub fn add_extrude(&mut self, sketch: FeatureId, operation: Operation) -> FeatureId {
        let id = self.add(FeatureKind::Extrude(Box::new(ExtrudeFeature {
            sketch,
            params: Extrude::new(operation),
        })));
        if let Some(s) = self.feature_mut(sketch) {
            s.visible = false;
        }
        id
    }

    /// Adds a revolve of `sketch` about its centreline (its first construction line), or
    /// about its vertical axis if it has none. The sketch is then hidden.
    pub fn add_revolve(&mut self, sketch: FeatureId, operation: Operation) -> FeatureId {
        let def = match self.sketch(sketch) {
            Some(s) => crate::RevolveFeature::new(sketch, &s.sketch, operation),
            None => crate::RevolveFeature::new(sketch, &Sketch::new(), operation),
        };
        let id = self.add(FeatureKind::Revolve(Box::new(def)));
        self.hide(sketch);
        id
    }

    /// Adds a fillet or a chamfer on `edges` (or waiting for edges to be picked).
    pub fn add_blend(
        &mut self,
        kind: crate::BlendKind,
        edges: Vec<crate::naming::EdgeRef>,
    ) -> FeatureId {
        self.add(FeatureKind::Blend(Box::new(crate::BlendFeature::new(
            kind, edges,
        ))))
    }

    /// Adds a shell that opens `faces` (a closed hollow if there are none).
    pub fn add_shell(&mut self, faces: Vec<crate::naming::FaceRef>) -> FeatureId {
        self.add(FeatureKind::Shell(Box::new(crate::ShellFeature::new(
            faces,
        ))))
    }

    /// Adds a draft of `faces` about `neutral`.
    pub fn add_draft(
        &mut self,
        faces: Vec<crate::naming::FaceRef>,
        neutral: Option<PlaneRef>,
    ) -> FeatureId {
        self.add(FeatureKind::Draft(Box::new(crate::DraftFeature::new(
            faces, neutral,
        ))))
    }

    /// Adds holes at the points of `sketch`, which is then hidden.
    pub fn add_hole(&mut self, sketch: FeatureId) -> FeatureId {
        let id = self.add(FeatureKind::Hole(Box::new(crate::HoleFeature::new(sketch))));
        self.hide(sketch);
        id
    }

    /// Adds a sweep of `profile` along `path` (or waiting for the path to be picked). Both
    /// sketches are then hidden.
    pub fn add_sweep(
        &mut self,
        profile: FeatureId,
        path: Option<FeatureId>,
        operation: Operation,
    ) -> FeatureId {
        let id = self.add(FeatureKind::Sweep(Box::new(crate::SweepFeature::new(
            profile, path, operation,
        ))));
        self.hide(profile);
        if let Some(path) = path {
            self.hide(path);
        }
        id
    }

    /// Adds a loft through the profiles of `sections` (sketches, in order), which are
    /// then hidden.
    pub fn add_loft(&mut self, sections: Vec<FeatureId>, operation: Operation) -> FeatureId {
        let id = self.add(FeatureKind::Loft(Box::new(crate::LoftFeature::new(
            sections.clone(),
            operation,
        ))));
        for s in sections {
            self.hide(s);
        }
        id
    }

    /// Adds bodies imported from a file.
    pub fn add_import(&mut self, source: String, solids: Vec<crate::ImportedSolid>) -> FeatureId {
        self.add(FeatureKind::Import(Box::new(crate::ImportFeature {
            source,
            solids,
        })))
    }

    /// Adds a conversion to sheet metal of the body `face` is on, with that flat face
    /// fixed (or of the only body, with its largest flat face fixed).
    pub fn add_convert_to_sheet(&mut self, face: Option<crate::naming::FaceRef>) -> FeatureId {
        self.add(FeatureKind::ConvertToSheet(Box::new(
            crate::ConvertToSheetFeature::new(face),
        )))
    }

    /// Adds a base flange (a new sheet metal body) from `sketch`, which is then hidden.
    pub fn add_base_flange(&mut self, sketch: FeatureId) -> FeatureId {
        let id = self.add(FeatureKind::BaseFlange(Box::new(
            crate::feature::BaseFlangeFeature::new(sketch),
        )));
        self.hide(sketch);
        id
    }

    /// Adds an edge flange on `edge` (or waiting for an edge to be picked).
    pub fn add_edge_flange(&mut self, edge: Option<crate::naming::EdgeRef>) -> FeatureId {
        self.add(FeatureKind::EdgeFlange(Box::new(
            crate::feature::EdgeFlangeFeature::new(edge),
        )))
    }

    /// Adds a sheet metal cut from `sketch` (drawn on a face of the sheet), which is then
    /// hidden.
    pub fn add_sheet_cut(&mut self, sketch: FeatureId) -> FeatureId {
        let id = self.add(FeatureKind::SheetCut(Box::new(
            crate::feature::SheetCutFeature { sketch },
        )));
        self.hide(sketch);
        id
    }

    /// Adds a hem on `edge` (or waiting for an edge to be picked).
    pub fn add_hem(&mut self, edge: Option<crate::naming::EdgeRef>) -> FeatureId {
        self.add(FeatureKind::Hem(Box::new(crate::feature::HemFeature::new(
            edge,
        ))))
    }

    /// Adds a sketched bend along the lines of `sketch`, which is then hidden.
    pub fn add_sketched_bend(&mut self, sketch: FeatureId) -> FeatureId {
        let id = self.add(FeatureKind::SketchedBend(Box::new(
            crate::feature::SketchedBendFeature::new(sketch),
        )));
        self.hide(sketch);
        id
    }

    /// Adds a jog along the line of `sketch`, which is then hidden.
    pub fn add_jog(&mut self, sketch: FeatureId) -> FeatureId {
        let id = self.add(FeatureKind::Jog(Box::new(crate::feature::JogFeature::new(
            sketch,
        ))));
        self.hide(sketch);
        id
    }

    /// Adds a mitre flange with the profile in `sketch` along `edges`.
    pub fn add_miter_flange(
        &mut self,
        sketch: FeatureId,
        edges: Vec<crate::naming::EdgeRef>,
    ) -> FeatureId {
        let id = self.add(FeatureKind::MiterFlange(Box::new(
            crate::feature::MiterFlangeFeature::new(sketch, edges),
        )));
        self.hide(sketch);
        id
    }

    /// Adds a corner treatment for the corners at `faces` (every corner if empty).
    pub fn add_corner(&mut self, faces: Vec<crate::naming::FaceRef>) -> FeatureId {
        self.add(FeatureKind::Corner(Box::new(
            crate::feature::CornerFeature::new(faces),
        )))
    }

    /// Adds forms of `kind` from the shapes in `sketch`, which is then hidden.
    pub fn add_form(&mut self, sketch: FeatureId, kind: peet_sheetmetal::FormKind) -> FeatureId {
        let id = self.add(FeatureKind::Form(Box::new(
            crate::feature::FormFeature::new(sketch, kind),
        )));
        self.hide(sketch);
        id
    }

    /// Adds a pattern of `seeds`.
    pub fn add_pattern(
        &mut self,
        seeds: Vec<FeatureId>,
        def: crate::feature::PatternDef,
    ) -> FeatureId {
        self.add(FeatureKind::Pattern(Box::new(
            crate::feature::PatternFeature { seeds, def },
        )))
    }

    /// Adds mirror images of `seeds` across `plane`.
    pub fn add_mirror(&mut self, seeds: Vec<FeatureId>, plane: PlaneRef) -> FeatureId {
        self.add(FeatureKind::Mirror(Box::new(
            crate::feature::MirrorFeature { seeds, plane },
        )))
    }

    fn hide(&mut self, id: FeatureId) {
        if let Some(f) = self.feature_mut(id) {
            f.visible = false;
        }
    }

    /// Removes a feature. Features that refer to it stay, and fail to rebuild with a
    /// message saying what they lost.
    pub fn remove(&mut self, id: FeatureId) -> Option<Feature> {
        let index = self.index_of(id)?;
        let removed = self.features.remove(index);
        if let Some(r) = &mut self.rollback
            && index < *r
        {
            *r -= 1;
        }
        Some(Arc::unwrap_or_clone(removed))
    }

    /// Puts the rollback bar below `built` features (`None`: at the end).
    pub fn set_rollback(&mut self, built: Option<usize>) {
        self.rollback = built.filter(|&b| b < self.features.len());
    }

    /// Moves a feature to `index` in the tree. Fails, with the reason, if that would put
    /// it before something it depends on or after something that depends on it.
    pub fn move_to(&mut self, id: FeatureId, index: usize) -> Result<(), String> {
        let from = self
            .index_of(id)
            .ok_or_else(|| "The feature no longer exists.".to_owned())?;
        let graph = DependencyGraph::new(self);
        let (lo, hi) = graph.allowed_positions(id);
        let index = index.min(self.features.len() - 1);
        if index < lo {
            let blocker = graph
                .dependencies(id)
                .iter()
                .copied()
                .filter(|d| self.index_of(*d).is_some_and(|i| i >= index))
                .min_by_key(|d| self.index_of(*d));
            return Err(format!(
                "{} can't go there: it uses {}, which must come first.",
                self.name_of(id),
                blocker.map_or("a feature", |b| self.name_of(b))
            ));
        }
        if index > hi {
            let blocker = graph
                .dependents(id)
                .iter()
                .copied()
                .filter(|d| self.index_of(*d).is_some_and(|i| i <= index))
                .max_by_key(|d| self.index_of(*d));
            return Err(format!(
                "{} can't go there: {} uses it and must come after it.",
                self.name_of(id),
                blocker.map_or("a feature", |b| self.name_of(b))
            ));
        }
        let feature = self.features.remove(from);
        self.features.insert(index, feature);
        Ok(())
    }

    /// Restores invariants after loading a model from a file: ids are unique and
    /// `next_id` is beyond all of them. Returns an error for a model that can't be used.
    pub fn validate(&mut self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for f in &self.features {
            if !seen.insert(f.id) {
                return Err(format!(
                    "The model is damaged: two features have the id {}.",
                    f.id.0
                ));
            }
        }
        let max = self.features.iter().map(|f| f.id.0).max().unwrap_or(0);
        self.next_id = self.next_id.max(max.saturating_add(1));
        self.rollback = self.rollback.filter(|&b| b < self.features.len());
        self.parameters.evaluate();
        Ok(())
    }
}

/// Which features refer to which: the explicit dependency graph of the history.
///
/// An edge `a → b` means feature `a` uses feature `b` directly: an extrusion uses its
/// sketch, a sketch uses the plane or the faces it lies on (and so the features that made
/// those faces), a reference plane uses what it is measured from.
///
/// Solid features also depend on the bodies as they are when their turn comes, which is
/// the result of every solid feature above them. That order dependency is not an edge
/// here (two extrusions may be swapped, with a different result); it is accounted for in
/// [`DependencyGraph::downstream`] and by the rebuild keys of [`crate::Engine`].
#[derive(Clone, Debug)]
pub struct DependencyGraph {
    order: Vec<FeatureId>,
    solid: Vec<bool>,
    dependencies: HashMap<FeatureId, Vec<FeatureId>>,
    dependents: HashMap<FeatureId, Vec<FeatureId>>,
}

impl DependencyGraph {
    pub fn new(model: &Model) -> Self {
        let order: Vec<FeatureId> = model.features().map(|f| f.id).collect();
        let solid = model.features().map(|f| f.kind.is_solid()).collect();
        let mut dependencies = HashMap::new();
        let mut dependents: HashMap<FeatureId, Vec<FeatureId>> = HashMap::new();
        for f in model.features() {
            let deps = f.kind.dependencies();
            for d in &deps {
                dependents.entry(*d).or_default().push(f.id);
            }
            dependencies.insert(f.id, deps);
        }
        Self {
            order,
            solid,
            dependencies,
            dependents,
        }
    }

    /// The features `id` refers to directly (including ones that no longer exist).
    pub fn dependencies(&self, id: FeatureId) -> &[FeatureId] {
        self.dependencies.get(&id).map_or(&[], Vec::as_slice)
    }

    /// The features that refer to `id` directly, in tree order.
    pub fn dependents(&self, id: FeatureId) -> &[FeatureId] {
        self.dependents.get(&id).map_or(&[], Vec::as_slice)
    }

    fn index(&self, id: FeatureId) -> Option<usize> {
        self.order.iter().position(|f| *f == id)
    }

    /// Every feature a change to `id` can affect, in tree order: those that refer to it
    /// directly or through others, and, once a solid feature is affected, every solid
    /// feature below it (they build on its bodies) and what refers to those.
    pub fn downstream(&self, id: FeatureId) -> Vec<FeatureId> {
        let Some(start) = self.index(id) else {
            return Vec::new();
        };
        let mut affected = vec![false; self.order.len()];
        affected[start] = true;
        let mut bodies_changed = self.solid[start];
        for i in start + 1..self.order.len() {
            let uses_affected = self
                .dependencies(self.order[i])
                .iter()
                .any(|d| self.index(*d).is_some_and(|j| affected[j]));
            if uses_affected || (self.solid[i] && bodies_changed) {
                affected[i] = true;
                bodies_changed |= self.solid[i];
            }
        }
        (start + 1..self.order.len())
            .filter(|&i| affected[i])
            .map(|i| self.order[i])
            .collect()
    }

    /// The first and last position `id` may be moved to without breaking a reference.
    pub fn allowed_positions(&self, id: FeatureId) -> (usize, usize) {
        let last = self.order.len().saturating_sub(1);
        let lo = self
            .dependencies(id)
            .iter()
            .filter_map(|d| self.index(*d))
            .max()
            .map_or(0, |i| i + 1);
        let hi = self
            .dependents(id)
            .iter()
            .filter_map(|d| self.index(*d))
            .min()
            .map_or(last, |i| i.saturating_sub(1));
        // After the move the feature itself is gone from its old slot, which shifts the
        // positions between the old and the new one by one.
        let from = self.index(id).unwrap_or(0);
        let lo = if from < lo { lo - 1 } else { lo };
        let hi = if from > hi { hi + 1 } else { hi };
        (lo.min(last), hi.min(last))
    }

    /// Features that refer to a feature placed after them, or to one that doesn't exist:
    /// `(feature, what it refers to)`.
    pub fn broken(&self) -> Vec<(FeatureId, FeatureId)> {
        let mut out = Vec::new();
        for (i, &id) in self.order.iter().enumerate() {
            for &d in self.dependencies(id) {
                if self.index(d).is_none_or(|j| j >= i) {
                    out.push((id, d));
                }
            }
        }
        out
    }
}
