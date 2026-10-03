//! Persistent (topological) naming: references to faces, edges and vertices that survive
//! upstream edits.
//!
//! **The problem.** A sketch drawn on "face 7" breaks as soon as an earlier feature
//! changes and the kernel numbers the faces differently. Face indices mean nothing across
//! rebuilds.
//!
//! **Names.** Every face of every body carries a [`FaceName`]: *which feature made it and
//! what part of that feature it is*, never an index.
//!
//! - An extrusion names its two caps [`FaceRole::NearCap`] and [`FaceRole::FarCap`], and
//!   each wall after the sketch curve that swept it ([`FaceRole::Side`]). Sketch entity
//!   ids are stable for the life of the sketch, so "the wall from that line" stays the
//!   same face when the sketch is redimensioned, when curves are added or removed
//!   elsewhere in it, and when the region loops come out in a different order.
//! - Booleans report which input faces each result face is part of
//!   ([`peet_kernel::boolean::boolean_traced`]). A result face inherits the names of its
//!   sources, so a face keeps its name through any number of later features. A pocket's
//!   floor is "the far cap of the cut", not "the plate's top face, modified".
//! - Faces merged into one (two bosses joined flush) carry both origins. A face split in
//!   two (a slot across it) gives two faces with the same name.
//!
//! Edges and vertices are not named separately: an edge is where two named faces meet, a
//! vertex is where three or more do.
//!
//! **References.** Names alone can be ambiguous (the two halves of a split face), so a
//! reference ([`FaceRef`], [`EdgeRef`], [`VertexRef`]) records the name plus what told the
//! candidates apart when the user picked: the names of the neighbouring faces, and
//! as a last resort a point on the picked geometry. Resolution ([`find_face`] and
//! friends) is deterministic:
//!
//! 1. faces with exactly the referenced name; failing that, faces sharing origins with it
//!    (the face has since been merged with another, or un-merged), most shared first;
//! 2. among several, the one whose neighbours best match the recorded neighbours;
//! 3. among those still tied, the one closest to the recorded point.
//!
//! If no face shares an origin with the name, the reference is *missing* and the feature
//! using it fails with a message naming what it lost. It never silently jumps to an
//! unrelated face.

use std::sync::Arc;

use peet_kernel::{EdgeId, FaceId, Solid, VertexId};
use peet_math::DVec3;
use peet_sheetmetal::SheetBody;
use peet_sketch::EntityId;
use serde::{Deserialize, Serialize};

use crate::feature::FeatureId;

/// What part of its feature a face is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FaceRole {
    /// The cap of an extrusion at its start: on the sketch plane for a blind extrusion,
    /// on the side against the sketch normal for a mid-plane one.
    NearCap,
    /// The cap at the far end of an extrusion (a pocket's floor).
    FarCap,
    /// A wall swept by this curve of the feature's sketch (also a base flange's or a sheet
    /// metal cut's wall from that sketch curve).
    Side(EntityId),
    /// The top side of a sheet metal flange: `part` tells the flanges of one feature
    /// apart (the profile line for an open-profile base flange, else 0).
    SheetTop(u32),
    /// The bottom side of a sheet metal flange.
    SheetBottom(u32),
    /// The top side of a bend (the inside when it bends up).
    BendTop(u32),
    /// The bottom side of a bend.
    BendBottom(u32),
    /// A wall through the sheet that the feature made: a flange's end or sides, a relief
    /// (numbered within `part`, see `peet_sheetmetal::layout::wall`).
    Wall(u32, u8),
}

/// One feature's contribution to a face.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FaceOrigin {
    pub feature: FeatureId,
    pub role: FaceRole,
}

/// The persistent name of a face: the origins it is made of, sorted and without
/// duplicates. Almost always one.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FaceName(Vec<FaceOrigin>);

impl FaceName {
    pub fn new(feature: FeatureId, role: FaceRole) -> Self {
        Self(vec![FaceOrigin { feature, role }])
    }

    /// The name of a face made of several named faces.
    pub fn merged<'a>(names: impl IntoIterator<Item = &'a FaceName>) -> Self {
        let mut origins: Vec<FaceOrigin> = names
            .into_iter()
            .flat_map(|n| n.0.iter())
            .copied()
            .collect();
        origins.sort_unstable();
        origins.dedup();
        Self(origins)
    }

    pub fn origins(&self) -> &[FaceOrigin] {
        &self.0
    }

    /// The features that made the face.
    pub fn features(&self) -> impl Iterator<Item = FeatureId> + '_ {
        self.0.iter().map(|o| o.feature)
    }

    /// How many origins the two names share.
    pub fn shared(&self, other: &FaceName) -> usize {
        self.0.iter().filter(|o| other.0.contains(o)).count()
    }
}

/// A reference to a face that survives rebuilds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FaceRef {
    pub name: FaceName,
    /// Names of the faces around it when it was picked, sorted.
    pub neighbours: Vec<FaceName>,
    /// A point on the face when it was picked.
    pub hint: DVec3,
}

/// A reference to an edge: where two faces meet (the same face twice for a seam).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeRef {
    /// The two faces, in sorted order.
    pub faces: [FaceName; 2],
    /// The middle of the edge when it was picked.
    pub hint: DVec3,
}

impl EdgeRef {
    pub fn features(&self) -> impl Iterator<Item = FeatureId> + '_ {
        self.faces.iter().flat_map(|f| f.features())
    }
}

/// A reference to a vertex: where its faces meet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VertexRef {
    /// The faces around the vertex, sorted, without duplicates.
    pub faces: Vec<FaceName>,
    /// Where the vertex was when it was picked.
    pub hint: DVec3,
}

impl VertexRef {
    pub fn features(&self) -> impl Iterator<Item = FeatureId> + '_ {
        self.faces.iter().flat_map(|f| f.features())
    }
}

/// A solid body with the names of its faces.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Body {
    pub solid: Solid,
    /// The name of each face, by face index.
    pub face_names: Vec<FaceName>,
    /// The feature that created the body.
    pub origin: FeatureId,
    /// Identifies this exact geometry: two bodies with the same stamp are identical, so
    /// display meshes can be cached by it.
    pub stamp: u64,
    /// For sheet metal bodies: the sheet definition, the flat pattern and what each face
    /// is. Not saved in the B-rep cache (rebuilt from the model).
    #[serde(skip)]
    pub sheet: Option<Arc<SheetBody>>,
}

impl Body {
    pub fn face_name(&self, face: FaceId) -> &FaceName {
        &self.face_names[face.index()]
    }

    /// The faces on the two sides of an edge (`None` if the solid is not closed there).
    pub fn edge_faces(&self, edge: EdgeId) -> Option<[FaceId; 2]> {
        match self.solid.edge(edge).coedges[..] {
            [a, b] => Some([self.solid.coedge_face(a), self.solid.coedge_face(b)]),
            _ => None,
        }
    }

    /// The faces around a vertex, without duplicates, in face order.
    pub fn vertex_faces(&self, vertex: VertexId) -> Vec<FaceId> {
        let mut out = Vec::new();
        for e in &self.solid.edges {
            if e.start == vertex || e.end == vertex {
                for &c in &e.coedges {
                    out.push(self.solid.coedge_face(c));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// A representative point of a face: the mean of the midpoints of its boundary edges.
    /// It moves smoothly as the model changes, which is all a tie-break needs.
    pub fn face_center(&self, face: FaceId) -> DVec3 {
        let mut sum = DVec3::ZERO;
        let mut n = 0.0;
        for &l in &self.solid.face(face).loops {
            for c in self.solid.loop_coedges(l) {
                sum += self
                    .solid
                    .edge(self.solid.coedge(c).edge)
                    .point_at_fraction(0.5);
                n += 1.0;
            }
        }
        if n > 0.0 { sum / n } else { DVec3::ZERO }
    }

    fn neighbour_names(&self, face: FaceId) -> Vec<FaceName> {
        let mut names: Vec<FaceName> = self
            .solid
            .adjacent_faces(face)
            .into_iter()
            .map(|f| self.face_name(f).clone())
            .collect();
        names.sort_unstable();
        names
    }

    fn edge_names(&self, edge: EdgeId) -> Option<[FaceName; 2]> {
        let [a, b] = self.edge_faces(edge)?;
        let mut names = [self.face_name(a).clone(), self.face_name(b).clone()];
        names.sort_unstable();
        Some(names)
    }

    fn vertex_names(&self, vertex: VertexId) -> Vec<FaceName> {
        let mut names: Vec<FaceName> = self
            .vertex_faces(vertex)
            .into_iter()
            .map(|f| self.face_name(f).clone())
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// A persistent reference to one of this body's faces.
    pub fn face_ref(&self, face: FaceId) -> FaceRef {
        FaceRef {
            name: self.face_name(face).clone(),
            neighbours: self.neighbour_names(face),
            hint: self.face_center(face),
        }
    }

    /// A persistent reference to one of this body's edges (`None` on an open solid).
    pub fn edge_ref(&self, edge: EdgeId) -> Option<EdgeRef> {
        Some(EdgeRef {
            faces: self.edge_names(edge)?,
            hint: self.solid.edge(edge).point_at_fraction(0.5),
        })
    }

    /// A persistent reference to one of this body's vertices.
    pub fn vertex_ref(&self, vertex: VertexId) -> VertexRef {
        VertexRef {
            faces: self.vertex_names(vertex),
            hint: self.solid.vertex(vertex).point,
        }
    }
}

/// Where a reference points now: the body (index in the list searched) and the element.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Found<T> {
    pub body: usize,
    pub id: T,
}

/// How many names two sorted lists have in common, counting repeats.
fn common(a: &[FaceName], b: &[FaceName]) -> usize {
    let (mut i, mut j, mut n) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                n += 1;
                i += 1;
                j += 1;
            }
        }
    }
    n
}

/// Similarity of two neighbour lists in `0..=1` (1 when both are empty).
fn similarity(a: &[FaceName], b: &[FaceName]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    2.0 * common(a, b) as f64 / (a.len() + b.len()) as f64
}

/// Picks the best of `candidates`, each scored `(rank, similarity, distance)`: the lowest
/// rank wins, then the highest similarity, then the smallest distance, then the first.
fn best<T: Copy>(candidates: &[(Found<T>, usize, f64, f64)]) -> Option<Found<T>> {
    candidates
        .iter()
        .min_by(|a, b| {
            a.1.cmp(&b.1)
                .then(b.2.total_cmp(&a.2))
                .then(a.3.total_cmp(&b.3))
        })
        .map(|c| c.0)
}

/// Finds the face a reference points to among `bodies`, or `None` if nothing shares an
/// origin with its name any more.
pub fn find_face(bodies: &[Arc<Body>], r: &FaceRef) -> Option<Found<FaceId>> {
    let mut candidates = Vec::new();
    for (bi, body) in bodies.iter().enumerate() {
        for face in body.solid.face_ids() {
            let name = body.face_name(face);
            let shared = name.shared(&r.name);
            if shared == 0 {
                continue;
            }
            // Rank 0: the same name. Otherwise fewer missing and fewer extra origins first.
            let rank = (r.name.origins().len() - shared) * 1000 + (name.origins().len() - shared);
            candidates.push((bi, face, rank));
        }
    }
    let lowest = candidates.iter().map(|c| c.2).min()?;
    let scored: Vec<(Found<FaceId>, usize, f64, f64)> = candidates
        .into_iter()
        .filter(|c| c.2 == lowest)
        .map(|(bi, face, rank)| {
            let body = &bodies[bi];
            (
                Found { body: bi, id: face },
                rank,
                similarity(&body.neighbour_names(face), &r.neighbours),
                body.face_center(face).distance(r.hint),
            )
        })
        .collect();
    // With a single candidate the scores are not needed, but computing them is cheap.
    best(&scored)
}

/// Finds the edge a reference points to: the edge between the two named faces.
pub fn find_edge(bodies: &[Arc<Body>], r: &EdgeRef) -> Option<Found<EdgeId>> {
    let mut scored = Vec::new();
    for (bi, body) in bodies.iter().enumerate() {
        for edge in body.solid.edge_ids() {
            let Some(names) = body.edge_names(edge) else {
                continue;
            };
            // Either pairing of the edge's faces with the referenced ones.
            let pair = |x: usize, y: usize| {
                let (a, b) = (names[0].shared(&r.faces[x]), names[1].shared(&r.faces[y]));
                (a > 0 && b > 0).then(|| {
                    let exact =
                        usize::from(names[0] != r.faces[x]) + usize::from(names[1] != r.faces[y]);
                    let total = names[0].origins().len() + names[1].origins().len();
                    exact * 1000 + (total - a - b)
                })
            };
            let Some(rank) = [pair(0, 1), pair(1, 0)].into_iter().flatten().min() else {
                continue;
            };
            let mid = body.solid.edge(edge).point_at_fraction(0.5);
            scored.push((
                Found { body: bi, id: edge },
                rank,
                1.0,
                mid.distance(r.hint),
            ));
        }
    }
    best(&scored)
}

/// Finds the vertex a reference points to: the vertex where the named faces meet.
pub fn find_vertex(bodies: &[Arc<Body>], r: &VertexRef) -> Option<Found<VertexId>> {
    let mut scored = Vec::new();
    for (bi, body) in bodies.iter().enumerate() {
        for vertex in body.solid.vertex_ids() {
            let names = body.vertex_names(vertex);
            // Faces of the reference found at this vertex, by name or by a shared origin.
            let matched = r
                .faces
                .iter()
                .filter(|f| names.iter().any(|n| n.shared(f) > 0))
                .count();
            // A vertex is a meeting of at least three faces: fewer matches say nothing.
            if matched < r.faces.len().min(3) {
                continue;
            }
            let exact = common(&names, &r.faces);
            let rank = (r.faces.len() - matched) * 1000
                + (r.faces.len() - exact) * 10
                + names.len().abs_diff(r.faces.len());
            let p = body.solid.vertex(vertex).point;
            scored.push((
                Found {
                    body: bi,
                    id: vertex,
                },
                rank,
                1.0,
                p.distance(r.hint),
            ));
        }
    }
    best(&scored)
}
