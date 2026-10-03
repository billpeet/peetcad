//! Bodies for the viewport: the document's tessellation as GPU-ready mesh data.

use peet_kernel::tessellate::SolidMesh;
use peet_render::{MeshData, MeshVertex};

pub use peet_document::GeomRef;

/// Colour of solid bodies (sRGB).
const BODY_COLOR: [u8; 4] = [196, 202, 210, 255];

/// Converts a kernel tessellation to renderer mesh data, with per-vertex face ids and
/// per-segment edge ids for picking.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_data_carries_pick_ids_for_every_vertex_and_edge_segment() {
        let (model, _) = peet_model::samples::bracket();
        let doc = peet_document::Document::from_model(model, None);
        let mesh = to_mesh_data(doc.bodies[0].tess());
        assert!(!mesh.indices.is_empty());
        assert_eq!(mesh.pick_ids.len(), mesh.vertices.len());
        assert_eq!(mesh.edge_pick_ids.len() * 2, mesh.edges.len());
    }
}
