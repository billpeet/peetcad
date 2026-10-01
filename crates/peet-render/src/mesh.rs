//! CPU-side mesh data handed to the renderer.
//!
//! The renderer only ever sees triangles and line segments in `f32`. Tessellating model
//! geometry into these (from the kernel's `f64` B-rep) happens upstream.

use bytemuck::{Pod, Zeroable};
use peet_math::{Aabb, DVec3};

/// A shaded triangle mesh with optional edge lines (for the "shaded with edges" display mode).
#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
    /// Line segments as pairs of points, drawn in the edge colour on top of the shading.
    pub edges: Vec<[f32; 3]>,
    /// Pick id of each vertex (one per entry in `vertices`), reported by GPU picking for
    /// the face under the cursor. Give each face its own vertices and id. Empty means
    /// every vertex has id 0.
    pub pick_ids: Vec<u32>,
    /// Pick id of each edge segment (one per pair of points in `edges`). Empty means 0.
    pub edge_pick_ids: Vec<u32>,
}

impl MeshData {
    pub fn bounds(&self) -> Aabb {
        Aabb::from_points(
            self.vertices
                .iter()
                .map(|v| DVec3::from(v.position.map(f64::from))),
        )
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// One pick id per vertex, as uploaded to the GPU (zeros if `pick_ids` is unusable).
    pub(crate) fn vertex_pick_ids(&self) -> Vec<u32> {
        if self.pick_ids.len() == self.vertices.len() {
            return self.pick_ids.clone();
        }
        if !self.pick_ids.is_empty() {
            log::warn!(
                "mesh has {} pick ids for {} vertices; using id 0",
                self.pick_ids.len(),
                self.vertices.len()
            );
        }
        vec![0; self.vertices.len()]
    }

    /// One pick id per edge vertex (each segment's id twice), as uploaded to the GPU.
    pub(crate) fn edge_vertex_pick_ids(&self) -> Vec<u32> {
        let segments = self.edges.len() / 2;
        if self.edge_pick_ids.len() != segments && !self.edge_pick_ids.is_empty() {
            log::warn!(
                "mesh has {} edge pick ids for {segments} edge segments; using id 0",
                self.edge_pick_ids.len()
            );
        }
        let ids_ok = self.edge_pick_ids.len() == segments;
        (0..self.edges.len())
            .map(|i| if ids_ok { self.edge_pick_ids[i / 2] } else { 0 })
            .collect()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct MeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// sRGB colour with alpha, 8 bits per channel.
    pub color: [u8; 4],
}

/// A position with a colour: used for lines and translucent overlays.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct ColorVertex {
    pub position: [f32; 3],
    /// sRGB colour with alpha, 8 bits per channel.
    pub color: [u8; 4],
}

impl ColorVertex {
    pub fn new(position: DVec3, color: [u8; 4]) -> Self {
        Self {
            position: position.as_vec3().to_array(),
            color,
        }
    }
}

/// Immediate-mode overlay geometry, rebuilt every frame (reference planes, gizmos, previews).
#[derive(Clone, Debug, Default)]
pub struct Overlay {
    /// Translucent triangles, three vertices each.
    pub triangles: Vec<ColorVertex>,
    /// Line segments, two vertices each.
    pub lines: Vec<ColorVertex>,
}

impl Overlay {
    pub fn clear(&mut self) {
        self.triangles.clear();
        self.lines.clear();
    }

    pub fn line(&mut self, a: DVec3, b: DVec3, color: [u8; 4]) {
        self.lines.push(ColorVertex::new(a, color));
        self.lines.push(ColorVertex::new(b, color));
    }

    /// A filled quad from four corners in order, plus its outline.
    pub fn quad(&mut self, corners: [DVec3; 4], fill: [u8; 4], outline: [u8; 4]) {
        for i in [0, 1, 2, 0, 2, 3] {
            self.triangles.push(ColorVertex::new(corners[i], fill));
        }
        for i in 0..4 {
            self.line(corners[i], corners[(i + 1) % 4], outline);
        }
    }
}

pub mod demo {
    //! Procedural demo geometry, used until the modelling kernel exists.

    use super::{MeshData, MeshVertex};
    use peet_math::{DVec2, DVec3};

    /// One step of a sheet metal cross-section, walked along the sheet's mid-plane.
    #[derive(Clone, Copy, Debug)]
    pub enum ProfileStep {
        /// A flat run of the given length (mm).
        Straight(f64),
        /// A bend turning left by `angle` radians (negative turns right) with the given inner radius.
        Bend { angle: f64, inner_radius: f64 },
    }

    /// Sweeps a sheet metal cross-section (in the XZ plane) along Y to make a solid mesh.
    ///
    /// The profile starts at `start` heading in direction `heading` (radians from +X).
    pub fn sheet_profile(
        steps: &[ProfileStep],
        start: DVec2,
        heading: f64,
        thickness: f64,
        width: f64,
        color: [u8; 4],
    ) -> MeshData {
        const SEGMENTS_PER_RADIAN: f64 = 12.0;
        let half_t = thickness * 0.5;

        // Walk the mid-line, recording points, left normals and whether each point starts
        // or ends a bend (those get tangent edge lines, as in other CAD tools).
        let mut points = vec![start];
        let mut normals = vec![left_normal(heading)];
        let mut edge_at = vec![true];
        let mut pos = start;
        let mut dir = heading;
        for step in steps {
            match *step {
                ProfileStep::Straight(len) => {
                    pos += DVec2::from_angle(dir) * len;
                    points.push(pos);
                    normals.push(left_normal(dir));
                    edge_at.push(true);
                }
                ProfileStep::Bend {
                    angle,
                    inner_radius,
                } => {
                    let r = inner_radius + half_t;
                    let side = angle.signum();
                    let center = pos + left_normal(dir) * (r * side);
                    let n = ((angle.abs() * SEGMENTS_PER_RADIAN).ceil() as usize).max(2);
                    for i in 1..=n {
                        let a = dir + angle * (i as f64 / n as f64);
                        let p = center - left_normal(a) * (r * side);
                        points.push(p);
                        normals.push(left_normal(a));
                        edge_at.push(i == n);
                    }
                    pos = *points.last().expect("non-empty");
                    dir += angle;
                }
            }
        }
        *edge_at.last_mut().expect("non-empty") = true;

        let to3 = |p: DVec2, y: f64| DVec3::new(p.x, y, p.y);
        let outer: Vec<DVec2> = points
            .iter()
            .zip(&normals)
            .map(|(p, n)| *p + *n * half_t)
            .collect();
        let inner: Vec<DVec2> = points
            .iter()
            .zip(&normals)
            .map(|(p, n)| *p - *n * half_t)
            .collect();

        let mut mesh = MeshData::default();
        let push = |mesh: &mut MeshData, p: DVec3, n: DVec3| -> u32 {
            mesh.vertices.push(MeshVertex {
                position: p.as_vec3().to_array(),
                normal: n.as_vec3().to_array(),
                color,
            });
            (mesh.vertices.len() - 1) as u32
        };

        // The two long surfaces of the sheet, smooth-shaded so bends look round.
        for (path, sign) in [(&outer, 1.0), (&inner, -1.0)] {
            let base = mesh.vertices.len() as u32;
            for (p, n) in path.iter().zip(&normals) {
                let n3 = to3(*n * sign, 0.0);
                push(&mut mesh, to3(*p, 0.0), n3);
                push(&mut mesh, to3(*p, width), n3);
            }
            for i in 0..path.len() as u32 - 1 {
                let (a, b, c, d) = (
                    base + 2 * i,
                    base + 2 * i + 1,
                    base + 2 * i + 2,
                    base + 2 * i + 3,
                );
                mesh.indices.extend_from_slice(&[a, c, d, a, d, b]);
            }
        }

        // End caps of the strip (the sheet edges at both ends of the profile).
        for (i, dir_sign) in [(0usize, -1.0), (points.len() - 1, 1.0)] {
            let tangent = DVec2::new(normals[i].y, -normals[i].x) * dir_sign;
            let n3 = to3(tangent, 0.0);
            let quad = [
                to3(outer[i], 0.0),
                to3(inner[i], 0.0),
                to3(inner[i], width),
                to3(outer[i], width),
            ];
            let idx: Vec<u32> = quad.iter().map(|p| push(&mut mesh, *p, n3)).collect();
            mesh.indices
                .extend_from_slice(&[idx[0], idx[1], idx[2], idx[0], idx[2], idx[3]]);
        }

        // Side faces at y = 0 and y = width: a strip between the outer and inner paths.
        for (y, ny) in [(0.0, -1.0), (width, 1.0)] {
            let base = mesh.vertices.len() as u32;
            for (o, i) in outer.iter().zip(&inner) {
                push(&mut mesh, to3(*o, y), DVec3::Y * ny);
                push(&mut mesh, to3(*i, y), DVec3::Y * ny);
            }
            for k in 0..outer.len() as u32 - 1 {
                let (a, b, c, d) = (
                    base + 2 * k,
                    base + 2 * k + 1,
                    base + 2 * k + 2,
                    base + 2 * k + 3,
                );
                mesh.indices.extend_from_slice(&[a, b, d, a, d, c]);
            }
        }

        // Edge lines: side outlines, plus lines along Y at profile ends and bend tangents.
        let mut edge = |a: DVec3, b: DVec3| {
            mesh.edges.push(a.as_vec3().to_array());
            mesh.edges.push(b.as_vec3().to_array());
        };
        for y in [0.0, width] {
            for path in [&outer, &inner] {
                for w in path.windows(2) {
                    edge(to3(w[0], y), to3(w[1], y));
                }
            }
            edge(to3(outer[0], y), to3(inner[0], y));
            let last = points.len() - 1;
            edge(to3(outer[last], y), to3(inner[last], y));
        }
        for (k, is_edge) in edge_at.iter().enumerate() {
            if *is_edge {
                for path in [&outer, &inner] {
                    edge(to3(path[k], 0.0), to3(path[k], width));
                }
            }
        }
        mesh
    }

    fn left_normal(angle: f64) -> DVec2 {
        DVec2::new(-angle.sin(), angle.cos())
    }

    /// A small U-channel bracket: two 90° bends, 2 mm sheet, 2 mm inner radius.
    pub fn u_channel() -> MeshData {
        use std::f64::consts::FRAC_PI_2;
        let t = 2.0;
        let bend = ProfileStep::Bend {
            angle: FRAC_PI_2,
            inner_radius: 2.0,
        };
        sheet_profile(
            &[
                ProfileStep::Straight(36.0),
                bend,
                ProfileStep::Straight(70.0),
                bend,
                ProfileStep::Straight(36.0),
            ],
            // Mid-line start: legs at x = ±38, and the base mid-plane at z = t/2 so the
            // underside rests on the XY plane (36 leg + 3 mid bend radius + 1 = 40).
            DVec2::new(-38.0, 40.0),
            -FRAC_PI_2,
            t,
            50.0,
            [176, 184, 196, 255],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u_channel_is_well_formed() {
        let mesh = demo::u_channel();
        assert!(mesh.triangle_count() > 50);
        assert_eq!(mesh.indices.len() % 3, 0);
        assert!(
            mesh.indices
                .iter()
                .all(|&i| (i as usize) < mesh.vertices.len())
        );
        assert_eq!(mesh.edges.len() % 2, 0);
        let b = mesh.bounds();
        // Mid-line spans 70 mm between bends, plus two bend radii and the sheet thickness.
        let expected_x = 70.0 + 2.0 * (2.0 + 1.0) + 2.0;
        assert!((b.size().x - expected_x).abs() < 1e-3, "{:?}", b.size());
        assert!((b.size().y - 50.0).abs() < 1e-4);
        assert!(
            b.min.z.abs() < 1e-4,
            "channel sits on the XY plane: {:?}",
            b.min
        );
        for v in &mesh.vertices {
            let n = glam::Vec3::from(v.normal);
            assert!((n.length() - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn pick_ids_expand_per_vertex() {
        let mut mesh = demo::u_channel();
        assert_eq!(mesh.vertex_pick_ids(), vec![0; mesh.vertices.len()]);
        assert_eq!(mesh.edge_vertex_pick_ids(), vec![0; mesh.edges.len()]);

        mesh.pick_ids = (0..mesh.vertices.len() as u32).collect();
        assert_eq!(mesh.vertex_pick_ids(), mesh.pick_ids);
        mesh.edge_pick_ids = (10..10 + mesh.edges.len() as u32 / 2).collect();
        let per_vertex = mesh.edge_vertex_pick_ids();
        assert_eq!(per_vertex.len(), mesh.edges.len());
        assert_eq!(&per_vertex[..4], &[10, 10, 11, 11]);

        // Mismatched lengths fall back to id 0 rather than misattributing.
        mesh.pick_ids.pop();
        mesh.edge_pick_ids.pop();
        assert!(mesh.vertex_pick_ids().iter().all(|&i| i == 0));
        assert!(mesh.edge_vertex_pick_ids().iter().all(|&i| i == 0));
    }
}
