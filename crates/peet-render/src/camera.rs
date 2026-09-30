//! The viewport camera: orbit, pan, zoom-to-cursor, standard views and smooth transitions.
//!
//! All camera math is `f64`. The camera orbits a `target` point at `distance`. Its
//! orientation is a quaternion (camera-to-world), so both turntable and trackball
//! navigation and any standard view can be represented without gimbal problems.
//!
//! Camera-local axes: +X right, +Y up, +Z backwards (the camera looks along -Z).

use glam::dcamera::rh::proj::directx as proj;
use peet_math::{Aabb, DMat3, DMat4, DQuat, DVec2, DVec3, Ray};

/// Perspective or orthographic projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Projection {
    Perspective,
    /// The CAD default: parallel lines stay parallel and dimensions read true.
    #[default]
    Orthographic,
}

/// How mouse drags rotate the view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OrbitStyle {
    /// Horizontal drags spin around the world Z axis, so "up" stays up. Best for most parts.
    #[default]
    Turntable,
    /// Free rotation around the screen axes.
    Trackball,
}

/// Named standard view directions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardView {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Isometric,
}

impl StandardView {
    pub const ALL: [Self; 7] = [
        Self::Front,
        Self::Back,
        Self::Left,
        Self::Right,
        Self::Top,
        Self::Bottom,
        Self::Isometric,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Front => "Front",
            Self::Back => "Back",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Top => "Top",
            Self::Bottom => "Bottom",
            Self::Isometric => "Isometric",
        }
    }

    /// The camera orientation (camera-to-world rotation) for this view.
    pub fn rotation(self) -> DQuat {
        match self {
            Self::Front => rotation_from_back_up(-DVec3::Y, DVec3::Z),
            Self::Back => rotation_from_back_up(DVec3::Y, DVec3::Z),
            Self::Left => rotation_from_back_up(-DVec3::X, DVec3::Z),
            Self::Right => rotation_from_back_up(DVec3::X, DVec3::Z),
            Self::Top => rotation_from_back_up(DVec3::Z, DVec3::Y),
            Self::Bottom => rotation_from_back_up(-DVec3::Z, -DVec3::Y),
            Self::Isometric => rotation_looking_from(DVec3::new(1.0, -1.0, 1.0)),
        }
    }
}

/// Camera orientation whose backwards axis (towards the viewer) is `back` and whose
/// up axis is as close to `up` as possible.
pub fn rotation_from_back_up(back: DVec3, up: DVec3) -> DQuat {
    let z = back.normalize();
    let y = (up - z * up.dot(z)).normalize();
    let x = y.cross(z);
    DQuat::from_mat3(&DMat3::from_cols(x, y, z)).normalize()
}

/// Camera orientation for viewing from direction `from` (pointing from the target to the
/// eye), keeping world Z up where possible.
pub fn rotation_looking_from(from: DVec3) -> DQuat {
    let back = from.normalize();
    let up = if back.cross(DVec3::Z).length() < 1e-6 {
        // Looking straight down or up: use +Y (or -Y from below) as screen up.
        if back.z > 0.0 { DVec3::Y } else { -DVec3::Y }
    } else {
        DVec3::Z
    };
    rotation_from_back_up(back, up)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// The point the camera orbits around and zooms relative to.
    pub target: DVec3,
    /// Camera-to-world rotation.
    pub rotation: DQuat,
    /// Distance from the eye to the target. In orthographic mode this sets the zoom level.
    pub distance: f64,
    pub projection: Projection,
    /// Vertical field of view in radians. Also scales the orthographic view, so switching
    /// projection keeps things at the target the same size on screen.
    pub fov_y: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            target: DVec3::ZERO,
            rotation: StandardView::Isometric.rotation(),
            distance: 400.0,
            projection: Projection::default(),
            fov_y: 30f64.to_radians(),
        }
    }
}

/// Closest and furthest distances the camera may zoom to (mm).
const MIN_DISTANCE: f64 = 1e-2;
const MAX_DISTANCE: f64 = 1e7;

impl Camera {
    pub fn right(&self) -> DVec3 {
        self.rotation * DVec3::X
    }

    pub fn up(&self) -> DVec3 {
        self.rotation * DVec3::Y
    }

    /// The viewing direction (from the eye towards the target).
    pub fn forward(&self) -> DVec3 {
        self.rotation * -DVec3::Z
    }

    pub fn eye(&self) -> DVec3 {
        self.target - self.forward() * self.distance
    }

    /// Height of the visible area, in world units, at the target's depth.
    pub fn view_height(&self) -> f64 {
        2.0 * self.distance * (self.fov_y * 0.5).tan()
    }

    /// World units per screen pixel at the target's depth.
    pub fn world_per_pixel(&self, viewport_height_px: f64) -> f64 {
        self.view_height() / viewport_height_px.max(1.0)
    }

    pub fn view_matrix(&self) -> DMat4 {
        glam::dcamera::rh::view::look_to_mat4(self.eye(), self.forward(), self.up())
    }

    /// Projection matrix with **reversed depth** (near maps to 1, far to 0) for better
    /// depth precision. `bounds` is everything that must not be clipped.
    pub fn projection_matrix(&self, aspect: f64, bounds: &Aabb) -> DMat4 {
        let (near, far) = self.clip_range(bounds);
        match self.projection {
            // Passing far/near swapped yields reversed depth.
            Projection::Perspective => proj::perspective(self.fov_y, aspect, far, near),
            Projection::Orthographic => {
                let half_h = self.view_height() * 0.5;
                let half_w = half_h * aspect;
                proj::orthographic(-half_w, half_w, -half_h, half_h, far, near)
            }
        }
    }

    /// Near and far clip distances along the view direction that enclose `bounds`.
    fn clip_range(&self, bounds: &Aabb) -> (f64, f64) {
        let (center, radius) = if bounds.is_empty() {
            (self.target, self.distance)
        } else {
            (bounds.center(), bounds.bounding_radius().max(1e-3))
        };
        let depth = (center - self.eye()).dot(self.forward());
        let margin = radius * 0.05 + 1e-3;
        let far = (depth + radius + margin).max(self.distance + margin);
        match self.projection {
            Projection::Perspective => {
                let near = (depth - radius - margin).max(far * 1e-5).max(1e-4);
                (near, far)
            }
            // Orthographic has no singularity at the eye, so the near plane may sit behind it.
            Projection::Orthographic => ((depth - radius - margin).min(0.0), far),
        }
    }

    /// Combined view-projection matrix.
    pub fn view_projection(&self, aspect: f64, bounds: &Aabb) -> DMat4 {
        self.projection_matrix(aspect, bounds) * self.view_matrix()
    }

    /// World point on the plane through the target (facing the camera) under a screen
    /// position given in normalized device coordinates (-1..1, +Y up).
    pub fn point_on_target_plane(&self, ndc: DVec2, aspect: f64) -> DVec3 {
        let half_h = self.view_height() * 0.5;
        let half_w = half_h * aspect;
        self.target + self.right() * (ndc.x * half_w) + self.up() * (ndc.y * half_h)
    }

    /// The ray from the eye through a screen position (normalized device coordinates).
    pub fn ray(&self, ndc: DVec2, aspect: f64) -> Ray {
        let p = self.point_on_target_plane(ndc, aspect);
        match self.projection {
            Projection::Perspective => Ray::new(self.eye(), p - self.eye()),
            Projection::Orthographic => {
                Ray::new(p - self.forward() * self.distance, self.forward())
            }
        }
    }

    /// Rotates the view by a mouse drag of `delta` pixels.
    pub fn orbit(&mut self, delta: DVec2, style: OrbitStyle) {
        const RADIANS_PER_PIXEL: f64 = 0.008;
        match style {
            OrbitStyle::Turntable => {
                let yaw = DQuat::from_rotation_z(-delta.x * RADIANS_PER_PIXEL);
                let pitch = DQuat::from_rotation_x(-delta.y * RADIANS_PER_PIXEL);
                self.rotation = (yaw * self.rotation * pitch).normalize();
            }
            OrbitStyle::Trackball => {
                let len = delta.length();
                if len > 0.0 {
                    let axis = DVec3::new(-delta.y, -delta.x, 0.0) / len;
                    let turn = DQuat::from_axis_angle(axis, len * RADIANS_PER_PIXEL);
                    self.rotation = (self.rotation * turn).normalize();
                }
            }
        }
    }

    /// Moves the view so the model follows a mouse drag of `delta` pixels (screen Y down).
    pub fn pan(&mut self, delta: DVec2, viewport_height_px: f64) {
        let wpp = self.world_per_pixel(viewport_height_px);
        self.target += (self.up() * delta.y - self.right() * delta.x) * wpp;
    }

    /// Zooms by `factor` (< 1 zooms in) keeping the point under `ndc` fixed on screen.
    pub fn zoom_at(&mut self, factor: f64, ndc: DVec2, aspect: f64) {
        let new_distance = (self.distance * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
        let factor = new_distance / self.distance;
        let anchor = self.point_on_target_plane(ndc, aspect);
        self.target = anchor + (self.target - anchor) * factor;
        self.distance = new_distance;
    }

    /// Centres `bounds` and zooms so it fills the view with a small margin.
    pub fn fit(&mut self, bounds: &Aabb, aspect: f64) {
        if bounds.is_empty() {
            return;
        }
        let radius = bounds.bounding_radius().max(1.0);
        // Fit the bounding sphere in the smaller of the two view dimensions.
        let half_fov_y = self.fov_y * 0.5;
        let half_fov_x = (half_fov_y.tan() * aspect.max(1e-3)).atan();
        let half_fov = half_fov_y.min(half_fov_x);
        self.target = bounds.center();
        self.distance = (radius * 1.1 / half_fov.tan()).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Interpolates between two cameras: slerp for rotation, log-space for distance.
    #[must_use]
    pub fn lerp(&self, to: &Self, t: f64) -> Self {
        Self {
            target: self.target.lerp(to.target, t),
            rotation: self.rotation.slerp(to.rotation, t).normalize(),
            distance: (self.distance.ln() * (1.0 - t) + to.distance.ln() * t).exp(),
            projection: to.projection,
            fov_y: self.fov_y + (to.fov_y - self.fov_y) * t,
        }
    }
}

/// A smooth transition between two camera states.
#[derive(Clone, Copy, Debug)]
pub struct CameraAnimation {
    from: Camera,
    to: Camera,
    elapsed: f64,
    duration: f64,
}

impl CameraAnimation {
    pub const DEFAULT_DURATION: f64 = 0.35;

    pub fn new(from: Camera, to: Camera, duration: f64) -> Self {
        Self {
            from,
            to,
            elapsed: 0.0,
            duration: duration.max(1e-6),
        }
    }

    /// Advances by `dt` seconds and returns the camera for this frame.
    pub fn step(&mut self, dt: f64) -> Camera {
        self.elapsed = (self.elapsed + dt).min(self.duration);
        let t = self.elapsed / self.duration;
        // Smootherstep easing: zero velocity and acceleration at both ends.
        let eased = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
        self.from.lerp(&self.to, eased)
    }

    pub fn is_finished(&self) -> bool {
        self.elapsed >= self.duration
    }

    pub fn target(&self) -> &Camera {
        &self.to
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: DVec3, b: DVec3) -> bool {
        a.abs_diff_eq(b, 1e-9)
    }

    #[test]
    fn standard_views_look_the_right_way() {
        let cam = |v: StandardView| Camera {
            rotation: v.rotation(),
            ..Camera::default()
        };
        let front = cam(StandardView::Front);
        assert!(approx(front.forward(), DVec3::Y));
        assert!(approx(front.up(), DVec3::Z));
        assert!(approx(front.right(), DVec3::X));
        let top = cam(StandardView::Top);
        assert!(approx(top.forward(), -DVec3::Z));
        assert!(approx(top.right(), DVec3::X));
        let right = cam(StandardView::Right);
        assert!(approx(right.forward(), -DVec3::X));
        assert!(approx(right.right(), DVec3::Y));
        let bottom = cam(StandardView::Bottom);
        assert!(approx(bottom.forward(), DVec3::Z));
        assert!(approx(bottom.right(), DVec3::X));
        let iso = cam(StandardView::Isometric);
        assert!(iso.forward().x < 0.0 && iso.forward().y > 0.0 && iso.forward().z < 0.0);
        assert!(iso.up().z > 0.0);
    }

    #[test]
    fn zoom_keeps_point_under_cursor_fixed() {
        for projection in [Projection::Perspective, Projection::Orthographic] {
            let mut cam = Camera {
                projection,
                ..Camera::default()
            };
            let ndc = DVec2::new(0.4, -0.3);
            let aspect = 1.5;
            let before = cam.point_on_target_plane(ndc, aspect);
            cam.zoom_at(0.5, ndc, aspect);
            let after = cam.point_on_target_plane(ndc, aspect);
            assert!(approx(before, after), "{projection:?}");
            assert!((cam.distance - 200.0).abs() < 1e-9);
        }
    }

    #[test]
    fn target_projects_to_screen_centre() {
        for projection in [Projection::Perspective, Projection::Orthographic] {
            let cam = Camera {
                projection,
                target: DVec3::new(10.0, 20.0, 30.0),
                ..Camera::default()
            };
            let bounds = Aabb::from_points([DVec3::splat(-100.0), DVec3::splat(100.0)]);
            let clip = cam.view_projection(1.0, &bounds) * cam.target.extend(1.0);
            let ndc = clip.truncate() / clip.w;
            assert!(ndc.x.abs() < 1e-9 && ndc.y.abs() < 1e-9, "{projection:?}");
            assert!(
                (0.0..=1.0).contains(&ndc.z),
                "{projection:?}: depth {}",
                ndc.z
            );
        }
    }

    #[test]
    fn reversed_depth_orders_near_before_far() {
        let cam = Camera {
            rotation: StandardView::Front.rotation(),
            projection: Projection::Perspective,
            ..Camera::default()
        };
        let bounds = Aabb::from_points([DVec3::splat(-50.0), DVec3::splat(50.0)]);
        let vp = cam.view_projection(1.0, &bounds);
        let depth = |p: DVec3| {
            let c = vp * p.extend(1.0);
            c.z / c.w
        };
        // Front view looks along +Y, so smaller Y is nearer and must have larger depth.
        assert!(depth(DVec3::new(0.0, -40.0, 0.0)) > depth(DVec3::new(0.0, 40.0, 0.0)));
    }

    #[test]
    fn pan_moves_model_with_cursor() {
        let mut cam = Camera {
            rotation: StandardView::Front.rotation(),
            ..Camera::default()
        };
        let wpp = cam.world_per_pixel(500.0);
        cam.pan(DVec2::new(10.0, 0.0), 500.0);
        // Dragging right moves the model right, i.e. the camera target moves left (-X).
        assert!(approx(cam.target, DVec3::new(-10.0 * wpp, 0.0, 0.0)));
    }

    #[test]
    fn turntable_keeps_world_up_vertical() {
        let mut cam = Camera::default();
        cam.orbit(DVec2::new(123.0, 0.0), OrbitStyle::Turntable);
        // A pure horizontal drag must not roll the camera: its right axis stays horizontal.
        assert!(cam.right().z.abs() < 1e-9);
    }

    #[test]
    fn fit_contains_bounds() {
        let mut cam = Camera::default();
        let bounds = Aabb::from_points([
            DVec3::new(100.0, 100.0, 0.0),
            DVec3::new(300.0, 200.0, 50.0),
        ]);
        cam.fit(&bounds, 1.0);
        assert!(approx(cam.target, bounds.center()));
        assert!(cam.view_height() * 0.5 >= bounds.bounding_radius());
    }

    #[test]
    fn animation_reaches_target() {
        let from = Camera::default();
        let to = Camera {
            rotation: StandardView::Top.rotation(),
            distance: 50.0,
            ..from
        };
        let mut anim = CameraAnimation::new(from, to, 0.3);
        let mid = anim.step(0.15);
        assert!(mid.distance < from.distance && mid.distance > to.distance);
        let end = anim.step(1.0);
        assert!(anim.is_finished());
        assert!((end.distance - to.distance).abs() < 1e-9);
        assert!(end.rotation.abs_diff_eq(to.rotation, 1e-9));
    }
}
