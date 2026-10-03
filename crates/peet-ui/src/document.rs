//! The open document: the parametric model, its rebuild state, the undo history and the
//! display data derived from them.
//!
//! Every change to the model goes through [`Document::change`], which records an undo
//! step and rebuilds. Rebuilds are incremental (see `peet_model::Engine`), and body
//! tessellations are kept per body stamp, so only bodies whose geometry changed are
//! tessellated again.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use peet_math::{Aabb, Plane};
use peet_model::naming::{find_edge, find_face, find_vertex};
use peet_model::{
    Datum, EdgeRef, Engine, Evaluation, FaceRef, Feature, FeatureId, FeatureKind, History, Model,
    Output, Status, VertexRef,
};
use peet_sketch::Sketch;

use crate::bodies::{BodyView, GeomRef};
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

/// The sketch being edited: a working copy, written back to the model (as one undo step)
/// when editing ends.
#[derive(Clone, Debug)]
pub struct SketchItem {
    pub plane: Plane,
    /// Name of what the sketch lies on, for the properties panel.
    pub plane_name: String,
    pub sketch: Sketch,
    pub status: SketchStatus,
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

pub struct Document {
    pub model: Model,
    engine: Engine,
    history: History<Model>,
    /// The bodies of the last rebuild, ready to draw.
    pub bodies: Vec<Arc<BodyView>>,
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
            revision: next_revision(),
            file,
            saved_hash: hash,
            current_hash: hash,
            pending_rebuild: true,
            flat: false,
        };
        match opened.bodies {
            Some(bodies) => {
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
        let previous: HashMap<u64, Arc<BodyView>> =
            self.bodies.drain(..).map(|b| (b.stamp, b)).collect();
        let flat = self.flat;
        self.bodies = self
            .engine
            .evaluation()
            .bodies
            .iter()
            .map(|b| match previous.get(&BodyView::key(b, flat)) {
                Some(view) => view.clone(),
                None => Arc::new(BodyView::of(b.clone(), flat)),
            })
            .collect();
        self.revision = next_revision();
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
            saved_at: peet_platform::SystemTime::now()
                .duration_since(peet_platform::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs()),
            ..Default::default()
        };
        // Caches only describe a fully built model.
        let caches = (with_caches && !self.pending_rebuild).then(|| peet_io::document::Caches {
            bodies: &self.evaluation().bodies,
            meshes: self
                .bodies
                .iter()
                .map(|b| peet_io::document::CachedMesh {
                    stamp: b.stamp,
                    mesh: b.tess.clone(),
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
            .map(|b| b.solid.bounds())
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
        assert!(!body.mesh.indices.is_empty());
        assert_eq!(body.mesh.pick_ids.len(), body.mesh.vertices.len());
        assert_eq!(body.mesh.edge_pick_ids.len() * 2, body.mesh.edges.len());
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
