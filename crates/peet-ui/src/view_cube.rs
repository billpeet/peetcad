//! The view cube and axis triad, drawn over the viewport with the UI painter.
//!
//! Each cube face is split into a 3×3 grid: the centre cell looks straight at that face,
//! edge cells look at the edge between two faces, and corner cells give the eight
//! isometric-style views. That is 26 one-click views, as in most CAD tools.

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2};
use peet_math::{DQuat, DVec3};
use peet_render::camera::rotation_looking_from;
use peet_render::{Camera, StandardView};

/// Half the cube's edge length, in points.
const CUBE_HALF: f32 = 34.0;
/// Fraction of the face (from its centre) taken by the centre cell.
const CENTER_CELL: f64 = 0.55;

struct Face {
    normal: DVec3,
    up: DVec3,
    label: &'static str,
    view: StandardView,
}

const FACES: [Face; 6] = [
    Face {
        normal: DVec3::NEG_Y,
        up: DVec3::Z,
        label: "FRONT",
        view: StandardView::Front,
    },
    Face {
        normal: DVec3::Y,
        up: DVec3::Z,
        label: "BACK",
        view: StandardView::Back,
    },
    Face {
        normal: DVec3::X,
        up: DVec3::Z,
        label: "RIGHT",
        view: StandardView::Right,
    },
    Face {
        normal: DVec3::NEG_X,
        up: DVec3::Z,
        label: "LEFT",
        view: StandardView::Left,
    },
    Face {
        normal: DVec3::Z,
        up: DVec3::Y,
        label: "TOP",
        view: StandardView::Top,
    },
    Face {
        normal: DVec3::NEG_Z,
        up: DVec3::NEG_Y,
        label: "BOTTOM",
        view: StandardView::Bottom,
    },
];

/// Space the cube needs in the top-right corner of the viewport.
pub fn cube_rect(viewport: Rect) -> Rect {
    let size = CUBE_HALF * 3.4;
    Rect::from_min_size(
        pos2(viewport.right() - size - 8.0, viewport.top() + 8.0),
        Vec2::splat(size),
    )
}

/// Draws the view cube. Returns the camera rotation to animate to if a cell was clicked.
pub fn view_cube(ui: &mut Ui, viewport: Rect, camera: &Camera, dark: bool) -> Option<DQuat> {
    let area = cube_rect(viewport);
    let center = area.center();
    let response = ui.interact(area, ui.id().with("view_cube"), Sense::click());
    let pointer = response.hover_pos();

    let inv = camera.rotation.inverse();
    let project = |p: DVec3| -> Pos2 {
        let v = inv * p;
        center + vec2(v.x as f32, -v.y as f32) * CUBE_HALF
    };

    let (face_fill, edge_stroke, text_color, hover_fill) = if dark {
        (
            Color32::from_rgb(112, 120, 136),
            Color32::from_rgb(170, 178, 192),
            Color32::from_rgb(240, 242, 246),
            Color32::from_rgb(64, 128, 222),
        )
    } else {
        (
            Color32::from_rgb(250, 251, 253),
            Color32::from_rgb(110, 118, 130),
            Color32::from_rgb(40, 44, 52),
            Color32::from_rgb(90, 150, 240),
        )
    };

    let painter = ui.painter_at(area.expand(4.0));
    let mut clicked = None;
    let bounds = [-1.0, -CENTER_CELL, CENTER_CELL, 1.0];

    for face in &FACES {
        let facing = (inv * face.normal).z;
        if facing <= 1e-3 {
            continue;
        }
        let right = face.up.cross(face.normal);
        // Shade faces by how directly they face the viewer, for a sense of depth.
        let shade = 0.78 + 0.22 * facing as f32;
        let fill = scale_color(face_fill, shade);

        for (ci, i) in [-1i32, 0, 1].into_iter().enumerate() {
            for (cj, j) in [-1i32, 0, 1].into_iter().enumerate() {
                let (a0, a1) = (bounds[ci], bounds[ci + 1]);
                let (b0, b1) = (bounds[cj], bounds[cj + 1]);
                let corner = |a: f64, b: f64| project(face.normal + right * a + face.up * b);
                let quad = [
                    corner(a0, b0),
                    corner(a1, b0),
                    corner(a1, b1),
                    corner(a0, b1),
                ];
                let hovered = pointer.is_some_and(|p| point_in_convex(p, &quad));
                let color = if hovered { hover_fill } else { fill };
                painter.add(Shape::convex_polygon(
                    quad.to_vec(),
                    color,
                    Stroke::new(0.5, edge_stroke.gamma_multiply(0.35)),
                ));
                if hovered && response.clicked() {
                    clicked = Some(if i == 0 && j == 0 {
                        face.view.rotation()
                    } else {
                        let dir = face.normal + right * f64::from(i) + face.up * f64::from(j);
                        rotation_looking_from(dir)
                    });
                }
            }
        }

        // Face outline.
        let outline: Vec<Pos2> = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .iter()
            .map(|(a, b)| project(face.normal + right * *a + face.up * *b))
            .collect();
        painter.add(Shape::closed_line(outline, Stroke::new(1.0, edge_stroke)));

        // Label, rotated to lie along the face's own "right" direction.
        if facing > 0.3 {
            let origin = project(face.normal);
            let along = project(face.normal + right) - origin;
            let angle = along.y.atan2(along.x);
            let galley = painter.layout_no_wrap(
                face.label.to_owned(),
                FontId::proportional(10.5),
                text_color,
            );
            let pos = origin - galley.size() * 0.5;
            let opacity = ((facing as f32 - 0.3) / 0.4).clamp(0.0, 1.0);
            painter.add(
                egui::epaint::TextShape::new(pos, galley, text_color)
                    .with_angle_and_anchor(angle, Align2::CENTER_CENTER)
                    .with_opacity_factor(opacity),
            );
        }
    }

    if response.hovered() {
        response.on_hover_text("Click a face, edge or corner to look from there");
    }
    clicked
}

/// Draws the XYZ axis triad in the bottom-left corner of the viewport.
pub fn axis_triad(ui: &Ui, viewport: Rect, camera: &Camera) {
    let origin = pos2(viewport.left() + 44.0, viewport.bottom() - 44.0);
    let length = 30.0;
    let inv = camera.rotation.inverse();
    let axes = [
        (DVec3::X, "X", Color32::from_rgb(226, 86, 86)),
        (DVec3::Y, "Y", Color32::from_rgb(112, 196, 92)),
        (DVec3::Z, "Z", Color32::from_rgb(84, 146, 240)),
    ];
    let mut projected: Vec<(f64, Vec2, &str, Color32)> = axes
        .iter()
        .map(|(axis, label, color)| {
            let v = inv * *axis;
            (v.z, vec2(v.x as f32, -v.y as f32), *label, *color)
        })
        .collect();
    // Draw axes pointing away from the viewer first.
    projected.sort_by(|a, b| a.0.total_cmp(&b.0));
    let painter = ui.painter();
    for (_, dir, label, color) in projected {
        let tip = origin + dir * length;
        painter.line_segment([origin, tip], Stroke::new(2.0, color));
        painter.text(
            origin + dir * (length + 9.0),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(12.0),
            color,
        );
    }
}

fn scale_color(c: Color32, k: f32) -> Color32 {
    let f = |v: u8| (f32::from(v) * k).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

/// Point-in-polygon test for a convex polygon of either winding.
fn point_in_convex(p: Pos2, poly: &[Pos2]) -> bool {
    let mut sign = 0.0f32;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let cross = (b - a).x * (p - a).y - (b - a).y * (p - a).x;
        if cross.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    sign != 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convex_hit_test_either_winding() {
        let square = [
            pos2(0.0, 0.0),
            pos2(10.0, 0.0),
            pos2(10.0, 10.0),
            pos2(0.0, 10.0),
        ];
        assert!(point_in_convex(pos2(5.0, 5.0), &square));
        assert!(!point_in_convex(pos2(15.0, 5.0), &square));
        let mut reversed = square;
        reversed.reverse();
        assert!(point_in_convex(pos2(5.0, 5.0), &reversed));
    }

    #[test]
    fn face_frames_are_right_handed() {
        for face in &FACES {
            let right = face.up.cross(face.normal);
            assert!((right.length() - 1.0).abs() < 1e-12);
            // The centre cell must match the standard view's camera orientation.
            let rot = face.view.rotation();
            assert!(
                (rot * DVec3::Z).abs_diff_eq(face.normal, 1e-9),
                "{}",
                face.label
            );
            assert!((rot * DVec3::X).abs_diff_eq(right, 1e-9), "{}", face.label);
        }
    }
}
