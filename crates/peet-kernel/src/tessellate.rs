//! Tessellation for display: triangle meshes per face, polylines per edge, and silhouette
//! lines of curved faces.
//!
//! **Method.** Every edge is sampled once ([`sample_edges`]): lines by their end points,
//! circles and ellipses in equal parameter steps that keep the chord error under the
//! tolerance and each step under [`MAX_ANGLE`]. The first and last samples are the edge's
//! vertices, bit for bit, and every face bounded by an edge reuses exactly these points, so
//! neighbouring faces share their boundary vertices and the mesh has no cracks.
//!
//! Each face's loops are mapped to its parameter space, scaled to millimetres (plane: local
//! `(u, v)`; cylinder: `(r θ, v)` with `θ` unwrapped continuously along each loop, and
//! holes shifted by whole turns into the outer loop's range). Faces whose outward normal
//! opposes the surface's natural normal are mirrored in `u` so their loops run
//! counter-clockwise. The polygon is triangulated (hole bridging + ear clipping), improved
//! with Delaunay flips, and on cylinders every interior edge spanning more than the angle
//! limit is split, so no triangle spans more than [`MAX_ANGLE`] of arc or deviates from the
//! surface by more than the tolerance.

mod silhouette;
mod triangulate;

use std::f64::consts::TAU;

use peet_math::{DVec2, DVec3, tolerance};

use crate::geom::{Curve3, Surface, unwrap_angle};
use crate::{EdgeId, FaceId, KernelError, Solid};

pub use silhouette::Silhouettes;

/// Largest angle a curved edge segment or a cylinder facet may span (10°).
pub const MAX_ANGLE: f64 = 10.0 * std::f64::consts::PI / 180.0;

/// No curved edge is cut into more than this many segments per full turn, however fine
/// the tolerance (keeps meshes bounded for absurd tolerances).
const MAX_SEGMENTS_PER_TURN: f64 = 2048.0;

/// Relative slack on the facet width limit of curved faces.
const FACET_SLACK: f64 = 1e-6;

/// At most this many interior points are added per face when splitting long facets.
const MAX_STEINER_POINTS: usize = 200_000;

/// Triangles of one face. Vertices are not shared between faces, so per-face data (ids,
/// highlight) can be attached per vertex.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FaceMesh {
    pub face: FaceId,
    pub positions: Vec<DVec3>,
    /// Outward unit normals (smooth across a cylinder, constant on a plane).
    pub normals: Vec<DVec3>,
    /// Counter-clockwise seen from outside.
    pub triangles: Vec<[u32; 3]>,
}

impl FaceMesh {
    /// Total area of the triangles (mm²).
    pub fn area(&self) -> f64 {
        self.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| self.positions[i as usize]);
                0.5 * (b - a).cross(c - a).length()
            })
            .sum()
    }
}

/// Points along one edge, from its start vertex to its end vertex.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EdgePolyline {
    pub edge: EdgeId,
    pub points: Vec<DVec3>,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SolidMesh {
    pub faces: Vec<FaceMesh>,
    pub edges: Vec<EdgePolyline>,
}

impl SolidMesh {
    /// All triangles as corner positions, counter-clockwise seen from outside (for export).
    pub fn triangles(&self) -> Vec<[DVec3; 3]> {
        self.faces
            .iter()
            .flat_map(|f| {
                f.triangles
                    .iter()
                    .map(|t| t.map(|i| f.positions[i as usize]))
            })
            .collect()
    }
}

/// How the viewer looks at the solid, for silhouettes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum View {
    /// Looking along `dir`.
    Orthographic { dir: DVec3 },
    /// Looking from `eye`.
    Perspective { eye: DVec3 },
}

/// Tessellates every face and edge. `tolerance` is the maximum chord deviation (mm);
/// curved faces also keep facets under about 10° of arc so shading looks smooth. Edge
/// polylines use exactly the same points as the faces' boundaries, so there are no cracks.
pub fn tessellate(solid: &Solid, tolerance: f64) -> Result<SolidMesh, KernelError> {
    if !(tolerance.is_finite() && tolerance > 0.0) {
        return Err(KernelError::InvalidInput(format!(
            "tessellation tolerance must be a positive number of mm (got {tolerance})"
        )));
    }
    let samples = sample_edges(solid, tolerance);
    let mut faces = Vec::with_capacity(solid.faces.len());
    for face in solid.face_ids() {
        faces.push(tessellate_face(solid, face, &samples, tolerance)?);
    }
    let edges = samples
        .into_iter()
        .enumerate()
        .map(|(i, s)| EdgePolyline {
            edge: EdgeId(i as u32),
            points: s.points,
        })
        .collect();
    Ok(SolidMesh { faces, edges })
}

/// Silhouette lines of curved faces (where the surface turns away from the viewer),
/// clipped to the faces, as line segments.
pub fn silhouettes(solid: &Solid, view: View) -> Vec<[DVec3; 2]> {
    Silhouettes::new(solid).lines(view)
}

/// Sample points of one edge, from its start vertex to its end vertex.
#[derive(Clone, Debug, Default)]
pub(crate) struct EdgeSamples {
    pub points: Vec<DVec3>,
}

/// Largest parameter step on `curve` keeping the chord error under `tolerance` and the arc
/// under [`MAX_ANGLE`].
fn max_step(curve: &Curve3, tolerance: f64) -> f64 {
    let radius = match curve {
        Curve3::Line(_) => return f64::INFINITY,
        Curve3::Circle(c) => c.radius,
        // Largest radius of curvature of an ellipse: a² / b (at the minor axis ends).
        Curve3::Ellipse(e) => e.major * e.major / e.minor.max(tolerance::LINEAR),
    };
    // A chord spanning angle φ deviates r (1 − cos(φ/2)) from the arc.
    let chord = if tolerance >= radius {
        std::f64::consts::PI
    } else {
        2.0 * (1.0 - tolerance / radius).acos()
    };
    chord.clamp(TAU / MAX_SEGMENTS_PER_TURN, MAX_ANGLE)
}

/// Samples every edge once, within `tolerance`.
pub(crate) fn sample_edges(solid: &Solid, tolerance: f64) -> Vec<EdgeSamples> {
    solid
        .edges
        .iter()
        .map(|e| {
            let span = e.t1 - e.t0;
            let n = if span.is_finite() && span > 0.0 {
                (span / max_step(&e.curve, tolerance)).ceil().max(1.0) as usize
            } else {
                1
            };
            let mut points = Vec::with_capacity(n + 1);
            for k in 0..=n {
                let t = e.t0 + span * k as f64 / n as f64;
                points.push(if k == 0 {
                    solid.vertex(e.start).point
                } else if k == n {
                    solid.vertex(e.end).point
                } else {
                    e.curve.point(t)
                });
            }
            EdgeSamples { points }
        })
        .collect()
}

/// A face's boundary in its scaled parameter space.
pub(crate) struct FaceBoundary {
    /// Scaled parameters (plane: `(u, v)`; cylinder: `(r θ, v)`), mirrored in `u` when
    /// `mirrored` is set.
    pub param: Vec<DVec2>,
    /// The boundary points in model space (exactly the edge samples).
    pub positions: Vec<DVec3>,
    /// Index ranges of the loops (outer first).
    pub loops: Vec<std::ops::Range<usize>>,
    /// The `u` axis is mirrored (faces whose outward normal opposes the surface normal).
    pub mirrored: bool,
}

/// Maps a face's loops to its parameter space (see [`FaceBoundary`]).
pub(crate) fn face_boundary(solid: &Solid, face: FaceId, samples: &[EdgeSamples]) -> FaceBoundary {
    let f = solid.face(face);
    let mirror = if f.reversed { -1.0 } else { 1.0 };
    let mut out = FaceBoundary {
        param: Vec::new(),
        positions: Vec::new(),
        loops: Vec::with_capacity(f.loops.len()),
        mirrored: f.reversed,
    };
    for &l in &f.loops {
        let start = out.param.len();
        let mut theta: Option<f64> = None;
        for c in solid.loop_coedges(l) {
            let co = solid.coedge(c);
            let s = &samples[co.edge.index()];
            let n = s.points.len();
            // Every point but the last (the next coedge's first).
            for k in 0..n - 1 {
                let k = if co.reversed { n - 1 - k } else { k };
                let p = s.points[k];
                let uv = match &f.surface {
                    Surface::Plane(pl) => pl.to_plane_coords(p),
                    Surface::Cylinder(cy) => {
                        let q = cy.frame.to_local(p);
                        let raw = q.y.atan2(q.x);
                        let th = theta.map_or(raw, |prev| unwrap_angle(raw, prev));
                        theta = Some(th);
                        DVec2::new(th * cy.radius, q.z)
                    }
                };
                out.param.push(DVec2::new(uv.x * mirror, uv.y));
                out.positions.push(p);
            }
        }
        out.loops.push(start..out.param.len());
    }
    // Shift cylinder holes by whole turns into the outer loop's angular range.
    if let Surface::Cylinder(cy) = &f.surface
        && out.loops.len() > 1
    {
        let turn = TAU * cy.radius;
        let mid = |r: &std::ops::Range<usize>, param: &[DVec2]| {
            let (lo, hi) = param[r.clone()]
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
                    (lo.min(p.x), hi.max(p.x))
                });
            0.5 * (lo + hi)
        };
        let outer_mid = mid(&out.loops[0], &out.param);
        for r in out.loops[1..].iter().cloned() {
            let shift = ((outer_mid - mid(&r, &out.param)) / turn).round() * turn;
            if shift != 0.0 {
                for p in &mut out.param[r] {
                    p.x += shift;
                }
            }
        }
    }
    out
}

fn signed_area(points: &[DVec2]) -> f64 {
    let n = points.len();
    (0..n)
        .map(|i| points[i].perp_dot(points[(i + 1) % n]))
        .sum::<f64>()
        * 0.5
}

fn tessellate_face(
    solid: &Solid,
    face: FaceId,
    samples: &[EdgeSamples],
    tolerance: f64,
) -> Result<FaceMesh, KernelError> {
    let f = solid.face(face);
    if f.loops.is_empty() {
        return Err(KernelError::InvalidResult(format!(
            "face {} has no boundary to tessellate",
            face.0
        )));
    }
    let boundary = face_boundary(solid, face, samples);
    let mut param = boundary.param;
    let mut positions = boundary.positions;
    let loops = boundary.loops;
    // An invalid solid could hand us a clockwise outer loop: reverse the winding to
    // triangulate, and flip the triangles back afterwards.
    let flip = signed_area(&param[loops[0].clone()]) < 0.0;
    if flip {
        for p in &mut param {
            p.x = -p.x;
        }
    }
    let tris = triangulate::triangulate(&triangulate::Polygon {
        points: &param,
        loops: &loops,
    });
    let mut mesh = triangulate::Mesh2::new(param, tris);
    let sign = if f.reversed { -1.0 } else { 1.0 };
    let normals = match &f.surface {
        Surface::Plane(pl) => {
            mesh.delaunay(|_, _| true);
            vec![pl.normal() * sign; mesh.points.len()]
        }
        Surface::Cylinder(cy) => {
            let r = cy.radius;
            // A little slack: boundary steps are often exactly the limit (an arc that
            // divides evenly), and rounding must not make those facets look too wide.
            let max_dx = (1.0 + FACET_SLACK)
                * r
                * max_step(
                    &Curve3::Circle(crate::geom::Circle3 {
                        frame: cy.frame,
                        radius: r,
                    }),
                    tolerance,
                );
            mesh.delaunay(|_, _| true);
            mesh.split_long_x(max_dx, MAX_STEINER_POINTS);
            mesh.delaunay(|a, b| (a.x - b.x).abs() <= max_dx);
            // Interior points are new: place them on the surface.
            let unmirror = if f.reversed != flip { -1.0 } else { 1.0 };
            let to_uv = |p: DVec2| DVec2::new(p.x * unmirror / r, p.y);
            for p in &mesh.points[positions.len()..] {
                positions.push(f.surface.point(to_uv(*p)));
            }
            mesh.points
                .iter()
                .map(|p| f.surface.normal(to_uv(*p)) * sign)
                .collect()
        }
    };
    let mut triangles = mesh.tris;
    if flip {
        for t in &mut triangles {
            t.swap(1, 2);
        }
    }
    Ok(FaceMesh {
        face,
        positions,
        normals,
        triangles,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::f64::consts::PI;

    use super::*;
    use crate::extrude::extrude;
    use crate::topo::test_shapes::cuboid;
    use crate::validate::{assert_valid, measure};
    use peet_math::Plane;
    use peet_sketch::Sketch;
    use peet_sketch::region::{Region, find_regions};
    use peet_sketch::shapes;

    /// A bracket-like plate: outline with a slot, several holes.
    pub(crate) fn bracket() -> Solid {
        let mut s = Sketch::new();
        shapes::rectangle(&mut s, DVec2::ZERO, DVec2::new(120.0, 60.0));
        for i in 0..6 {
            s.add_circle(DVec2::new(15.0 + 18.0 * f64::from(i), 15.0), 4.0);
        }
        shapes::slot(&mut s, DVec2::new(20.0, 42.0), DVec2::new(60.0, 42.0), 6.0);
        s.add_circle(DVec2::new(95.0, 42.0), 10.0);
        let plate: Vec<Region> = find_regions(&s)
            .regions
            .into_iter()
            .filter(|r| r.holes.len() == 8)
            .collect();
        assert_eq!(plate.len(), 1);
        extrude(&Plane::TOP, &plate, 0.0, 3.0).unwrap()
    }

    fn plate_with_hole() -> Solid {
        let mut s = Sketch::new();
        shapes::rectangle(&mut s, DVec2::ZERO, DVec2::new(40.0, 20.0));
        s.add_circle(DVec2::new(10.0, 10.0), 3.0);
        let plate: Vec<Region> = find_regions(&s)
            .regions
            .into_iter()
            .filter(|r| r.holes.len() == 1)
            .collect();
        extrude(&Plane::TOP, &plate, 0.0, 2.0).unwrap()
    }

    fn cylinder() -> Solid {
        let mut s = Sketch::new();
        s.add_circle(DVec2::ZERO, 5.0);
        extrude(&Plane::TOP, &find_regions(&s).regions, 0.0, 10.0).unwrap()
    }

    fn bits(p: DVec3) -> [u64; 3] {
        [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]
    }

    /// Every undirected mesh edge (by exact positions) is used exactly twice, in opposite
    /// directions: the union of the face meshes is closed.
    fn assert_watertight(mesh: &SolidMesh) {
        type Key = ([u64; 3], [u64; 3]);
        let mut uses: HashMap<Key, Vec<(u32, [u32; 3])>> = HashMap::new();
        for f in &mesh.faces {
            for t in &f.triangles {
                for k in 0..3 {
                    let a = bits(f.positions[t[k] as usize]);
                    let b = bits(f.positions[t[(k + 1) % 3] as usize]);
                    uses.entry((a, b)).or_default().push((f.face.0, *t));
                }
            }
        }
        for (&(a, b), list) in &uses {
            assert_eq!(
                list.len(),
                1,
                "directed edge used by (face, triangle) {list:?}"
            );
            let back = uses.get(&(b, a)).map_or(0, Vec::len);
            assert_eq!(back, 1, "edge of (face, triangle) {list:?} has no opposite");
        }
    }

    /// Volume of the closed triangle mesh (divergence theorem).
    fn mesh_volume(mesh: &SolidMesh) -> f64 {
        mesh.triangles()
            .iter()
            .map(|[a, b, c]| a.dot(b.cross(*c)) / 6.0)
            .sum()
    }

    fn check_mesh(solid: &Solid, tol: f64) -> SolidMesh {
        assert_valid(solid);
        let mesh = tessellate(solid, tol).unwrap();
        assert_watertight(&mesh);
        for fm in &mesh.faces {
            let exact = measure::face_area(solid, fm.face);
            let area = fm.area();
            // Chords cut inside curves: the error is bounded by perimeter × tolerance.
            let perimeter: f64 = solid
                .face(fm.face)
                .loops
                .iter()
                .flat_map(|&l| solid.loop_coedges(l))
                .map(|c| {
                    let pts = &mesh.edges[solid.coedge(c).edge.index()].points;
                    pts.windows(2).map(|w| w[0].distance(w[1])).sum::<f64>()
                })
                .sum();
            assert!(
                (area - exact).abs() <= perimeter * tol + 1e-9,
                "face {}: mesh {area} vs exact {exact}",
                fm.face.0
            );
            assert_eq!(fm.positions.len(), fm.normals.len());
            for t in &fm.triangles {
                let [a, b, c] = t.map(|i| fm.positions[i as usize]);
                let n = (b - a).cross(c - a);
                for &i in t {
                    assert!(
                        n.dot(fm.normals[i as usize]) > 0.0,
                        "face {}: triangle against its normals",
                        fm.face.0
                    );
                }
            }
            for n in &fm.normals {
                assert!((n.length() - 1.0).abs() < 1e-12);
            }
        }
        let exact = measure::volume(solid);
        let v = mesh_volume(&mesh);
        assert!(
            v > 0.0 && (v - exact).abs() <= exact * 0.01,
            "{v} vs {exact}"
        );
        mesh
    }

    #[test]
    fn cuboid_mesh() {
        let s = cuboid(DVec3::ZERO, DVec3::new(1.0, 2.0, 3.0));
        let mesh = check_mesh(&s, 0.01);
        assert_eq!(
            mesh.faces.iter().map(|f| f.triangles.len()).sum::<usize>(),
            12
        );
        assert!((mesh_volume(&mesh) - 6.0).abs() < 1e-12);
        assert_eq!(mesh.edges.len(), 12);
    }

    #[test]
    fn polygonal_faces_are_exact() {
        let mut s = Sketch::new();
        let pts = [
            DVec2::ZERO,
            DVec2::new(80.0, 0.0),
            DVec2::new(80.0, 10.0),
            DVec2::new(10.0, 10.0),
            DVec2::new(10.0, 60.0),
            DVec2::new(0.0, 60.0),
        ];
        for i in 0..pts.len() {
            s.add_line(pts[i], pts[(i + 1) % pts.len()]);
        }
        let solid = extrude(&Plane::front(), &find_regions(&s).regions, 0.0, 3.0).unwrap();
        let mesh = check_mesh(&solid, 0.01);
        for fm in &mesh.faces {
            assert!((fm.area() - measure::face_area(&solid, fm.face)).abs() < 1e-9);
        }
        assert!((mesh_volume(&mesh) - 3900.0).abs() < 1e-9);
    }

    #[test]
    fn plate_with_hole_mesh() {
        check_mesh(&plate_with_hole(), 0.01);
        check_mesh(&plate_with_hole(), 0.5);
    }

    #[test]
    fn cylinder_mesh() {
        let solid = cylinder();
        let mesh = check_mesh(&solid, 0.01);
        let side = mesh
            .faces
            .iter()
            .find(|f| matches!(solid.face(f.face).surface, Surface::Cylinder(_)))
            .unwrap();
        // Lateral area 2π r h, within the chord error.
        assert!((side.area() - 2.0 * PI * 50.0).abs() < 2.0 * PI * 5.0 * 0.01 * 10.0);
        // Normals point radially outwards; facets stay under 10°.
        for (p, n) in side.positions.iter().zip(&side.normals) {
            let radial = DVec3::new(p.x, p.y, 0.0).normalize();
            assert!(radial.dot(*n) > 1.0 - 1e-9);
        }
        for t in &side.triangles {
            let angles: Vec<f64> = t
                .iter()
                .map(|&i| {
                    let p = side.positions[i as usize];
                    p.y.atan2(p.x)
                })
                .collect();
            for a in &angles {
                for b in &angles {
                    let d = unwrap_angle(*a, *b) - b;
                    assert!(d.abs() <= MAX_ANGLE + 1e-9, "facet spans {d} rad");
                }
            }
        }
    }

    #[test]
    fn slot_hole_and_tilted_plane() {
        let plane = Plane::from_origin_normal_x(
            DVec3::new(3.0, 1.0, -2.0),
            DVec3::new(1.0, 2.0, 0.5),
            DVec3::Z,
        )
        .unwrap();
        let mut s = Sketch::new();
        shapes::rectangle(&mut s, DVec2::ZERO, DVec2::new(30.0, 30.0));
        shapes::slot(&mut s, DVec2::new(8.0, 15.0), DVec2::new(22.0, 15.0), 3.0);
        let plate: Vec<Region> = find_regions(&s)
            .regions
            .into_iter()
            .filter(|r| r.holes.len() == 1)
            .collect();
        let solid = extrude(&plane, &plate, -2.5, 2.5).unwrap();
        check_mesh(&solid, 0.02);
    }

    #[test]
    fn ellipse_edge_and_cylinder_hole() {
        use crate::validate::test_solids;
        let solid = test_solids::oblique_cylinder(5.0, 10.0, 0.5);
        let mesh = check_mesh(&solid, 0.01);
        assert!((mesh_volume(&mesh) - PI * 250.0).abs() < 250.0 * PI * 0.005);

        let solid = test_solids::pocketed_cylinder();
        let mesh = check_mesh(&solid, 0.01);
        let exact = test_solids::pocketed_cylinder_volume();
        assert!((mesh_volume(&mesh) - exact).abs() < exact * 0.005);
        // Facets of the holed cylinder face stay within the angle limit, without an
        // explosion of interior points.
        let side = &mesh.faces[0];
        assert!(
            side.triangles.len() < 700,
            "{} triangles",
            side.triangles.len()
        );
        for t in &side.triangles {
            let a: Vec<f64> = t
                .iter()
                .map(|&i| side.positions[i as usize])
                .map(|p| p.y.atan2(p.x))
                .collect();
            for i in 0..3 {
                let d = unwrap_angle(a[i], a[(i + 1) % 3]) - a[(i + 1) % 3];
                assert!(d.abs() <= MAX_ANGLE + 1e-9);
            }
        }
    }

    #[test]
    fn bracket_mesh() {
        let solid = bracket();
        let mesh = check_mesh(&solid, 0.05);
        assert_eq!(mesh.faces.len(), solid.faces.len());
        let start = std::time::Instant::now();
        let runs = 5;
        for _ in 0..runs {
            tessellate(&solid, 0.05).unwrap();
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(runs);
        eprintln!(
            "bracket: {} faces, {} triangles, {ms:.2} ms per tessellation",
            solid.faces.len(),
            mesh.triangles().len()
        );
    }

    #[test]
    fn edge_polylines_match_vertices() {
        let solid = plate_with_hole();
        let mesh = tessellate(&solid, 0.05).unwrap();
        for (e, pl) in solid.edges.iter().zip(&mesh.edges) {
            assert_eq!(pl.points[0], solid.vertex(e.start).point);
            assert_eq!(*pl.points.last().unwrap(), solid.vertex(e.end).point);
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(48))]

        /// Random plates with a round hole and a slot, on random planes: always valid,
        /// watertight, outward-facing, with the right volume.
        #[test]
        fn random_plates(
            w in 30.0f64..200.0,
            h in 30.0f64..200.0,
            hole_r in 0.5f64..6.0,
            fx in 0.0f64..1.0,
            slot_r in 0.5f64..4.0,
            depth in 0.2f64..40.0,
            from in -20.0f64..20.0,
            rot in proptest::array::uniform3(-3.0f64..3.0),
            tol in 0.005f64..0.5,
        ) {
            let plane = Plane {
                frame: peet_math::Frame {
                    origin: DVec3::new(rot[2] * 10.0, rot[0] * 7.0, -rot[1] * 3.0),
                    rotation: peet_math::DQuat::from_euler(
                        peet_math::EulerRot::XYZ,
                        rot[0],
                        rot[1],
                        rot[2],
                    ),
                },
            };
            let mut s = Sketch::new();
            shapes::rectangle(&mut s, DVec2::ZERO, DVec2::new(w, h));
            let hole = DVec2::new(7.0 + fx * (w - 14.0), h * 0.25);
            s.add_circle(hole, hole_r);
            let y = h * 0.7;
            shapes::slot(&mut s, DVec2::new(8.0, y), DVec2::new(w - 8.0, y), slot_r);
            let plate: Vec<Region> = find_regions(&s)
                .regions
                .into_iter()
                .filter(|r| r.holes.len() == 2)
                .collect();
            proptest::prop_assert_eq!(plate.len(), 1);
            let solid = extrude(&plane, &plate, from, from + depth).unwrap();
            let counts = assert_valid(&solid);
            proptest::prop_assert_eq!(counts.genus, 2);
            let area = w * h
                - PI * hole_r * hole_r
                - (PI * slot_r * slot_r + (w - 16.0) * 2.0 * slot_r);
            let volume = measure::volume(&solid);
            proptest::prop_assert!((volume - area * depth).abs() < 1e-6 * volume);
            check_mesh(&solid, tol);
        }
    }

    #[test]
    fn bad_tolerance() {
        let s = cuboid(DVec3::ZERO, DVec3::ONE);
        assert!(tessellate(&s, 0.0).is_err());
        assert!(tessellate(&s, f64::NAN).is_err());
    }
}
