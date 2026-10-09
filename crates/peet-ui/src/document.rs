//! The open document as the UI uses it. The document itself (model, rebuilds, undo,
//! bodies) is headless and lives in `peet-document`; this adds what only an interactive
//! session has.

use peet_math::Plane;
use peet_sketch::Sketch;

pub use peet_document::{
    Document, FileLocation, ItemId, Persistent, Session, SketchStatus, sketch_bounds,
};

/// The sketch being edited: a working copy, written back to the model (as one undo step)
/// when editing ends.
#[derive(Clone, Debug)]
pub struct SketchItem {
    pub plane: Plane,
    /// Name of what the sketch lies on, for the properties panel.
    pub plane_name: String,
    pub sketch: Sketch,
    pub projections: Vec<peet_model::projection::Projection>,
    pub status: SketchStatus,
}

impl SketchItem {
    /// Working geometry for face sketches includes the supporting face's boundary.
    /// Resolve it from history before the sketch, so cuts made from this sketch cannot
    /// become references back into it. Unsupported boundary curves remain unprojected.
    pub fn from_feature(doc: &Document, id: peet_model::FeatureId) -> Option<(Self, usize)> {
        use peet_model::PlaneRef;
        use peet_model::naming::{find_edge, find_face};
        use peet_model::projection::{Projection, Source, project_edge};
        let feature = doc.model.sketch(id)?;
        let (plane, status) = doc.sketch_placement(id)?;
        let mut work = Self {
            plane,
            plane_name: doc.plane_name(&feature.plane),
            sketch: feature.sketch.clone(),
            projections: feature.projections.clone(),
            status,
        };
        let mut skipped = 0;
        if let PlaneRef::Face(face) = &feature.plane {
            let bodies = doc.bodies_before(id)?;
            if let Some(found) = find_face(bodies, face) {
                let body = &bodies[found.body];
                for &l in &body.solid.face(found.id).loops {
                    for c in body.solid.loop_coedges(l) {
                        let edge = body.solid.coedge(c).edge;
                        if work.projections.iter().any(|p| {
                            matches!(&p.source, Source::Edge(r) if find_edge(bodies,r).is_some_and(|e| e.body==found.body && e.id==edge))
                        }) { continue; }
                        match project_edge(body.solid.edge(edge), plane) {
                            Ok(shape) => {
                                if let Some(reference) = body.edge_ref(edge) {
                                    let entity = shape.add(&mut work.sketch, true);
                                    work.projections.push(Projection {
                                        entity,
                                        source: Source::Edge(reference),
                                    });
                                }
                            }
                            Err(_) => skipped += 1,
                        }
                    }
                }
            }
        }
        Some((work, skipped))
    }
}
