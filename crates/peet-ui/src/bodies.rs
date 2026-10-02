//! Bodies as the UI sees them: the model's body plus its display tessellation.

use std::ops::Deref;
use std::sync::Arc;

use peet_kernel::tessellate::{Silhouettes, SolidMesh, tessellate};
use peet_kernel::{EdgeId, FaceId, VertexId};
use peet_render::{MeshData, MeshVertex};

/// Colour of solid bodies (sRGB).
const BODY_COLOR: [u8; 4] = [196, 202, 210, 255];

/// A face, edge or vertex of a body, for picking and selection. Only valid for the
/// current rebuild: across rebuilds, selections go through persistent references.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GeomRef {
    Face { body: usize, face: FaceId },
    Edge { body: usize, edge: EdgeId },
    Vertex { body: usize, vertex: VertexId },
}

impl GeomRef {
    pub fn body(self) -> usize {
        match self {
            Self::Face { body, .. } | Self::Edge { body, .. } | Self::Vertex { body, .. } => body,
        }
    }
}

#[derive(Debug)]
pub struct BodyView {
    pub body: Arc<peet_model::Body>,
    /// Kernel tessellation (kept for highlighting faces and edges).
    pub tess: SolidMesh,
    /// GPU-ready mesh: per-vertex face ids and per-segment edge ids for picking.
    pub mesh: MeshData,
    /// Precomputed data for the view-dependent silhouette lines of curved faces.
    pub silhouettes: Silhouettes,
    /// Set if tessellation failed (the body is then drawn empty).
    pub error: Option<String>,
}

impl Deref for BodyView {
    type Target = peet_model::Body;

    fn deref(&self) -> &Self::Target {
        &self.body
    }
}

/// The tessellation tolerance for a solid: finer for small parts.
pub fn tolerance(solid: &peet_kernel::Solid) -> f64 {
    let size = solid.bounds().size().length();
    (size * 5e-4).clamp(0.005, 0.1)
}

impl BodyView {
    /// Tessellates `body`, or uses `cached` (a mesh of this exact body from a file).
    pub fn new(body: Arc<peet_model::Body>, cached: Option<SolidMesh>) -> Self {
        let (tess, error) = match cached {
            Some(t) if t.faces.len() == body.solid.faces.len() => (t, None),
            _ => match tessellate(&body.solid, tolerance(&body.solid)) {
                Ok(t) => (t, None),
                Err(e) => (SolidMesh::default(), Some(e.to_string())),
            },
        };
        let mesh = to_mesh_data(&tess);
        Self {
            silhouettes: Silhouettes::new(&body.solid),
            body,
            tess,
            mesh,
            error,
        }
    }
}

/// Converts a kernel tessellation to renderer mesh data.
pub fn to_mesh_data(tess: &SolidMesh) -> MeshData {
    let mut mesh = MeshData::default();
    for face in &tess.faces {
        let base = mesh.vertices.len() as u32;
        for (p, n) in face.positions.iter().zip(&face.normals) {
            mesh.vertices.push(MeshVertex {
                position: p.as_vec3().to_array(),
                normal: n.as_vec3().to_array(),
                color: BODY_COLOR,
            });
            mesh.pick_ids.push(face.face.0);
        }
        for t in &face.triangles {
            mesh.indices.extend(t.iter().map(|i| base + i));
        }
    }
    for edge in &tess.edges {
        for w in edge.points.windows(2) {
            mesh.edges.push(w[0].as_vec3().to_array());
            mesh.edges.push(w[1].as_vec3().to_array());
            mesh.edge_pick_ids.push(edge.edge.0);
        }
    }
    mesh
}
