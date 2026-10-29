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
    /// x = pick object id + 1 (0: not pickable); y = its depth band in the pick pass
    /// ([`Band`]); z = the bits of its opacity (an `f32`, 1 opaque).
    pub pick: [u32; 4],
}

/// Where an object's depths go in the pick pass. To pick through what is see-through
/// (SolidWorks selects the opaque geometry behind a transparent component), the opaque
/// objects' depths are put in front of every see-through object's: each in its own half
/// of the depth range, in order within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum Band {
    /// Depth as it is: nothing is picked through.
    Whole = 0,
    /// The nearer half: an opaque object, picked before anything see-through.
    Front = 1,
    /// The farther half: a see-through object, picked where nothing opaque is.
    Back = 2,
}

/// Below this an object is see-through: drawn blended, after the opaque ones.
const OPAQUE: f32 = 0.999;

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
    /// The opaque objects, by mesh.
    pub batches: Vec<Batch>,
    /// The see-through objects, farthest first, to be drawn blended after the opaque
    /// ones (one after another of the same mesh are one batch). In the pick pass, empty:
    /// every object is in `batches`.
    pub transparent: Vec<Batch>,
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
/// for a mesh that is not uploaded: its objects are skipped). With `pick`, for the pick
/// pass: objects that can't be picked are left out, and `Some(true)` picks through
/// see-through objects (see [`Band`]). Without it, see-through objects are put in
/// [`Batches::transparent`], farthest first, and those not seen at all are left out.
pub(crate) fn batch(
    objects: &[ObjectDraw],
    bounds: impl Fn(MeshId) -> Option<Aabb>,
    view_proj: &DMat4,
    highlight: [f32; 3],
    pick: Option<bool>,
) -> Batches {
    let mut out = Batches::default();
    let mut kept: Vec<&ObjectDraw> = Vec::with_capacity(objects.len());
    // The see-through ones, with how near they are (reversed depth: larger is nearer).
    let mut clear: Vec<(f64, &ObjectDraw)> = Vec::new();
    for draw in objects {
        if pick.is_some() && draw.pick_object.is_none() {
            continue;
        }
        let see_through = draw.opacity < OPAQUE;
        if pick.is_none() && draw.opacity <= 0.0 {
            continue;
        }
        let Some(mesh_bounds) = bounds(draw.mesh) else {
            continue;
        };
        if !in_view(view_proj, &mesh_bounds, &draw.transform) {
            out.culled += 1;
        } else if see_through && pick.is_none() {
            let c = *view_proj
                * draw
                    .transform
                    .transform_point3(mesh_bounds.center())
                    .extend(1.0);
            clear.push((if c.w > 0.0 { c.z / c.w } else { 0.0 }, draw));
        } else {
            kept.push(draw);
        }
    }
    let through = pick == Some(true) && kept.iter().any(|d| d.opacity < OPAQUE);
    // By mesh, and in a mesh the ones with edges first. (Stable: otherwise in the
    // order given.)
    kept.sort_by_key(|d| (d.mesh, !d.show_edges));
    clear.sort_by(|a, b| a.0.total_cmp(&b.0));
    out.instances.reserve(kept.len() + clear.len());
    let instance = |draw: &ObjectDraw| {
        let band = match (through, draw.opacity < OPAQUE) {
            (false, _) => Band::Whole,
            (true, false) => Band::Front,
            (true, true) => Band::Back,
        };
        Instance {
            model: draw.transform.as_mat4().to_cols_array_2d(),
            tint: [
                highlight[0],
                highlight[1],
                highlight[2],
                draw.highlight.clamp(0.0, 1.0) * 0.55,
            ],
            pick: [
                draw.pick_object.map_or(0, crate::pick::encode_object),
                band as u32,
                draw.opacity.clamp(0.0, 1.0).to_bits(),
                0,
            ],
        }
    };
    let add = |batches: &mut Vec<Batch>, instances: &mut Vec<Instance>, draw: &ObjectDraw| {
        let at = instances.len() as u32;
        instances.push(instance(draw));
        match batches.last_mut() {
            Some(batch) if batch.mesh == draw.mesh && batch.edges.end == at => {
                batch.all.end = at + 1;
                if draw.show_edges {
                    batch.edges.end = at + 1;
                }
            }
            Some(batch) if batch.mesh == draw.mesh && !draw.show_edges => batch.all.end = at + 1,
            _ => batches.push(Batch {
                mesh: draw.mesh,
                all: at..at + 1,
                edges: if draw.show_edges { at..at + 1 } else { at..at },
            }),
        }
    };
    for draw in kept {
        add(&mut out.batches, &mut out.instances, draw);
    }
    for (_, draw) in clear {
        add(&mut out.transparent, &mut out.instances, draw);
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
            opacity: 1.0,
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
        let all = batch(&objects, bounds, &view_proj, [1.0, 0.5, 0.0], None);
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

        let pickable = batch(&objects, bounds, &view_proj, [0.0; 3], Some(true));
        assert_eq!(pickable.instances.len(), 3);
        assert_eq!(pickable.batches[0].all, 0..2);
        assert_eq!(pickable.batches[0].edges, 0..1);
    }

    #[test]
    fn see_through_objects_come_after_the_opaque_ones_farthest_first() {
        for view_proj in views() {
            let (a, b) = (MeshId::test(1), MeshId::test(2));
            let eye = view_proj.inverse().project_point3(DVec3::new(0.0, 0.0, 1.0));
            let towards = eye.normalize();
            let draw = |mesh, along: f64, opacity, pick| ObjectDraw {
                mesh,
                transform: DMat4::from_translation(towards * along),
                show_edges: true,
                highlight: 0.0,
                pick_object: Some(pick),
                opacity,
            };
            let objects = [
                draw(a, 0.0, 1.0, 0),
                draw(b, 10.0, 0.5, 1),
                draw(b, -10.0, 0.5, 2),
                draw(a, -5.0, 0.3, 3),
                draw(a, 3.0, 0.0, 4),
            ];
            let all = batch(&objects, |_| Some(unit()), &view_proj, [0.0; 3], None);
            assert_eq!(all.batches.len(), 1);
            // The farthest (-10, then -5) first; the one not seen at all is left out.
            let picks: Vec<u32> = all.instances.iter().map(|i| i.pick[0]).collect();
            assert_eq!(picks, [1, 3, 4, 2]);
            assert_eq!(
                all.transparent.iter().map(|b| b.mesh).collect::<Vec<_>>(),
                [b, a, b]
            );
            let bands: Vec<u32> = all.instances.iter().map(|i| i.pick[1]).collect();
            assert_eq!(bands, [0; 4]);
            assert_eq!(f32::from_bits(all.instances[1].pick[2]), 0.5);

            // Picking: every object in one list, each in its band.
            let pick = batch(&objects, |_| Some(unit()), &view_proj, [0.0; 3], Some(true));
            assert!(pick.transparent.is_empty());
            assert_eq!(pick.instances.len(), 5);
            let band = |object: u32| {
                pick.instances
                    .iter()
                    .find(|i| i.pick[0] == object + 1)
                    .unwrap()
                    .pick[1]
            };
            assert_eq!(band(0), Band::Front as u32);
            assert_eq!(band(1), Band::Back as u32);
            assert_eq!(band(4), Band::Back as u32);
            // Not picking through, or with nothing see-through: depth as it is.
            let nearest = batch(&objects, |_| Some(unit()), &view_proj, [0.0; 3], Some(false));
            assert!(nearest.instances.iter().all(|i| i.pick[1] == 0));
            let opaque = batch(&objects[..1], |_| Some(unit()), &view_proj, [0.0; 3], Some(true));
            assert_eq!(opaque.instances[0].pick[1], 0);
        }
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
                opacity: 1.0,
            })
            .collect();
        let start = std::time::Instant::now();
        let batches = batch(&objects, |_| Some(unit()), &view_proj, [0.0; 3], None);
        let took = start.elapsed();
        assert_eq!(batches.culled, 0);
        assert_eq!(batches.instances.len(), 1000);
        assert_eq!(batches.batches.len(), 50);
        assert!(batches.batches.iter().all(|b| b.all.len() == 20));
        // A small part of a 16 ms frame, even unoptimised.
        assert!(took.as_millis() < 8, "{took:?}");
    }
}
