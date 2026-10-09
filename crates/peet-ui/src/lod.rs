//! What of a large assembly is drawn, and how finely.
//!
//! Each frame every shown body is looked at once: one outside the view is not drawn (nor
//! tessellated, nor sent to the GPU, until it comes into view); one that is small on
//! screen is drawn with a coarser mesh, and smaller still without its edges and
//! silhouette lines, which at that size are noise. Nothing here touches the GPU.

use std::collections::HashSet;

use peet_document::Document;
use peet_math::{Aabb, DMat4, DVec3, Frame};
use peet_render::{Camera, Projection};

/// A body drawn smaller than this (the diagonal of its box, in pixels) gets the coarse
/// mesh: the coarse mesh's facets are then under a pixel from the fine one's.
pub const COARSE_BELOW_PX: f64 = 100.0;
/// A body drawn smaller than this has no edges or silhouette lines.
pub const NO_EDGES_BELOW_PX: f64 = 28.0;

/// How a body is drawn this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// Not at all: its component is hidden, or it is outside the view.
    None,
    /// With the coarse mesh, and its edges if it is not too small for them.
    Coarse {
        edges: bool,
    },
    Full,
}

impl Detail {
    pub fn is_drawn(self) -> bool {
        self != Self::None
    }

    pub fn is_coarse(self) -> bool {
        matches!(self, Self::Coarse { .. })
    }

    /// Whether the body's edges and silhouette lines are drawn.
    pub fn edges(self) -> bool {
        matches!(self, Self::Full | Self::Coarse { edges: true })
    }
}

/// What the viewport keeps for each shown body between changes to the document.
#[derive(Clone, Copy, Debug)]
pub struct BodyInfo {
    /// The key of its GPU meshes: its stamp and its colour.
    pub key: u64,
    /// Its box, in its own coordinates.
    pub bounds: Aabb,
    /// Its component is hidden.
    pub hidden: bool,
}

/// The key, box and visibility of every shown body of `doc`.
pub fn body_infos(doc: &Document) -> Vec<BodyInfo> {
    let hidden: HashSet<peet_model::CompId> = doc
        .model
        .assembly()
        .map(|a| {
            a.components()
                .filter(|c| !c.visible)
                .map(|c| c.id)
                .collect()
        })
        .unwrap_or_default();
    doc.bodies
        .iter()
        .zip(&doc.placed)
        .map(|(body, placed)| BodyInfo {
            key: body.stamp ^ crate::bodies::color_key(placed.color),
            bounds: body.bounds(),
            hidden: placed.component().is_some_and(|c| hidden.contains(&c)),
        })
        .collect()
}

/// The view a frame is drawn with, as far as level of detail needs it.
pub struct LodView<'a> {
    pub camera: &'a Camera,
    pub view_proj: DMat4,
    /// The height of the viewport, in the pixels the thresholds are in.
    pub height_px: f64,
}

impl LodView<'_> {
    /// How large something of `radius` at `center` is drawn: its diameter in pixels.
    /// Infinite for something the eye is in or right in front of.
    pub fn size_px(&self, center: DVec3, radius: f64) -> f64 {
        let at_target = self.camera.world_per_pixel(self.height_px);
        let per_pixel = match self.camera.projection {
            Projection::Orthographic => at_target,
            Projection::Perspective => {
                let depth = (center - self.camera.eye()).dot(self.camera.forward());
                if depth <= radius {
                    return f64::INFINITY;
                }
                at_target * depth / self.camera.distance
            }
        };
        2.0 * radius / per_pixel.max(f64::MIN_POSITIVE)
    }

    /// How a body with the box `bounds`, placed at `frame`, is drawn.
    pub fn detail(&self, info: &BodyInfo, frame: &Frame) -> Detail {
        if info.hidden || info.bounds.min.cmpgt(info.bounds.max).any() {
            return Detail::None;
        }
        let transform = if *frame == Frame::WORLD {
            DMat4::IDENTITY
        } else {
            frame.to_mat4()
        };
        if !peet_render::in_view(&self.view_proj, &info.bounds, &transform) {
            return Detail::None;
        }
        let radius = info.bounds.size().length() * 0.5;
        let size = self.size_px(frame.to_world(info.bounds.center()), radius);
        if size >= COARSE_BELOW_PX {
            Detail::Full
        } else {
            Detail::Coarse {
                edges: size >= NO_EDGES_BELOW_PX,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use peet_math::{DVec2, Plane};
    use peet_model::{Engine, Model, Operation, PlaneRef, Scalar, StdPlane};

    use super::*;

    /// A plate with a hole, `w` wide.
    fn plate(w: f64) -> Arc<Model> {
        let mut m = Model::new();
        m.name = format!("Plate{w}");
        let s = m.add_sketch(PlaneRef::Standard(StdPlane::Top), Plane::TOP);
        let sketch = &mut m.feature_mut(s).unwrap().sketch_mut().unwrap().sketch;
        peet_sketch::shapes::rectangle(sketch, DVec2::ZERO, DVec2::new(w, 30.0));
        sketch.add_circle(DVec2::new(w / 2.0, 15.0), 5.0);
        let e = m.add_extrude(s, Operation::Add);
        m.feature_mut(e)
            .unwrap()
            .extrude_mut()
            .unwrap()
            .params
            .depth = Scalar::new(4.0);
        Engine::new().regenerate(&mut m);
        Arc::new(m)
    }

    /// `parts` different plates, `each` instances of each, on a grid 100 apart.
    fn grid(parts: usize, each: usize) -> Document {
        let mut model = Model::new_assembly();
        let assembly = model.assembly_mut().unwrap();
        let side = ((parts * each) as f64).cbrt().ceil() as usize;
        let mut n = 0;
        for p in 0..parts {
            let definition = assembly.define(plate(40.0 + p as f64));
            for _ in 0..each {
                let (x, y, z) = (n % side, n / side % side, n / (side * side));
                n += 1;
                assembly.insert(
                    definition,
                    Frame {
                        origin: DVec3::new(x as f64, y as f64, z as f64) * 100.0,
                        ..Frame::WORLD
                    },
                );
            }
        }
        Document::from_model(model, None)
    }

    fn view<'a>(camera: &'a Camera, scene: &Aabb) -> LodView<'a> {
        LodView {
            camera,
            view_proj: camera.view_projection(1.5, scene),
            height_px: 800.0,
        }
    }

    #[test]
    fn what_is_small_is_coarse_and_what_is_out_of_view_is_not_drawn() {
        let doc = grid(3, 9);
        let infos = body_infos(&doc);
        let scene = doc.visible_body_bounds();
        for projection in [Projection::Orthographic, Projection::Perspective] {
            let mut camera = Camera {
                projection,
                ..Camera::default()
            };
            let details = |camera: &Camera| -> Vec<Detail> {
                let view = view(camera, &scene);
                infos
                    .iter()
                    .zip(&doc.placed)
                    .map(|(info, placed)| view.detail(info, &placed.shown))
                    .collect()
            };
            // Everything framed: 27 plates of 50 mm in a 240 mm cube, 800 pixels high.
            camera.fit(&scene, 1.5);
            let all = details(&camera);
            assert!(all.iter().all(|d| d.is_drawn()), "{projection:?}");
            // Zoomed far out they are specks: coarse, and without edges.
            camera.distance *= 40.0;
            let far = details(&camera);
            assert!(
                far.iter().all(|d| *d == Detail::Coarse { edges: false }),
                "{projection:?}: {far:?}"
            );
            // Zoomed in on the first: it is drawn in full, and most others not at all.
            camera.fit(&infos[0].bounds, 1.5);
            let near = details(&camera);
            assert_eq!(near[0], Detail::Full);
            let drawn = near.iter().filter(|d| d.is_drawn()).count();
            assert!(drawn < 14, "{projection:?}: {drawn} drawn");
        }

        // A hidden component's bodies are not drawn wherever they are.
        let mut hidden = doc.model.clone();
        let first = hidden.assembly().unwrap().components().next().unwrap().id;
        hidden
            .assembly_mut()
            .unwrap()
            .component_mut(first)
            .unwrap()
            .visible = false;
        let doc = Document::from_model(hidden, None);
        let infos = body_infos(&doc);
        assert!(infos[0].hidden && !infos[1].hidden);
        let mut camera = Camera::default();
        camera.fit(&scene, 1.5);
        assert_eq!(
            view(&camera, &scene).detail(&infos[0], &doc.placed[0].shown),
            Detail::None
        );
    }

    #[test]
    fn a_thousand_instances_of_fifty_parts_are_planned_in_a_frame() {
        let start = std::time::Instant::now();
        let doc = grid(50, 20);
        let built = start.elapsed();
        assert_eq!(doc.bodies.len(), 1000);
        // Fifty meshes, however many instances: one per part.
        let infos = body_infos(&doc);
        let keys: HashSet<u64> = infos.iter().map(|i| i.key).collect();
        assert_eq!(keys.len(), 50);
        // Nothing was tessellated to get here.
        assert!(doc.bodies.iter().all(|b| !b.is_tessellated()));

        let scene = doc.visible_body_bounds();
        let mut camera = Camera::default();
        camera.fit(&scene, 1.5);
        let view = view(&camera, &scene);
        let start = std::time::Instant::now();
        let mut drawn = 0;
        for _ in 0..10 {
            let _ = doc.visible_body_bounds();
            drawn = infos
                .iter()
                .zip(&doc.placed)
                .filter(|(info, placed)| view.detail(info, &placed.shown).is_drawn())
                .count();
        }
        let per_frame = start.elapsed() / 10;
        assert_eq!(drawn, 1000);
        println!("1000 instances: built in {built:?}, planned in {per_frame:?} a frame");
        // Well inside a 16 ms frame, even in an unoptimised build.
        assert!(per_frame.as_millis() < 8, "{per_frame:?}");
    }
}
