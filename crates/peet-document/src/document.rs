//! The open document: the parametric model, its rebuild state, the undo history and the
//! bodies derived from them.
//!
//! A document is a part or an assembly ([`Document::is_assembly`]). An assembly's bodies
//! are those of its components: [`Document::bodies`] has one entry per placed body, with
//! where it is in [`Document::placed`], and the instances of one part share one view (and
//! so one tessellation).
//!
//! Every change to the model goes through [`Document::change`], which records an undo
//! step and rebuilds. Rebuilds are incremental (see `peet_model::Engine`), and body
//! views are kept per body stamp, so only bodies whose geometry changed are tessellated
//! again (and only if something asks for their triangles).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use peet_math::{Aabb, DVec3, Frame, Plane};
use peet_model::naming::{find_edge, find_face, find_vertex};
use peet_model::{
    CompId, Datum, DefId, EdgeRef, Engine, Evaluation, FaceRef, Feature, FeatureId, FeatureKind,
    History, Model, Output, Status, VertexRef,
};
use peet_sketch::Sketch;

use crate::body::{BodyView, GeomRef};
use crate::session::DocId;
pub use peet_model::SketchStatus;

/// Something in the feature tree: the built-in datums, or a feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemId {
    Datum(Datum),
    Feature(FeatureId),
}

impl ItemId {
    pub fn feature(self) -> Option<FeatureId> {
        match self {
            Self::Feature(id) => Some(id),
            Self::Datum(_) => None,
        }
    }
}

/// A selection that survives rebuilds.
#[derive(Clone, Debug, PartialEq)]
pub enum Persistent {
    Face(FaceRef),
    Edge(EdgeRef),
    Vertex(VertexRef),
}

/// Where the document lives on disk (or, on the web, the name it was opened or
/// downloaded as).
#[derive(Clone, Debug, PartialEq)]
pub struct FileLocation {
    pub name: String,
    pub path: Option<PathBuf>,
}

/// Where a shown body is, and whose it is. One per entry of [`Document::bodies`].
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    /// Where the body's coordinates are in the document's: the world for a part's own
    /// bodies, the component's placement in an assembly.
    pub frame: Frame,
    /// In an assembly: the component the body belongs to (and below it, through
    /// sub-assemblies, the component of the part itself).
    pub path: Vec<CompId>,
    /// The colour of the part the body belongs to.
    pub color: Option<[u8; 3]>,
}

impl Placed {
    /// The component of this assembly the body belongs to.
    pub fn component(&self) -> Option<CompId> {
        self.path.first().copied()
    }
}

/// A part that lives in an assembly's file, opened as a document of its own to be
/// edited: saving it stores it back in the assembly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Embedded {
    /// The assembly's document in the session.
    pub assembly: DocId,
    /// Which of the assembly's parts this is.
    pub definition: DefId,
}

/// The eight corners of a box, moved, as a box.
fn placed_bounds(bounds: &Aabb, frame: &Frame) -> Aabb {
    if *frame == Frame::WORLD || bounds.min.cmpgt(bounds.max).any() {
        return *bounds;
    }
    let mut out = Aabb::EMPTY;
    for i in 0..8 {
        let pick = |bit: usize, lo: f64, hi: f64| if i >> bit & 1 == 0 { lo } else { hi };
        out.extend(frame.to_world(DVec3::new(
            pick(0, bounds.min.x, bounds.max.x),
            pick(1, bounds.min.y, bounds.max.y),
            pick(2, bounds.min.z, bounds.max.z),
        )));
    }
    out
}

pub struct Document {
    pub model: Model,
    engine: Engine,
    history: History<Model>,
    /// The bodies of the last rebuild, ready to draw. In an assembly: one per body of
    /// each component, the instances of a part sharing one view.
    pub bodies: Vec<Arc<BodyView>>,
    /// Where each of `bodies` is.
    pub placed: Vec<Placed>,
    /// Set for a part of an assembly that is open for editing.
    pub embedded: Option<Embedded>,
    /// Changes on every rebuild, so the viewport knows to refresh. Unique across all
    /// documents of the process (see [`next_revision`]), so a newly opened document can
    /// never be mistaken for the one it replaced.
    pub revision: u64,
    pub file: Option<FileLocation>,
    /// Hash of the model as last saved: the document is modified when they differ.
    saved_hash: u64,
    current_hash: u64,
    /// Set when the shown bodies come from a file's cache and the model still has to be
    /// rebuilt.
    pending_rebuild: bool,
    /// Sheet metal bodies are shown as flat patterns.
    flat: bool,
}

impl Default for Document {
    fn default() -> Self {
        Self::from_model(Model::new(), None)
    }
}

impl Document {
    /// A document for `model`, rebuilt.
    pub fn from_model(model: Model, file: Option<FileLocation>) -> Self {
        let hash = peet_model::hash::of(&model);
        let mut doc = Self {
            model,
            engine: Engine::new(),
            history: History::default(),
            bodies: Vec::new(),
            placed: Vec::new(),
            embedded: None,
            revision: next_revision(),
            file,
            saved_hash: hash,
            current_hash: hash,
            pending_rebuild: false,
            flat: false,
        };
        doc.rebuild();
        doc
    }

    /// A document for a part read from a file. If the file has caches, its bodies are
    /// shown at once and the model is rebuilt on the next frame (see
    /// [`Document::finish_loading`]); cached meshes save tessellating again.
    pub fn from_opened(opened: peet_io::document::Opened, file: Option<FileLocation>) -> Self {
        let hash = peet_model::hash::of(&opened.model);
        let mut meshes: HashMap<u64, peet_kernel::tessellate::SolidMesh> = opened
            .meshes
            .into_iter()
            .map(|m| (m.stamp, m.mesh))
            .collect();
        let mut doc = Self {
            model: opened.model,
            engine: Engine::new(),
            history: History::default(),
            bodies: Vec::new(),
            placed: Vec::new(),
            embedded: None,
            revision: next_revision(),
            file,
            saved_hash: hash,
            current_hash: hash,
            pending_rebuild: true,
            flat: false,
        };
        // The caches are of a part's own bodies: an assembly is rebuilt at once.
        match opened.bodies.filter(|_| !doc.model.is_assembly()) {
            Some(bodies) => {
                let color = doc.model.color;
                doc.placed = bodies
                    .iter()
                    .map(|_| Placed {
                        frame: Frame::WORLD,
                        path: Vec::new(),
                        color,
                    })
                    .collect();
                doc.bodies = bodies
                    .into_iter()
                    .map(|b| {
                        let cached = meshes.remove(&b.stamp);
                        Arc::new(BodyView::new(b, cached))
                    })
                    .collect();
                doc.revision = next_revision();
            }
            None => doc.rebuild(),
        }
        doc
    }

    /// Rebuilds a model whose bodies came from a file's cache. Returns whether it did.
    pub fn finish_loading(&mut self) -> bool {
        if self.pending_rebuild {
            self.rebuild();
            true
        } else {
            false
        }
    }

    /// The document's name for titles and file names.
    pub fn title(&self) -> String {
        match &self.file {
            Some(f) => f.name.clone(),
            None => self.model.name.clone(),
        }
    }

    pub fn is_modified(&self) -> bool {
        self.current_hash != self.saved_hash
    }

    /// Marks the document as never saved (recovered work).
    pub fn mark_unsaved(&mut self) {
        self.saved_hash = self.current_hash.wrapping_add(1);
    }

    /// Notes that the document was saved (to `file`).
    pub fn mark_saved(&mut self, file: Option<FileLocation>) {
        if file.is_some() {
            self.file = file;
        }
        self.saved_hash = self.current_hash;
    }

    /// Rebuilds the model and refreshes the bodies (tessellating only changed ones).
    pub fn rebuild(&mut self) {
        self.pending_rebuild = false;
        self.engine.regenerate(&mut self.model);
        self.refresh_views();
        // Rebuilds write solved sketches back, which can change the hash.
        self.current_hash = peet_model::hash::of(&self.model);
    }

    /// Makes the displayed bodies match the last rebuild and the view (folded or flat),
    /// reusing the tessellation of every body that didn't change.
    fn refresh_views(&mut self) {
        // Views are kept by stamp: from the last rebuild, and between the instances of
        // one part in this one.
        let mut views: HashMap<u64, Arc<BodyView>> =
            self.bodies.drain(..).map(|b| (b.stamp, b)).collect();
        let flat = self.flat;
        let mut view = |b: &Arc<peet_model::Body>| {
            views
                .entry(BodyView::key(b, flat))
                .or_insert_with(|| Arc::new(BodyView::of(b.clone(), flat)))
                .clone()
        };
        let built = self.engine.evaluation();
        let color = self.model.color;
        self.placed = built
            .bodies
            .iter()
            .map(|_| Placed {
                frame: Frame::WORLD,
                path: Vec::new(),
                color,
            })
            .chain(built.instances.iter().map(|i| Placed {
                frame: i.frame,
                path: i.path.clone(),
                color: i.color,
            }))
            .collect();
        self.bodies = built
            .bodies
            .iter()
            .chain(built.instances.iter().map(|i| &i.body))
            .map(&mut view)
            .collect();
        self.revision = next_revision();
    }

    /// Whether the document is an assembly, and not a part.
    pub fn is_assembly(&self) -> bool {
        self.model.is_assembly()
    }

    /// The shown bodies of a component, as indices into [`Document::bodies`].
    pub fn bodies_of(&self, component: CompId) -> impl Iterator<Item = usize> + '_ {
        self.placed
            .iter()
            .enumerate()
            .filter(move |(_, p)| p.component() == Some(component))
            .map(|(i, _)| i)
    }

    /// The bounds of a component where it is, or of nothing if it shows no bodies.
    pub fn component_bounds(&self, component: CompId) -> Aabb {
        self.bodies_of(component)
            .map(|i| placed_bounds(&self.bodies[i].solid.bounds(), &self.placed[i].frame))
            .fold(Aabb::EMPTY, |a, b| a.union(&b))
    }

    /// Whether sheet metal bodies are shown flat.
    pub fn is_flat(&self) -> bool {
        self.flat
    }

    /// Shows sheet metal bodies flat (their flat patterns) or folded.
    pub fn set_flat(&mut self, flat: bool) {
        if self.flat != flat {
            self.flat = flat;
            if !self.pending_rebuild {
                self.refresh_views();
            }
        }
    }

    /// Whether any body is sheet metal.
    pub fn has_sheet_metal(&self) -> bool {
        self.evaluation().bodies.iter().any(|b| b.sheet.is_some())
    }

    /// The sheet metal body to export or report on: the one `preferred` points to if it is
    /// sheet metal, else the first one.
    pub fn sheet_body(&self, preferred: Option<usize>) -> Option<&Arc<peet_model::Body>> {
        let bodies = &self.evaluation().bodies;
        preferred
            .and_then(|i| bodies.get(i))
            .filter(|b| b.sheet.is_some())
            .or_else(|| bodies.iter().find(|b| b.sheet.is_some()))
    }

    pub fn evaluation(&self) -> &Evaluation {
        self.engine.evaluation()
    }

    /// Applies a change as one undo step and rebuilds. Returns whether anything changed.
    pub fn change(&mut self, label: &str, f: impl FnOnce(&mut Model)) -> bool {
        self.change_inner(label, None, f)
    }

    /// Like [`Document::change`], but consecutive changes with the same `key` make one
    /// undo step (a drag).
    pub fn change_merging(&mut self, label: &str, key: u64, f: impl FnOnce(&mut Model)) -> bool {
        self.change_inner(label, Some(key), f)
    }

    fn change_inner(&mut self, label: &str, key: Option<u64>, f: impl FnOnce(&mut Model)) -> bool {
        let before = self.model.clone();
        f(&mut self.model);
        if self.model == before {
            return false;
        }
        match key {
            Some(k) => self.history.record_merging(label, k, before),
            None => self.history.record(label, before),
        }
        self.rebuild();
        true
    }

    /// Pulls a component of an assembly: the point `point` of it (in the component's
    /// coordinates) towards `to` (in the assembly's), as far as its mates let it go,
    /// taking what it is mated to along (see [`peet_model::Drag`]). Returns whether
    /// anything moved. It is an undo step called `label`; with a `key`, consecutive
    /// drags with the same key are one step.
    pub fn drag_component(
        &mut self,
        label: &str,
        key: Option<u64>,
        drag: peet_model::Drag,
    ) -> bool {
        let before = self.model.clone();
        self.engine.set_drag(Some(drag));
        self.rebuild();
        if self.model == before {
            return false;
        }
        match key {
            Some(k) => self.history.record_merging(label, k, before),
            None => self.history.record(label, before),
        }
        true
    }

    /// Ends a run of merged changes (a drag), so the next change is a new undo step.
    pub fn seal_history(&mut self) {
        self.history.seal();
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.history.undo_label()
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.history.redo_label()
    }

    /// Undoes the last change and rebuilds. Returns what was undone.
    pub fn undo(&mut self) -> Option<String> {
        let label = self.history.undo(&mut self.model)?;
        self.rebuild();
        Some(label)
    }

    pub fn redo(&mut self) -> Option<String> {
        let label = self.history.redo(&mut self.model)?;
        self.rebuild();
        Some(label)
    }

    /// The bytes of the document as a `.peet` file.
    pub fn save_bytes(&self, with_caches: bool) -> Result<Vec<u8>, String> {
        let metadata = peet_io::document::Metadata {
            saved_at: web_time::SystemTime::now()
                .duration_since(web_time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs()),
            ..Default::default()
        };
        // Caches only describe a fully built model, and a part's own bodies: an assembly
        // is saved without (its parts are rebuilt when it is opened).
        let cached = with_caches && !self.pending_rebuild && !self.is_assembly();
        let caches = cached.then(|| peet_io::document::Caches {
            bodies: &self.evaluation().bodies,
            meshes: self
                .bodies
                .iter()
                .map(|b| peet_io::document::CachedMesh {
                    stamp: b.stamp,
                    mesh: b.tess().clone(),
                })
                .collect(),
        });
        peet_io::document::save(&self.model, &metadata, caches).map_err(|e| e.message)
    }

    // ---- Queries ----

    pub fn feature(&self, id: FeatureId) -> Option<&Feature> {
        self.model.feature(id)
    }

    pub fn status(&self, id: FeatureId) -> Option<&Status> {
        self.evaluation().status(id)
    }

    pub fn item_name(&self, item: ItemId) -> String {
        match item {
            ItemId::Datum(d) => d.label().to_owned(),
            ItemId::Feature(id) => self.model.name_of(id).to_owned(),
        }
    }

    /// The plane a sketch is drawn on (its last known one if it can't be resolved now),
    /// and how well defined it is.
    pub fn sketch_placement(&self, id: FeatureId) -> Option<(Plane, SketchStatus)> {
        match self.evaluation().output(id) {
            Output::Sketch { plane, definition } => Some((plane, definition)),
            _ => self
                .model
                .sketch(id)
                .map(|s| (s.placement, SketchStatus::Under)),
        }
    }

    /// Words for what a sketch lies on.
    pub fn plane_name(&self, plane: &peet_model::PlaneRef) -> String {
        match plane {
            peet_model::PlaneRef::Standard(p) => p.label().to_owned(),
            peet_model::PlaneRef::Feature(id) => self.model.name_of(*id).to_owned(),
            peet_model::PlaneRef::Face(f) => {
                let d = self.model.describe_face(&f.name);
                let mut c = d.chars();
                c.next()
                    .map(|first| first.to_uppercase().chain(c).collect())
                    .unwrap_or(d)
            }
        }
    }

    /// The flat face, standard plane or reference plane a click picked, as something a
    /// sketch or feature can refer to.
    pub fn plane_ref_of_item(&self, item: ItemId) -> Option<peet_model::PlaneRef> {
        match item {
            ItemId::Datum(Datum::Plane(p)) => Some(peet_model::PlaneRef::Standard(p)),
            ItemId::Feature(id) => match self.evaluation().output(id) {
                Output::Plane(_) | Output::Frame(_) => Some(peet_model::PlaneRef::Feature(id)),
                _ => None,
            },
            ItemId::Datum(Datum::Origin) => None,
        }
    }

    /// The resolved plane of a reference.
    pub fn resolve_plane(&self, item: ItemId) -> Option<Plane> {
        match item {
            ItemId::Datum(Datum::Plane(p)) => Some(p.plane()),
            ItemId::Feature(id) => match self.evaluation().output(id) {
                Output::Plane(p) => Some(p),
                Output::Frame(frame) => Some(Plane { frame }),
                _ => None,
            },
            ItemId::Datum(Datum::Origin) => None,
        }
    }

    /// Converts a selection into references that survive a rebuild.
    pub fn persist(&self, g: GeomRef) -> Option<Persistent> {
        // In an assembly a face belongs to a component, which these references don't say.
        if self.is_assembly() {
            return None;
        }
        let body = &self.bodies.get(g.body())?.source;
        Some(match g {
            GeomRef::Face { face, .. } => Persistent::Face(body.face_ref(face)),
            GeomRef::Edge { edge, .. } => Persistent::Edge(body.edge_ref(edge)?),
            GeomRef::Vertex { vertex, .. } => Persistent::Vertex(body.vertex_ref(vertex)),
        })
    }

    /// Finds a persisted selection in the current bodies.
    pub fn restore(&self, p: &Persistent) -> Option<GeomRef> {
        let bodies = &self.evaluation().bodies;
        Some(match p {
            Persistent::Face(r) => {
                let f = find_face(bodies, r)?;
                GeomRef::Face {
                    body: f.body,
                    face: f.id,
                }
            }
            Persistent::Edge(r) => {
                let f = find_edge(bodies, r)?;
                GeomRef::Edge {
                    body: f.body,
                    edge: f.id,
                }
            }
            Persistent::Vertex(r) => {
                let f = find_vertex(bodies, r)?;
                GeomRef::Vertex {
                    body: f.body,
                    vertex: f.id,
                }
            }
        })
    }

    /// Bounds of all bodies (reference geometry is excluded, as in other CAD tools'
    /// "zoom to fit").
    pub fn visible_body_bounds(&self) -> Aabb {
        self.bodies
            .iter()
            .zip(&self.placed)
            .map(|(b, p)| placed_bounds(&b.solid.bounds(), &p.frame))
            .fold(Aabb::EMPTY, |a, b| a.union(&b))
    }

    /// Bounds of all bodies and visible sketches: what "zoom to fit" frames.
    pub fn visible_bounds(&self) -> Aabb {
        let mut bounds = self.visible_body_bounds();
        for f in self.model.features().filter(|f| f.visible) {
            if let FeatureKind::Sketch(s) = &f.kind
                && let Some((plane, _)) = self.sketch_placement(f.id)
            {
                bounds = bounds.union(&sketch_bounds(&plane, &s.sketch));
            }
        }
        bounds
    }

    /// Whether all three standard planes are visible.
    pub fn planes_visible(&self) -> bool {
        Datum::ALL
            .iter()
            .filter(|d| matches!(d, Datum::Plane(_)))
            .all(|d| self.model.datum_visible(*d))
    }
}

/// A revision number never used before in this process.
fn next_revision() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Model-space bounds of a sketch's geometry (the origin point alone counts as empty).
pub fn sketch_bounds(plane: &Plane, sketch: &Sketch) -> Aabb {
    let mut bounds = Aabb::EMPTY;
    for (id, e) in sketch.entities() {
        if id == Sketch::ORIGIN {
            continue;
        }
        let (lo, hi) = match sketch.curve(id) {
            Some(c) => c.bounds(),
            None => match e.geometry {
                peet_sketch::Geometry::Point { pos } => (pos, pos),
                _ => continue,
            },
        };
        for corner in [
            lo,
            hi,
            peet_math::DVec2::new(lo.x, hi.y),
            peet_math::DVec2::new(hi.x, lo.y),
        ] {
            bounds.extend(plane.from_plane_coords(corner));
        }
    }
    bounds
}

#[cfg(test)]
mod tests {
    use super::*;
    use peet_math::DVec2;
    use peet_model::{Operation, PlaneRef, StdPlane};

    fn doc_with_extruded_rectangle() -> (Document, FeatureId, FeatureId) {
        let mut doc = Document::default();
        let mut ids = (FeatureId(0), FeatureId(0));
        doc.change("Add", |m| {
            let s = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
            if let Some(f) = m.feature_mut(s).and_then(|f| f.sketch_mut()) {
                peet_sketch::shapes::rectangle(&mut f.sketch, DVec2::ZERO, DVec2::new(40.0, 20.0));
            }
            let e = m.add_extrude(s, Operation::Add);
            ids = (s, e);
        });
        (doc, ids.0, ids.1)
    }

    #[test]
    fn extrude_builds_a_body_and_hides_its_sketch() {
        let (doc, sketch, ex) = doc_with_extruded_rectangle();
        assert_eq!(doc.bodies.len(), 1);
        assert_eq!(doc.status(ex), Some(&Status::Ok));
        assert!(!doc.feature(sketch).unwrap().visible);
        let body = &doc.bodies[0];
        assert!(body.error().is_none());
        assert_eq!(body.tess().faces.len(), body.solid.faces.len());
        assert!(body.tess().faces.iter().all(|f| !f.triangles.is_empty()));
        let size = doc.visible_body_bounds().size();
        assert!((size.x - 40.0).abs() < 1e-9 && (size.z - 10.0).abs() < 1e-9);
    }

    #[test]
    fn changes_are_undoable_and_tracked() {
        let (mut doc, sketch, ex) = doc_with_extruded_rectangle();
        assert!(doc.is_modified());
        doc.mark_saved(None);
        assert!(!doc.is_modified());
        let stamp = doc.bodies[0].stamp;
        doc.change("Delete", |m| {
            m.remove(sketch);
        });
        assert!(doc.is_modified());
        assert!(
            doc.status(ex)
                .unwrap()
                .message()
                .unwrap()
                .contains("sketch")
        );
        assert!(doc.bodies.is_empty());
        assert_eq!(doc.undo().as_deref(), Some("Delete"));
        assert!(!doc.is_modified(), "back to the saved state");
        assert_eq!(doc.bodies[0].stamp, stamp);
        assert_eq!(doc.redo().as_deref(), Some("Delete"));
        // A change that changes nothing is not a step.
        assert!(!doc.change("Nothing", |_| {}));
        assert_eq!(doc.undo_label(), Some("Delete"));
    }

    #[test]
    fn an_assembly_shows_its_components_bodies_where_they_are() {
        let part = Arc::new(peet_model::samples::bracket().0);
        let mut model = Model::new_assembly();
        let (c1, c2) = {
            let a = model.assembly_mut().unwrap();
            let d = a.define(part);
            let far = Frame {
                origin: DVec3::new(1000.0, 0.0, 0.0),
                ..Frame::WORLD
            };
            (
                a.insert(d, Frame::WORLD).unwrap(),
                a.insert(d, far).unwrap(),
            )
        };
        let mut doc = Document::from_model(model, None);
        assert!(doc.is_assembly());
        assert_eq!((doc.bodies.len(), doc.placed.len()), (2, 2));
        // One view for both instances: tessellated once.
        assert!(Arc::ptr_eq(&doc.bodies[0], &doc.bodies[1]));
        assert_eq!(doc.placed[1].component(), Some(c2));
        assert_eq!(doc.bodies_of(c2).collect::<Vec<_>>(), [1]);
        let one = doc.component_bounds(c1).size();
        let all = doc.visible_body_bounds().size();
        assert!((all.x - (one.x + 1000.0)).abs() < 1e-9 && (all.y - one.y).abs() < 1e-9);
        assert!(
            doc.persist(GeomRef::Face {
                body: 0,
                face: peet_kernel::FaceId(0)
            })
            .is_none()
        );

        // Moving a component keeps the view; it is a step that can be undone.
        let view = doc.bodies[0].clone();
        assert!(doc.change("Move", |m| {
            let c = m.assembly_mut().unwrap().component_mut(c2).unwrap();
            c.placement.origin.x = 500.0;
        }));
        assert!(Arc::ptr_eq(&doc.bodies[1], &view));
        assert_eq!(doc.placed[1].frame.origin.x, 500.0);
        assert_eq!(doc.undo().as_deref(), Some("Move"));
        assert_eq!(doc.placed[1].frame.origin.x, 1000.0);

        // Saved and opened again, it is the same assembly.
        let bytes = doc.save_bytes(true).unwrap();
        let mut again = Document::from_opened(peet_io::document::open(&bytes).unwrap(), None);
        again.finish_loading();
        assert_eq!(again.model, doc.model);
        assert_eq!(again.bodies.len(), 2);
        assert!(!again.is_modified());
    }

    #[test]
    fn selections_survive_rebuilds() {
        let (mut doc, _, ex) = doc_with_extruded_rectangle();
        let top = doc.bodies[0]
            .solid
            .face_ids()
            .find(|f| doc.bodies[0].face_center(*f).z > 9.0)
            .unwrap();
        let p = doc.persist(GeomRef::Face { body: 0, face: top }).unwrap();
        doc.change("Deeper", |m| {
            m.feature_mut(ex)
                .unwrap()
                .extrude_mut()
                .unwrap()
                .params
                .depth = peet_model::Scalar::new(25.0);
        });
        let Some(GeomRef::Face { face, .. }) = doc.restore(&p) else {
            panic!()
        };
        assert!((doc.bodies[0].face_center(face).z - 25.0).abs() < 1e-9);
    }

    #[test]
    fn a_new_document_has_a_new_revision() {
        // The viewport re-uploads meshes when the revision changes: a replacement
        // document must never repeat the revision of the one it replaces.
        let a = Document::default();
        let (model, _) = peet_model::samples::bracket();
        let b = Document::from_model(model, None);
        assert_ne!(a.revision, b.revision);
        assert_ne!(Document::default().revision, Document::default().revision);
    }

    #[test]
    fn flat_pattern_view_shares_topology_and_references() {
        let (model, _) = peet_model::samples::enclosure();
        let mut doc = Document::from_model(model, None);
        assert!(doc.has_sheet_metal());
        let folded = doc.bodies[0].clone();
        let picks: Vec<GeomRef> = [0u32, 7, 23, 41]
            .into_iter()
            .map(|f| GeomRef::Face {
                body: 0,
                face: peet_kernel::FaceId(f),
            })
            .chain([3u32, 30].into_iter().map(|e| GeomRef::Edge {
                body: 0,
                edge: peet_kernel::EdgeId(e),
            }))
            .collect();
        let refs: Vec<_> = picks.iter().map(|g| doc.persist(*g)).collect();

        doc.set_flat(true);
        let flat = doc.bodies[0].clone();
        assert!(flat.flat && !folded.flat);
        assert_ne!(flat.stamp, folded.stamp);
        assert_eq!(flat.solid.faces.len(), folded.solid.faces.len());
        let t = 1.5;
        assert!(
            (flat.solid.bounds().size().z - t).abs() < 1e-9,
            "the flat pattern is flat"
        );
        // Picking the same element in the flat view gives the same reference.
        let flat_refs: Vec<_> = picks.iter().map(|g| doc.persist(*g)).collect();
        assert_eq!(refs, flat_refs);
        // A change while flat keeps showing flat.
        doc.change("Thicker", |m| {
            m.parameters.set("thickness", "2mm").unwrap();
        });
        assert!(doc.bodies[0].flat);
        assert!((doc.bodies[0].solid.bounds().size().z - 2.0).abs() < 1e-9);
        doc.set_flat(false);
        assert!(!doc.bodies[0].flat);
        assert_eq!(doc.bodies[0].stamp, doc.evaluation().bodies[0].stamp);
    }

    #[test]
    fn chassis_sample_shows_folded_and_flat() {
        // The Phase 5 sample: mitre flange, hem, patterns, louvers and dimples all
        // tessellate for display, folded and flat, and every face can be referred to.
        let (model, _) = peet_model::samples::chassis();
        let mut doc = Document::from_model(model, None);
        assert!(doc.has_sheet_metal());
        assert_eq!(doc.evaluation().failures().count(), 0);
        let folded = doc.bodies[0].clone();
        assert!(!folded.tess().faces.is_empty());
        let faces = folded.solid.faces.len() as u32;
        for f in (0..faces).step_by(7) {
            let g = GeomRef::Face {
                body: 0,
                face: peet_kernel::FaceId(f),
            };
            let kept = doc.persist(g).expect("every face has a name");
            assert_eq!(doc.restore(&kept), Some(g), "face {f}");
        }
        doc.set_flat(true);
        let flat = doc.bodies[0].clone();
        assert_eq!(flat.solid.faces.len(), folded.solid.faces.len());
        assert_eq!(flat.tess().faces.len(), folded.tess().faces.len());
        // Flat, except for the forms: the louvers stand 3 above the 1.5 sheet.
        assert!((flat.solid.bounds().size().z - 4.5).abs() < 1e-9);
        // An edit rebuilds it all.
        doc.change("Thicker", |m| {
            m.parameters.set("thickness", "2mm").unwrap();
        });
        assert_eq!(doc.evaluation().failures().count(), 0);
    }

    #[test]
    fn editing_and_saving_never_tessellates() {
        // A headless run that changes the model and saves it without caches does no
        // display work; asking for triangles (an export, the viewport) does it once.
        let (model, _) = peet_model::samples::enclosure();
        let mut doc = Document::from_model(model, None);
        doc.change("Thicker", |m| {
            m.parameters.set("thickness", "2mm").unwrap();
        });
        let bytes = doc.save_bytes(false).unwrap();
        assert!(doc.bodies.iter().all(|b| !b.is_tessellated()));
        let reopened = Document::from_opened(peet_io::document::open(&bytes).unwrap(), None);
        assert_eq!(reopened.model, doc.model);
        assert!(!doc.bodies[0].tess().faces.is_empty());
        assert!(doc.bodies[0].is_tessellated());
    }

    #[test]
    fn opening_a_file_shows_cached_bodies_then_rebuilds() {
        let (model, _) = peet_model::samples::bracket();
        let doc = Document::from_model(model, None);
        let bytes = doc.save_bytes(true).unwrap();
        let opened = peet_io::document::open(&bytes).unwrap();
        let mut loaded = Document::from_opened(opened, None);
        assert_eq!(loaded.bodies.len(), 1, "shown from the cache");
        assert!(!loaded.is_modified());
        assert!(loaded.finish_loading());
        assert!(!loaded.finish_loading());
        assert_eq!(loaded.bodies[0].stamp, doc.bodies[0].stamp);
        assert!(!loaded.is_modified());
    }
}
