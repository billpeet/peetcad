//! Placing sketches on model faces.

use peet_kernel::{FaceId, Solid, Surface};
use peet_math::{DVec3, Plane};

/// The sketch plane for a planar face: normal pointing out of the material, origin at the
/// world origin projected onto the face's plane, and sketch X along world X projected onto
/// the plane (world Y if the face is perpendicular to X). The same face always gives the
/// same plane, so sketches line up with the standard views. `None` for curved faces.
pub fn face_sketch_plane(solid: &Solid, face: FaceId) -> Option<Plane> {
    let f = solid.faces.get(face.index())?;
    let Surface::Plane(plane) = f.surface else {
        return None;
    };
    let normal = if f.reversed {
        -plane.normal()
    } else {
        plane.normal()
    };
    let origin = plane.project_point(DVec3::ZERO);
    let x = if normal.cross(DVec3::X).length() > 0.1 {
        DVec3::X
    } else {
        DVec3::Y
    };
    Plane::from_origin_normal_x(origin, normal, x)
}
