//! Questions about a finished solid: mass properties, and measurements between its
//! vertices, edges and faces.
//!
//! **Mass properties.** Volume and area are exact (the boundary integrals of
//! [`crate::validate::measure`]). The centre of gravity and the moments of inertia are
//! those of a fine triangle mesh of the solid, corrected by the exact volume: flat-faced
//! solids come out exact, curved ones within about one part in 10⁴.
//!
//! **Measurements** work on the unbounded geometry where that is what a person means (the
//! distance between two parallel faces is the distance between their planes) and on the
//! bounded edges otherwise (the distance from a vertex to an edge is to the nearest point
//! of the edge itself).

use peet_math::{Aabb, DMat3, DVec3, tolerance};

use crate::geom::{Curve3, Surface};
use crate::tessellate::tessellate;
use crate::topo::{EdgeId, FaceId, VertexId};
use crate::validate::measure;
use crate::{KernelError, Solid};

/// Mass properties of a solid of uniform density 1 (multiply by the density for mass and
/// inertia). Lengths in mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MassProperties {
    /// mm³.
    pub volume: f64,
    /// Total surface area, mm².
    pub area: f64,
    /// The centre of gravity.
    pub centroid: DVec3,
    /// The inertia tensor about the centre of gravity, along the model axes, mm⁵.
    pub inertia: DMat3,
    /// The principal moments of inertia (ascending), mm⁵.
    pub principal_moments: [f64; 3],
    pub bounds: Aabb,
}

/// The mesh used for the centre of gravity deviates from the solid by at most this
/// fraction of its size.
const MESH_PRECISION: f64 = 5e-5;

/// Mass properties of `solid` (which must be valid and not empty).
pub fn mass_properties(solid: &Solid) -> Result<MassProperties, KernelError> {
    if solid.faces.is_empty() {
        return Err(KernelError::InvalidInput(
            "there is no body to measure".to_owned(),
        ));
    }
    let bounds = solid.bounds();
    let size = bounds.size().length().max(tolerance::LINEAR);
    let volume = measure::volume(solid);
    let area: f64 = solid.face_ids().map(|f| measure::face_area(solid, f)).sum();
    let mesh = tessellate(solid, (size * MESH_PRECISION).clamp(1e-4, 0.05))?;
    // Tetrahedra from a point near the solid (less cancellation than from the origin).
    let reference = bounds.center();
    let mut mesh_volume = 0.0;
    let mut first = DVec3::ZERO;
    let mut second = DMat3::ZERO;
    for [a, b, c] in mesh.triangles() {
        let (a, b, c) = (a - reference, b - reference, c - reference);
        let det = a.dot(b.cross(c));
        mesh_volume += det / 6.0;
        first += (a + b + c) * (det / 24.0);
        let s = a + b + c;
        let outer = |v: DVec3| DMat3::from_cols(v * v.x, v * v.y, v * v.z);
        second += (outer(s) + outer(a) + outer(b) + outer(c)) * (det / 120.0);
    }
    if mesh_volume.abs() <= f64::MIN_POSITIVE {
        return Err(KernelError::InvalidInput(
            "the body encloses no volume".to_owned(),
        ));
    }
    // The mesh is slightly smaller than the solid. Its second moments are scaled as if
    // it were the solid shrunk evenly: by the volume ratio to the power 5/3.
    let scale = (volume / mesh_volume).abs().powf(5.0 / 3.0);
    let centroid = first / mesh_volume;
    // Second moments about the centroid, then the inertia tensor.
    let outer = |v: DVec3| DMat3::from_cols(v * v.x, v * v.y, v * v.z);
    let central = (second - outer(centroid) * mesh_volume) * scale;
    let trace = central.x_axis.x + central.y_axis.y + central.z_axis.z;
    let inertia = DMat3::from_diagonal(DVec3::splat(trace)) - central;
    Ok(MassProperties {
        volume,
        area,
        centroid: centroid + reference,
        inertia,
        principal_moments: symmetric_eigenvalues(&inertia),
        bounds,
    })
}

/// Eigenvalues of a symmetric 3×3 matrix, ascending (Jacobi rotations).
fn symmetric_eigenvalues(m: &DMat3) -> [f64; 3] {
    let mut a = m.to_cols_array_2d();
    for _ in 0..32 {
        // The largest off-diagonal element.
        let (mut p, mut q, mut big) = (0, 1, 0.0_f64);
        for (i, j) in [(0, 1), (0, 2), (1, 2)] {
            if a[i][j].abs() > big {
                (p, q, big) = (i, j, a[i][j].abs());
            }
        }
        let scale = a[0][0].abs() + a[1][1].abs() + a[2][2].abs();
        if big <= 1e-15 * scale.max(f64::MIN_POSITIVE) {
            break;
        }
        let theta = 0.5 * (2.0 * a[p][q]).atan2(a[q][q] - a[p][p]);
        let (s, c) = theta.sin_cos();
        // Rotate columns p and q, then rows p and q.
        for row in &mut a {
            let (akp, akq) = (row[p], row[q]);
            row[p] = c * akp - s * akq;
            row[q] = s * akp + c * akq;
        }
        let (row_p, row_q) = (a[p], a[q]);
        for k in 0..3 {
            a[p][k] = c * row_p[k] - s * row_q[k];
            a[q][k] = s * row_p[k] + c * row_q[k];
        }
    }
    let mut values = [a[0][0], a[1][1], a[2][2]];
    values.sort_by(f64::total_cmp);
    values
}

/// A vertex, an edge or a face of a solid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Entity {
    Vertex(VertexId),
    Edge(EdgeId),
    Face(FaceId),
}

/// The length of an edge (mm).
pub fn edge_length(solid: &Solid, edge: EdgeId) -> f64 {
    let e = solid.edge(edge);
    match &e.curve {
        Curve3::Line(_) => e.t1 - e.t0,
        Curve3::Circle(c) => c.radius * (e.t1 - e.t0),
        Curve3::Ellipse(_) => {
            // Simpson's rule on the speed: plenty for a smooth integrand.
            const STEPS: usize = 256;
            let h = (e.t1 - e.t0) / STEPS as f64;
            let speed = |i: usize| e.curve.derivative(e.t0 + h * i as f64).length();
            let mut sum = speed(0) + speed(STEPS);
            for i in 1..STEPS {
                sum += speed(i) * if i % 2 == 1 { 4.0 } else { 2.0 };
            }
            sum * h / 3.0
        }
    }
}

/// What one entity is by itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Description {
    Vertex {
        point: DVec3,
    },
    Edge {
        length: f64,
        /// The radius of a circular edge.
        radius: Option<f64>,
        /// The centre of a circular or elliptical edge.
        center: Option<DVec3>,
    },
    Face {
        area: f64,
        /// The radius of a cylindrical or spherical face.
        radius: Option<f64>,
    },
}

pub fn describe(solid: &Solid, entity: Entity) -> Description {
    match entity {
        Entity::Vertex(v) => Description::Vertex {
            point: solid.vertex(v).point,
        },
        Entity::Edge(e) => {
            let curve = &solid.edge(e).curve;
            Description::Edge {
                length: edge_length(solid, e),
                radius: match curve {
                    Curve3::Circle(c) => Some(c.radius),
                    _ => None,
                },
                center: curve.plane().map(|p| p.origin()),
            }
        }
        Entity::Face(f) => Description::Face {
            area: measure::face_area(solid, f),
            radius: match &solid.face(f).surface {
                Surface::Cylinder(c) => Some(c.radius),
                Surface::Sphere(s) => Some(s.radius),
                _ => None,
            },
        },
    }
}

/// A measurement between two entities (of the same solid or of two).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Between {
    /// The distance (mm), with the two points it is measured between. `None` when the
    /// entities meet at an angle, where a single distance means nothing (two faces that
    /// are not parallel).
    pub distance: Option<(f64, DVec3, DVec3)>,
    /// The angle between the two directions (radians, 0 to π/2 for planes and lines),
    /// when both have one: flat faces by their normals, straight edges by their lines, a
    /// flat face and a straight edge by the angle between the edge and the plane.
    pub angle: Option<f64>,
    /// What kind of distance it is, in words: "between the faces' planes".
    pub note: &'static str,
}

/// What an entity is, for measuring.
#[derive(Clone, Copy)]
enum Shape<'a> {
    Point(DVec3),
    Edge(&'a crate::topo::Edge),
    Plane(peet_math::Plane),
    /// A round face, by its axis (or a sphere, by its centre: `dir` is zero).
    Round {
        origin: DVec3,
        dir: DVec3,
    },
}

fn shape(solid: &Solid, entity: Entity) -> Shape<'_> {
    match entity {
        Entity::Vertex(v) => Shape::Point(solid.vertex(v).point),
        Entity::Edge(e) => Shape::Edge(solid.edge(e)),
        Entity::Face(f) => match &solid.face(f).surface {
            Surface::Plane(p) => Shape::Plane(*p),
            Surface::Sphere(s) => Shape::Round {
                origin: s.frame.origin,
                dir: DVec3::ZERO,
            },
            other => {
                let frame = other
                    .revolution_frame()
                    .expect("every curved face is turned about an axis");
                Shape::Round {
                    origin: frame.origin,
                    dir: frame.z_axis(),
                }
            }
        },
    }
}

/// The point of the bounded edge closest to `p`.
fn closest_on_edge(e: &crate::topo::Edge, p: DVec3) -> DVec3 {
    let mut t = e.curve.param(p);
    if e.curve.period().is_some() {
        t = e.t0 + (t - e.t0).rem_euclid(std::f64::consts::TAU);
    }
    if t >= e.t0 && t <= e.t1 {
        return e.curve.point(t);
    }
    let (a, b) = (e.curve.point(e.t0), e.curve.point(e.t1));
    if a.distance_squared(p) <= b.distance_squared(p) {
        a
    } else {
        b
    }
}

/// The closest points of two bounded edges: exact for two lines, otherwise found by
/// alternating projection from a set of starting points.
fn closest_between_edges(a: &crate::topo::Edge, b: &crate::topo::Edge) -> (DVec3, DVec3) {
    const STARTS: usize = 16;
    let mut best = (f64::INFINITY, DVec3::ZERO, DVec3::ZERO);
    for i in 0..=STARTS {
        let mut p = a.point_at_fraction(i as f64 / STARTS as f64);
        let mut q = closest_on_edge(b, p);
        for _ in 0..64 {
            let p2 = closest_on_edge(a, q);
            let q2 = closest_on_edge(b, p2);
            let moved = p2.distance(p) + q2.distance(q);
            (p, q) = (p2, q2);
            if moved <= 1e-12 {
                break;
            }
        }
        let d = p.distance(q);
        if d < best.0 {
            best = (d, p, q);
        }
    }
    (best.1, best.2)
}

/// The acute angle between two directions.
fn acute(a: DVec3, b: DVec3) -> f64 {
    a.dot(b).abs().clamp(0.0, 1.0).acos()
}

fn line_dir(e: &crate::topo::Edge) -> Option<DVec3> {
    match &e.curve {
        Curve3::Line(l) => Some(l.dir),
        _ => None,
    }
}

/// Measures between two entities, each on its own solid (pass the same solid twice for
/// two entities of one body).
pub fn between(a: (&Solid, Entity), b: (&Solid, Entity)) -> Between {
    let (sa, sb) = (shape(a.0, a.1), shape(b.0, b.1));
    let points = |p: DVec3, q: DVec3| Some((p.distance(q), p, q));
    let parallel = |u: DVec3, v: DVec3| u.cross(v).length() <= 1e-9;
    match (sa, sb) {
        (Shape::Point(p), Shape::Point(q)) => Between {
            distance: points(p, q),
            angle: None,
            note: "between the points",
        },
        (Shape::Point(p), Shape::Edge(e)) | (Shape::Edge(e), Shape::Point(p)) => Between {
            distance: points(p, closest_on_edge(e, p)),
            angle: None,
            note: "from the point to the edge",
        },
        (Shape::Point(p), Shape::Plane(pl)) | (Shape::Plane(pl), Shape::Point(p)) => Between {
            distance: points(p, pl.project_point(p)),
            angle: None,
            note: "from the point to the face's plane",
        },
        (Shape::Point(p), Shape::Round { origin, dir })
        | (Shape::Round { origin, dir }, Shape::Point(p)) => Between {
            distance: points(p, origin + dir * (p - origin).dot(dir)),
            angle: None,
            note: if dir == DVec3::ZERO {
                "from the point to the face's centre"
            } else {
                "from the point to the face's axis"
            },
        },
        (Shape::Edge(e), Shape::Edge(f)) => {
            let (p, q) = closest_between_edges(e, f);
            Between {
                distance: points(p, q),
                angle: line_dir(e).zip(line_dir(f)).map(|(u, v)| acute(u, v)),
                note: "between the edges",
            }
        }
        (Shape::Edge(e), Shape::Plane(pl)) | (Shape::Plane(pl), Shape::Edge(e)) => {
            let dir = line_dir(e);
            let angle = dir.map(|d| std::f64::consts::FRAC_PI_2 - acute(d, pl.normal()));
            // A distance only where the edge runs along the plane.
            let along = match &e.curve {
                Curve3::Line(l) => l.dir.dot(pl.normal()).abs() <= 1e-9,
                other => other
                    .plane()
                    .is_some_and(|p| parallel(p.normal(), pl.normal())),
            };
            let p = e.point_at_fraction(0.5);
            Between {
                distance: along.then(|| (pl.signed_distance(p).abs(), p, pl.project_point(p))),
                angle,
                note: "from the edge to the face's plane",
            }
        }
        (Shape::Edge(e), Shape::Round { origin, dir })
        | (Shape::Round { origin, dir }, Shape::Edge(e)) => {
            let p = e
                .curve
                .plane()
                .map_or(e.point_at_fraction(0.5), |c| c.origin());
            Between {
                distance: points(p, origin + dir * (p - origin).dot(dir)),
                angle: line_dir(e)
                    .filter(|_| dir != DVec3::ZERO)
                    .map(|d| acute(d, dir)),
                note: "from the edge to the face's axis",
            }
        }
        (Shape::Plane(p), Shape::Plane(q)) => {
            let same = parallel(p.normal(), q.normal());
            let at = q.origin();
            Between {
                distance: same.then(|| (p.signed_distance(at).abs(), p.project_point(at), at)),
                // Faces are compared by the angle between them, not their normals'
                // directions: 0 for parallel faces.
                angle: Some(acute(p.normal(), q.normal())),
                note: "between the faces' planes",
            }
        }
        (Shape::Plane(pl), Shape::Round { origin, dir })
        | (Shape::Round { origin, dir }, Shape::Plane(pl)) => {
            let along = dir == DVec3::ZERO || dir.dot(pl.normal()).abs() <= 1e-9;
            Between {
                distance: along.then(|| {
                    (
                        pl.signed_distance(origin).abs(),
                        origin,
                        pl.project_point(origin),
                    )
                }),
                angle: (dir != DVec3::ZERO)
                    .then(|| std::f64::consts::FRAC_PI_2 - acute(dir, pl.normal())),
                note: "from the round face's axis to the flat face's plane",
            }
        }
        (
            Shape::Round { origin, dir },
            Shape::Round {
                origin: o2,
                dir: d2,
            },
        ) => {
            // Centre to centre, axis to centre, or between parallel axes.
            let (p, q) = if dir == DVec3::ZERO && d2 == DVec3::ZERO {
                (origin, o2)
            } else if dir == DVec3::ZERO {
                (origin, o2 + d2 * (origin - o2).dot(d2))
            } else {
                (origin + dir * (o2 - origin).dot(dir), o2)
            };
            let both_axes = dir != DVec3::ZERO && d2 != DVec3::ZERO;
            Between {
                distance: (!both_axes || parallel(dir, d2)).then(|| (p.distance(q), p, q)),
                angle: both_axes.then(|| acute(dir, d2)),
                note: "between the round faces' axes",
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::*;
    use crate::topo::test_shapes::cuboid;
    use peet_math::{DVec2, Plane};
    use peet_sketch::Sketch;
    use peet_sketch::region::find_regions;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * b.abs().max(1.0)
    }

    #[test]
    fn box_properties_are_exact() {
        let s = cuboid(DVec3::new(1.0, 2.0, 3.0), DVec3::new(5.0, 4.0, 9.0));
        let m = mass_properties(&s).unwrap();
        assert!(close(m.volume, 48.0, 1e-12));
        assert!(close(m.area, 2.0 * (8.0 + 24.0 + 12.0), 1e-12));
        assert!(m.centroid.abs_diff_eq(DVec3::new(3.0, 3.0, 6.0), 1e-9));
        // A box of sides a, b, c: Ixx = V (b² + c²) / 12.
        let (a, b, c) = (4.0_f64, 2.0_f64, 6.0_f64);
        let v = 48.0;
        assert!(close(m.inertia.x_axis.x, v * (b * b + c * c) / 12.0, 1e-9));
        assert!(close(m.inertia.y_axis.y, v * (a * a + c * c) / 12.0, 1e-9));
        assert!(close(m.inertia.z_axis.z, v * (a * a + b * b) / 12.0, 1e-9));
        assert!(m.inertia.x_axis.y.abs() < 1e-9);
        let mut expected = [
            v * (b * b + c * c) / 12.0,
            v * (a * a + c * c) / 12.0,
            v * (a * a + b * b) / 12.0,
        ];
        expected.sort_by(f64::total_cmp);
        for (got, want) in m.principal_moments.iter().zip(expected) {
            assert!(close(*got, want, 1e-9));
        }
    }

    #[test]
    fn turned_shapes() {
        // A cone standing on z = 0: its centre of gravity is a quarter of the way up.
        let mut s = Sketch::new();
        for (a, b) in [
            (DVec2::ZERO, DVec2::new(6.0, 0.0)),
            (DVec2::new(6.0, 0.0), DVec2::new(0.0, 12.0)),
            (DVec2::new(0.0, 12.0), DVec2::ZERO),
        ] {
            s.add_line(a, b);
        }
        let axis = crate::revolve::RevolveAxis {
            origin: DVec2::ZERO,
            dir: DVec2::Y,
        };
        let cone = crate::revolve::revolve(
            &Plane::front(),
            &find_regions(&s).regions,
            &axis,
            0.0,
            2.0 * PI,
        )
        .unwrap();
        let m = mass_properties(&cone).unwrap();
        assert!(close(m.volume, PI * 36.0 * 12.0 / 3.0, 1e-12));
        assert!(m.centroid.abs_diff_eq(DVec3::new(0.0, 0.0, 3.0), 1e-3));
        // About its axis: 3/10 m r².
        assert!(close(m.inertia.z_axis.z, 0.3 * m.volume * 36.0, 3e-4));
        // A ball: 2/5 m r² about every axis.
        let mut s = Sketch::new();
        s.add_arc(DVec2::ZERO, DVec2::new(0.0, -5.0), DVec2::new(0.0, 5.0));
        s.add_line(DVec2::new(0.0, 5.0), DVec2::new(0.0, -5.0));
        let ball = crate::revolve::revolve(
            &Plane::front(),
            &find_regions(&s).regions,
            &axis,
            0.0,
            2.0 * PI,
        )
        .unwrap();
        let m = mass_properties(&ball).unwrap();
        assert!(m.centroid.length() < 1e-3);
        for moment in m.principal_moments {
            assert!(close(moment, 0.4 * m.volume * 25.0, 3e-4));
        }
        assert!(mass_properties(&Solid::new()).is_err());
    }

    #[test]
    fn measurements() {
        let s = cuboid(DVec3::ZERO, DVec3::new(4.0, 2.0, 1.0));
        // Faces: 0 bottom, 1 top, 2 front (−Y), 3 back, 4 left, 5 right.
        let m = between((&s, Entity::Face(FaceId(0))), (&s, Entity::Face(FaceId(1))));
        assert!(close(m.distance.unwrap().0, 1.0, 1e-12));
        assert!(m.angle.unwrap().abs() < 1e-12);
        let m = between((&s, Entity::Face(FaceId(0))), (&s, Entity::Face(FaceId(2))));
        assert!(m.distance.is_none());
        assert!(close(m.angle.unwrap(), PI / 2.0, 1e-12));
        // Opposite corners.
        let m = between(
            (&s, Entity::Vertex(VertexId(0))),
            (&s, Entity::Vertex(VertexId(7))),
        );
        assert!(close(m.distance.unwrap().0, 21f64.sqrt(), 1e-12));
        // A vertex to the far face.
        let m = between(
            (&s, Entity::Vertex(VertexId(0))),
            (&s, Entity::Face(FaceId(5))),
        );
        assert!(close(m.distance.unwrap().0, 4.0, 1e-12));
        // Two parallel edges, and two that are square to each other without meeting.
        let edges: Vec<EdgeId> = s.edge_ids().collect();
        let dir = |e: EdgeId| match s.edge(e).curve {
            Curve3::Line(l) => l.dir,
            _ => unreachable!(),
        };
        let x_edges: Vec<EdgeId> = edges
            .iter()
            .copied()
            .filter(|&e| dir(e).x.abs() > 0.5)
            .collect();
        let m = between(
            (&s, Entity::Edge(x_edges[0])),
            (&s, Entity::Edge(x_edges[1])),
        );
        assert!(m.angle.unwrap().abs() < 1e-12);
        assert!(m.distance.unwrap().0 > 0.99);
        for e in &edges {
            assert!(close(
                edge_length(&s, *e),
                s.vertex(s.edge(*e).start)
                    .point
                    .distance(s.vertex(s.edge(*e).end).point),
                1e-12
            ));
        }
        assert!(matches!(
            describe(&s, Entity::Face(FaceId(1))),
            Description::Face { area, radius: None } if close(area, 8.0, 1e-12)
        ));
    }

    #[test]
    fn eigenvalues() {
        let m = DMat3::from_cols(
            DVec3::new(2.0, 1.0, 0.0),
            DVec3::new(1.0, 2.0, 0.0),
            DVec3::new(0.0, 0.0, 5.0),
        );
        let e = symmetric_eigenvalues(&m);
        assert!(close(e[0], 1.0, 1e-12) && close(e[1], 3.0, 1e-12) && close(e[2], 5.0, 1e-12));
    }
}
