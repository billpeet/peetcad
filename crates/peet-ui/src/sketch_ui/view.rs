//! Mapping between sketch-plane coordinates and screen positions.

use egui::{Pos2, Rect, pos2};
use peet_math::{Aabb, DMat4, DVec2, DVec4, Plane};
use peet_render::Camera;

/// How the sketch plane maps to the viewport this frame.
#[derive(Clone, Debug)]
pub struct SketchView {
    pub rect: Rect,
    pub plane: Plane,
    camera: Camera,
    aspect: f64,
    /// Plane-to-clip-space matrix.
    to_clip: DMat4,
    /// Sketch units (mm) per screen point, measured at the middle of the viewport.
    px: f64,
}

impl SketchView {
    pub fn new(camera: &Camera, rect: Rect, plane: Plane) -> Self {
        let aspect = if rect.height() > 0.0 {
            f64::from(rect.width() / rect.height())
        } else {
            1.0
        };
        // Clip planes don't affect where points land on screen, so any bounds will do.
        let view_proj = camera.view_projection(aspect, &Aabb::EMPTY);
        let to_clip = view_proj * plane.frame.to_mat4();
        let mut view = Self {
            rect,
            plane,
            camera: *camera,
            aspect,
            to_clip,
            px: camera.world_per_pixel(f64::from(rect.height())),
        };
        let c = rect.center();
        if let (Some(a), Some(b)) = (view.to_sketch(c), view.to_sketch(c + egui::vec2(1.0, 0.0))) {
            let d = a.distance(b);
            if d.is_finite() && d > 0.0 {
                view.px = d;
            }
        }
        view
    }

    /// Sketch units per screen point: multiply a pixel tolerance by this to get mm.
    pub fn px(&self) -> f64 {
        self.px
    }

    /// Screen position of a sketch point. Points behind a perspective camera land far
    /// off screen.
    pub fn to_screen(&self, p: DVec2) -> Pos2 {
        let clip = self.to_clip * DVec4::new(p.x, p.y, 0.0, 1.0);
        if clip.w <= 1e-12 {
            return pos2(-1e6, -1e6);
        }
        let ndc = clip.truncate() / clip.w;
        let r = self.rect;
        pos2(
            r.left() + ((ndc.x + 1.0) * 0.5) as f32 * r.width(),
            r.top() + ((1.0 - ndc.y) * 0.5) as f32 * r.height(),
        )
        .clamp(pos2(-1e6, -1e6), pos2(1e6, 1e6))
    }

    /// Sketch point under a screen position, or `None` if the plane is seen edge-on there.
    pub fn to_sketch(&self, pos: Pos2) -> Option<DVec2> {
        let r = self.rect;
        let ndc = DVec2::new(
            f64::from((pos.x - r.left()) / r.width()) * 2.0 - 1.0,
            1.0 - f64::from((pos.y - r.top()) / r.height()) * 2.0,
        );
        let ray = self.camera.ray(ndc, self.aspect);
        let t = self.plane.intersect_ray(&ray)?;
        Some(self.plane.to_plane_coords(ray.at(t)))
    }

    /// Screen-space direction of a sketch-space direction at `at` (normalized, or zero).
    pub fn screen_dir(&self, at: DVec2, dir: DVec2) -> egui::Vec2 {
        let a = self.to_screen(at);
        let b = self.to_screen(at + dir * self.px * 20.0);
        (b - a).normalized()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peet_render::StandardView;

    #[test]
    fn round_trip_front_plane() {
        let camera = Camera {
            rotation: StandardView::Front.rotation(),
            projection: peet_render::Projection::Orthographic,
            ..Camera::default()
        };
        let rect = Rect::from_min_size(pos2(10.0, 20.0), egui::vec2(800.0, 600.0));
        let view = SketchView::new(&camera, rect, Plane::front());
        let p = DVec2::new(12.0, -7.5);
        let s = view.to_screen(p);
        let back = view.to_sketch(s).unwrap();
        assert!(back.abs_diff_eq(p, 1e-3), "{back:?}");
        // Sketch +Y is screen up on the front plane.
        assert!(view.to_screen(DVec2::Y * 10.0).y < view.to_screen(DVec2::ZERO).y);
        assert!(view.px() > 0.0);
    }
}
