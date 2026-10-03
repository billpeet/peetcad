//! Bodies for the viewport: the document's tessellation as GPU-ready mesh data.

use peet_kernel::tessellate::SolidMesh;
use peet_render::{MeshData, MeshVertex};

pub use peet_document::GeomRef;

/// Colour of solid bodies (sRGB), for parts without a colour of their own.
pub const BODY_COLOR: [u8; 3] = [196, 202, 210];

/// Tells the GPU mesh of a body in one colour from the same body in another: meshes are
/// kept by the stamp of the body combined with this.
pub fn color_key(color: Option<[u8; 3]>) -> u64 {
    color.map_or(0, |[r, g, b]| {
        (1 << 24 | u64::from(r) << 16 | u64::from(g) << 8 | u64::from(b))
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
    })
}

/// Converts a kernel tessellation to renderer mesh data in `color` (the usual colour of
/// bodies if `None`), with per-vertex face ids and per-segment edge ids for picking.
pub fn to_mesh_data(tess: &SolidMesh, color: Option<[u8; 3]>) -> MeshData {
    let [r, g, b] = color.unwrap_or(BODY_COLOR);
    let color = [r, g, b, 255];
    let mut mesh = MeshData::default();
    for face in &tess.faces {
        let base = mesh.vertices.len() as u32;
        for (p, n) in face.positions.iter().zip(&face.normals) {
            mesh.vertices.push(MeshVertex {
                position: p.as_vec3().to_array(),
                normal: n.as_vec3().to_array(),
                color,
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
        let mesh = to_mesh_data(doc.bodies[0].tess(), None);
        assert!(!mesh.indices.is_empty());
        assert_eq!(mesh.vertices[0].color[..3], BODY_COLOR);
        let red = to_mesh_data(doc.bodies[0].tess(), Some([200, 40, 40]));
        assert_eq!(red.vertices[0].color, [200, 40, 40, 255]);
        assert_eq!(color_key(None), 0);
        assert_ne!(color_key(Some([0, 0, 0])), color_key(None));
        assert_ne!(color_key(Some([0, 0, 1])), color_key(Some([0, 1, 0])));
        assert_eq!(mesh.pick_ids.len(), mesh.vertices.len());
        assert_eq!(mesh.edge_pick_ids.len() * 2, mesh.edges.len());
    }
}
