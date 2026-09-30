use glam::{DMat3, DMat4, DQuat, DVec3};
use serde::{Deserialize, Serialize};

/// A right handed orthonormal coordinate system (a rigid placement) in model space.
///
/// Used for sketch planes, reference geometry and part placement.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub origin: DVec3,
    pub rotation: DQuat,
}

impl Default for Frame {
    fn default() -> Self {
        Self::WORLD
    }
}

impl Frame {
    pub const WORLD: Self = Self {
        origin: DVec3::ZERO,
        rotation: DQuat::IDENTITY,
    };

    /// Builds a frame from an origin, a normal (local Z) and a preferred local X direction.
    ///
    /// `x_hint` is projected onto the plane perpendicular to `z`. Returns `None` if either
    /// direction is degenerate or they are parallel.
    pub fn from_origin_z_x(origin: DVec3, z: DVec3, x_hint: DVec3) -> Option<Self> {
        let z = z.try_normalize()?;
        let x = (x_hint - z * x_hint.dot(z)).try_normalize()?;
        let y = z.cross(x);
        let rotation = DQuat::from_mat3(&DMat3::from_cols(x, y, z)).normalize();
        Some(Self { origin, rotation })
    }

    pub fn x_axis(&self) -> DVec3 {
        self.rotation * DVec3::X
    }

    pub fn y_axis(&self) -> DVec3 {
        self.rotation * DVec3::Y
    }

    pub fn z_axis(&self) -> DVec3 {
        self.rotation * DVec3::Z
    }

    /// Converts a point from this frame's local coordinates to world coordinates.
    pub fn to_world(&self, local: DVec3) -> DVec3 {
        self.origin + self.rotation * local
    }

    /// Converts a world point into this frame's local coordinates.
    pub fn to_local(&self, world: DVec3) -> DVec3 {
        self.rotation.inverse() * (world - self.origin)
    }

    pub fn vector_to_world(&self, local: DVec3) -> DVec3 {
        self.rotation * local
    }

    pub fn vector_to_local(&self, world: DVec3) -> DVec3 {
        self.rotation.inverse() * world
    }

    #[must_use]
    pub fn inverse(&self) -> Self {
        let inv = self.rotation.inverse();
        Self {
            origin: -(inv * self.origin),
            rotation: inv,
        }
    }

    /// Places `child` (given relative to `self`) in world space.
    #[must_use]
    pub fn compose(&self, child: &Self) -> Self {
        Self {
            origin: self.to_world(child.origin),
            rotation: (self.rotation * child.rotation).normalize(),
        }
    }

    /// The local-to-world matrix.
    pub fn to_mat4(&self) -> DMat4 {
        DMat4::from_rotation_translation(self.rotation, self.origin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: DVec3, b: DVec3) -> bool {
        a.abs_diff_eq(b, 1e-12)
    }

    #[test]
    fn construct_from_axes() {
        let f = Frame::from_origin_z_x(
            DVec3::new(1.0, 2.0, 3.0),
            DVec3::Y * 5.0,
            DVec3::X + DVec3::Y,
        )
        .unwrap();
        assert!(approx(f.z_axis(), DVec3::Y));
        assert!(approx(f.x_axis(), DVec3::X));
        assert!(approx(f.y_axis(), -DVec3::Z)); // right handed: y = z × x
        assert!(Frame::from_origin_z_x(DVec3::ZERO, DVec3::Z, DVec3::Z).is_none());
        assert!(Frame::from_origin_z_x(DVec3::ZERO, DVec3::ZERO, DVec3::X).is_none());
    }

    #[test]
    fn round_trip() {
        let f = Frame {
            origin: DVec3::new(10.0, -4.0, 2.5),
            rotation: DQuat::from_euler(glam::EulerRot::XYZ, 0.3, -1.1, 2.0),
        };
        let p = DVec3::new(3.0, 7.0, -9.0);
        assert!(approx(f.to_local(f.to_world(p)), p));
        assert!(approx(f.inverse().to_world(f.to_world(p)), p));
        assert!(approx(f.to_mat4().transform_point3(p), f.to_world(p)));
        let g = Frame {
            origin: DVec3::X,
            rotation: DQuat::from_rotation_z(0.7),
        };
        assert!(approx(f.compose(&g).to_world(p), f.to_world(g.to_world(p))));
    }
}
