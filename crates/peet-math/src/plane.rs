use glam::{DVec2, DVec3};
use serde::{Deserialize, Serialize};

use crate::{Frame, Ray, tolerance};

/// An infinite plane with an in-plane coordinate system, so 2D sketch coordinates are well defined.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plane {
    pub frame: Frame,
}

impl Plane {
    /// The XY plane (normal +Z). Called "Top" in the UI.
    pub const TOP: Self = Self {
        frame: Frame::WORLD,
    };

    /// The XZ plane, with normal -Y so it faces the viewer in the Front view. Called "Front" in the UI.
    pub fn front() -> Self {
        Self::from_origin_normal_x(DVec3::ZERO, -DVec3::Y, DVec3::X).expect("valid axes")
    }

    /// The YZ plane (normal +X). Called "Right" in the UI.
    pub fn right() -> Self {
        Self::from_origin_normal_x(DVec3::ZERO, DVec3::X, DVec3::Y).expect("valid axes")
    }

    pub fn from_origin_normal_x(origin: DVec3, normal: DVec3, x_dir: DVec3) -> Option<Self> {
        Frame::from_origin_z_x(origin, normal, x_dir).map(|frame| Self { frame })
    }

    pub fn origin(&self) -> DVec3 {
        self.frame.origin
    }

    pub fn normal(&self) -> DVec3 {
        self.frame.z_axis()
    }

    /// Positive on the side the normal points to.
    pub fn signed_distance(&self, p: DVec3) -> f64 {
        (p - self.origin()).dot(self.normal())
    }

    pub fn project_point(&self, p: DVec3) -> DVec3 {
        p - self.normal() * self.signed_distance(p)
    }

    /// 2D coordinates of a point in the plane's own coordinate system (after projection).
    pub fn to_plane_coords(&self, p: DVec3) -> DVec2 {
        self.frame.to_local(p).truncate()
    }

    pub fn from_plane_coords(&self, uv: DVec2) -> DVec3 {
        self.frame.to_world(uv.extend(0.0))
    }

    /// Ray parameter `t` where the ray hits the plane, or `None` if the ray is parallel to it.
    /// Hits behind the ray origin (negative `t`) are returned too.
    pub fn intersect_ray(&self, ray: &Ray) -> Option<f64> {
        let denom = ray.direction.dot(self.normal());
        if denom.abs() <= tolerance::ANGULAR {
            return None;
        }
        Some((self.origin() - ray.origin).dot(self.normal()) / denom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_planes() {
        assert_eq!(Plane::TOP.normal(), DVec3::Z);
        assert!(Plane::front().normal().abs_diff_eq(-DVec3::Y, 1e-12));
        assert!(Plane::right().normal().abs_diff_eq(DVec3::X, 1e-12));
        // Front plane: sketch X is world X, sketch Y is world Z (up).
        assert!(Plane::front().frame.y_axis().abs_diff_eq(DVec3::Z, 1e-12));
        // Right plane: sketch X is world Y, sketch Y is world Z (up).
        assert!(Plane::right().frame.y_axis().abs_diff_eq(DVec3::Z, 1e-12));
    }

    #[test]
    fn distance_and_projection() {
        let p = DVec3::new(3.0, 4.0, 5.0);
        assert_eq!(Plane::TOP.signed_distance(p), 5.0);
        assert_eq!(Plane::TOP.project_point(p), DVec3::new(3.0, 4.0, 0.0));
        let uv = Plane::front().to_plane_coords(p);
        assert!(uv.abs_diff_eq(DVec2::new(3.0, 5.0), 1e-12));
        let back = Plane::front().from_plane_coords(uv);
        assert!(back.abs_diff_eq(DVec3::new(3.0, 0.0, 5.0), 1e-12));
    }

    #[test]
    fn ray_hit() {
        let ray = Ray::new(DVec3::new(1.0, 2.0, 10.0), -DVec3::Z);
        let t = Plane::TOP.intersect_ray(&ray).unwrap();
        assert_eq!(t, 10.0);
        assert_eq!(ray.at(t), DVec3::new(1.0, 2.0, 0.0));
        assert!(
            Plane::TOP
                .intersect_ray(&Ray::new(DVec3::Z, DVec3::X))
                .is_none()
        );
    }
}
