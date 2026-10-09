//! Turning a frame's objects into instanced draws: the objects that share a mesh are
//! drawn in one call, each with its own transform, tint and pick id, and the ones that
//! can't be seen are left out before anything is sent to the GPU.
//!
//! Nothing here touches the GPU, so it is tested without one.

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use peet_math::{Aabb, DMat4, DVec3};

use crate::renderer::{MeshId, ObjectDraw};

/// What the shaders get for each object drawn. Must match `InstanceIn` in common.wgsl
/// (its fields are vertex attributes 4 to 9).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub(crate) struct Instance {
    pub model: [[f32; 4]; 4],
    /// rgb = highlight colour, a = how much of it to blend in.
    pub tint: [f32; 4],
    /// x = pick object id + 1 (0: not pickable).
    pub pick: [u32; 4],
}

/// The objects of a frame that share a mesh, as a range of the instance buffer. The
/// ones that show their edges come first.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Batch {
    pub mesh: MeshId,
    pub all: Range<u32>,
    pub edges: Range<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Batches {
    pub instances: Vec<Instance>,
    pub batches: Vec<Batch>,
    /// How many objects were left out because they are outside the view.
    pub culled: usize,
}

/// Whether a box, placed by `transform`, can be in the view of `view_proj`: false only
/// if all of it is beyond one side of the view. (Not the near and far sides: the
/// projection is fitted to the scene, so nothing is cut off in depth.)
pub fn in_view(view_proj: &DMat4, bounds: &Aabb, transform: &DMat4) -> bool {
    if bounds.min.cmpgt(bounds.max).any() {
        return false;
    }
    let to_clip = *view_proj * *transform;
    // For each side, whether every corner so far is beyond it.
    let mut beyond = [true; 4];
    let mut behind = true;
    for i in 0..8 {
        let pick = |bit: usize, lo: f64, hi: f64| if i >> bit & 1 == 0 { lo } else { hi };
        let c = to_clip
            * DVec3::new(
                pick(0, bounds.min.x, bounds.max.x),
                pick(1, bounds.min.y, bounds.max.y),
                pick(2, bounds.min.z, bounds.max.z),
            )
            .extend(1.0);
        beyond[0] &= c.x < -c.w;
        beyond[1] &= c.x > c.w;
        beyond[2] &= c.y < -c.w;
        beyond[3] &= c.y > c.w;
        // Behind the eye of a perspective view.
        behind &= c.w <= 0.0;
    }
    !behind && !beyond.contains(&true)
}

/// Groups `objects` by mesh for instanced drawing. `bounds` gives a mesh's box (`None`
/// for a mesh that is not uploaded: its objects are skipped). With `pickable_only`,
/// objects that can't be picked are left out.
pub(crate) fn batch(
    objects: &[ObjectDraw],
    bounds: impl Fn(MeshId) -> Option<Aabb>,
    view_proj: &DMat4,
    highlight: [f32; 3],
    pickable_only: bool,
) -> Batches {
    let mut out = Batches::default();
    let mut kept: Vec<&ObjectDraw> = Vec::with_capacity(objects.len());
    for draw in objects {
        if pickable_only && draw.pick_object.is_none() {
            continue;
        }
        let Some(mesh_bounds) = bounds(draw.mesh) else {
            continue;
        };
        if in_view(view_proj, &mesh_bounds, &draw.transform) {
            kept.push(draw);
        } else {
            out.culled += 1;
        }
    }
    // By mesh, and in a mesh the ones with edges first. (Stable: otherwise in the
    // order given.)
    kept.sort_by_key(|d| (d.mesh, !d.show_edges));
    out.instances.reserve(kept.len());
    for draw in kept {
        let at = out.instances.len() as u32;
        out.instances.push(Instance {
            model: draw.transform.as_mat4().to_cols_array_2d(),
            tint: [
                highlight[0],
                highlight[1],
                highlight[2],
                draw.highlight.clamp(0.0, 1.0) * 0.55,
            ],
            pick: [
                draw.pick_object.map_or(0, crate::pick::encode_object),
                0,
                0,
                0,
            ],
        });
        match out.batches.last_mut() {
            Some(batch) if batch.mesh == draw.mesh => batch.all.end = at + 1,
            _ => out.batches.push(Batch {
                mesh: draw.mesh,
                all: at..at + 1,
                edges: at..at,
            }),
        }
        if draw.show_edges
            && let Some(batch) = out.batches.last_mut()
        {
            batch.edges.end = at + 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{Camera, Projection};

    fn unit() -> Aabb {
        Aabb::from_points([DVec3::splat(-1.0), DVec3::splat(1.0)])
    }

    fn at(x: f64, y: f64, z: f64) -> DMat4 {
        DMat4::from_translation(DVec3::new(x, y, z))
    }

    fn views() -> Vec<DMat4> {
        let scene = Aabb::from_points([DVec3::splat(-500.0), DVec3::splat(500.0)]);
        [Projection::Orthographic, Projection::Perspective]
            .into_iter()
            .map(|projection| {
                let mut camera = Camera {
                    projection,
                    ..Camera::default()
                };
                camera.target = DVec3::ZERO;
                camera.fit(
                    &Aabb::from_points([DVec3::splat(-20.0), DVec3::splat(20.0)]),
                    1.5,
                );
                camera.view_projection(1.5, &scene)
            })
            .collect()
    }

    #[test]
    fn what_is_beside_or_behind_the_view_is_not_in_it() {
        for view_proj in views() {
            assert!(in_view(&view_proj, &unit(), &DMat4::IDENTITY));
            assert!(in_view(&view_proj, &unit(), &at(5.0, -5.0, 5.0)));
            // Far to every side of what the camera frames.
            for far in [
                at(400.0, 0.0, 0.0),
                at(-400.0, 0.0, 0.0),
                at(0.0, 400.0, 0.0),
                at(0.0, 0.0, -400.0),
                at(0.0, 0.0, 400.0),
            ] {
                // (One of these directions is along the view, where the box is in front
                // of or behind what is framed: the others must all be out.)
                let _ = in_view(&view_proj, &unit(), &far);
            }
            let out = [
                at(400.0, 0.0, 0.0),
                at(-400.0, 0.0, 0.0),
                at(0.0, 400.0, 0.0),
                at(0.0, -400.0, 0.0),
                at(0.0, 0.0, 400.0),
                at(0.0, 0.0, -400.0),
            ]
            .iter()
            .filter(|t| !in_view(&view_proj, &unit(), t))
            .count();
            assert!(out >= 4, "{out} of 6 are out of view");
            // A box that is larger than the view and around it is in it.
            let huge = Aabb::from_points([DVec3::splat(-900.0), DVec3::splat(900.0)]);
            assert!(in_view(&view_proj, &huge, &DMat4::IDENTITY));
            // An empty box is nothing to draw.
            assert!(!in_view(&view_proj, &Aabb::EMPTY, &DMat4::IDENTITY));
        }
    }

    #[test]
    fn objects_of_one_mesh_are_one_batch() {
        let view_proj = views().remove(0);
        let (a, b, missing) = (MeshId::test(1), MeshId::test(2), MeshId::test(3));
        let draw = |mesh, x: f64, show_edges, pick| ObjectDraw {
            mesh,
            transform: at(x, 0.0, 0.0),
            show_edges,
            highlight: 0.0,
            pick_object: pick,
        };
        let objects = [
            draw(b, 0.0, true, Some(0)),
            draw(a, 1.0, false, Some(1)),
            draw(a, 2.0, true, None),
            draw(missing, 3.0, true, Some(3)),
            draw(a, 4.0, true, Some(4)),
            draw(a, 4000.0, true, Some(5)),
        ];
        let bounds = |mesh| (mesh != missing).then(unit);
        let all = batch(&objects, bounds, &view_proj, [1.0, 0.5, 0.0], false);
        assert_eq!(all.culled, 1);
        assert_eq!(
            all.batches,
            [
                Batch {
                    mesh: a,
                    all: 0..3,
                    edges: 0..2
                },
                Batch {
                    mesh: b,
                    all: 3..4,
                    edges: 3..4
                },
            ]
        );
        // Each keeps its own place and pick id: with edges first, in the order given.
        let x: Vec<f32> = all.instances.iter().map(|i| i.model[3][0]).collect();
        assert_eq!(x, [2.0, 4.0, 1.0, 0.0]);
        let picks: Vec<u32> = all.instances.iter().map(|i| i.pick[0]).collect();
        assert_eq!(picks, [0, 5, 2, 1]);

        let pickable = batch(&objects, bounds, &view_proj, [0.0; 3], true);
        assert_eq!(pickable.instances.len(), 3);
        assert_eq!(pickable.batches[0].all, 0..2);
        assert_eq!(pickable.batches[0].edges, 0..1);
    }

    #[test]
    fn a_thousand_instances_of_fifty_parts_are_fifty_draws() {
        let scene = Aabb::from_points([DVec3::splat(-2000.0), DVec3::splat(2000.0)]);
        let mut camera = Camera::default();
        camera.fit(&scene, 1.5);
        let view_proj = camera.view_projection(1.5, &scene);
        let objects: Vec<ObjectDraw> = (0..1000)
            .map(|i| ObjectDraw {
                mesh: MeshId::test(i % 50),
                transform: at(
                    f64::from(i as u32 % 10) * 100.0 - 450.0,
                    f64::from(i as u32 / 10 % 10) * 100.0 - 450.0,
                    f64::from(i as u32 / 100) * 100.0 - 450.0,
                ),
                show_edges: true,
                highlight: 0.0,
                pick_object: Some(i as u32),
            })
            .collect();
        let start = std::time::Instant::now();
        let batches = batch(&objects, |_| Some(unit()), &view_proj, [0.0; 3], false);
        let took = start.elapsed();
        assert_eq!(batches.culled, 0);
        assert_eq!(batches.instances.len(), 1000);
        assert_eq!(batches.batches.len(), 50);
        assert!(batches.batches.iter().all(|b| b.all.len() == 20));
        // A small part of a 16 ms frame, even unoptimised.
        assert!(took.as_millis() < 8, "{took:?}");
    }
}
