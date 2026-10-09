//! General sweeps: validity, meshes, and volumes against Pappus's theorem (a profile
//! whose centroid rides on the path sweeps its area times the path's length).

use std::f64::consts::PI;

use peet_math::{DVec2, DVec3, Plane};
use peet_sketch::Sketch;
use peet_sketch::region::{Region, find_regions};

use super::*;
use crate::geom::Surface;
use crate::tessellate::tessellate;
use crate::validate::{assert_valid, measure};

fn v3(x: f64, y: f64, z: f64) -> DVec3 {
    DVec3::new(x, y, z)
}

fn top() -> Plane {
    Plane::from_origin_normal_x(DVec3::ZERO, DVec3::Z, DVec3::X).unwrap()
}

fn circle(r: f64) -> Region {
    let mut s = Sketch::new();
    s.add_circle(DVec2::ZERO, r);
    find_regions(&s).regions.remove(0)
}

fn ring(outer: f64, inner: f64) -> Region {
    let mut s = Sketch::new();
    s.add_circle(DVec2::ZERO, outer);
    s.add_circle(DVec2::ZERO, inner);
    let mut regions = find_regions(&s).regions;
    let at = regions
        .iter()
        .position(|r| !r.holes.is_empty())
        .expect("a ring");
    regions.remove(at)
}

fn rectangle(w: f64, h: f64) -> Region {
    let mut s = Sketch::new();
    peet_sketch::shapes::rectangle(
        &mut s,
        DVec2::new(-w / 2.0, -h / 2.0),
        DVec2::new(w / 2.0, h / 2.0),
    );
    find_regions(&s).regions.remove(0)
}

fn length(path: &[PathPiece]) -> f64 {
    let n = 4000;
    path.iter()
        .map(|p| {
            (0..n)
                .map(|k| {
                    p.point(f64::from(k) / f64::from(n))
                        .distance(p.point(f64::from(k + 1) / f64::from(n)))
                })
                .sum::<f64>()
        })
        .sum()
}

/// An S-shaped path in the XZ plane, leaving the origin straight up.
fn s_curve() -> Vec<PathPiece> {
    let curve = NurbsCurve::interpolate(&[
        v3(0.0, 0.0, 0.0),
        v3(0.0, 0.0, 15.0),
        v3(12.0, 0.0, 40.0),
        v3(30.0, 0.0, 55.0),
        v3(30.0, 0.0, 80.0),
    ])
    .unwrap();
    // Start exactly along Z: a straight lead-in the curve carries on from.
    let dir = curve.evaluate(0.0)[1].normalize();
    let lead = PathPiece::line(-dir * 10.0, DVec3::ZERO).unwrap();
    vec![lead, PathPiece::nurbs(curve)]
}

#[track_caller]
fn swept(plane: &Plane, region: &Region, path: &[PathPiece]) -> (Solid, Vec<SweepFace>) {
    let (solid, faces) = match sweep_traced(plane, region, path) {
        Ok(out) => out,
        Err(e) => panic!("sweep failed: {e}"),
    };
    assert_valid(&solid);
    assert_eq!(faces.len(), solid.faces.len());
    // The mesh is closed.
    let mesh = tessellate(&solid, 0.02).unwrap();
    let mut uses = std::collections::HashMap::new();
    let bits = |p: DVec3| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
    for f in &mesh.faces {
        for t in &f.triangles {
            let [a, b, c] = t.map(|i| f.positions[i as usize]);
            for (p, q) in [(a, b), (b, c), (c, a)] {
                *uses.entry((bits(p), bits(q))).or_insert(0) += 1;
                *uses.entry((bits(q), bits(p))).or_insert(0) -= 1;
            }
        }
    }
    assert!(uses.values().all(|n| *n == 0), "the mesh has open edges");
    (solid, faces)
}

/// The plane square to the start of `path`, through it.
fn square_to(path: &[PathPiece]) -> Plane {
    let dir = path[0].direction(0.0);
    Plane::from_origin_normal_x(path[0].point(0.0), dir, dir.any_orthonormal_vector()).unwrap()
}

#[test]
fn a_rod_along_a_spline() {
    let path = s_curve();
    let plane = square_to(&path);
    let (solid, faces) = swept(&plane, &circle(3.0), &path);
    let expected = PI * 9.0 * length(&path);
    let volume = measure::volume(&solid);
    assert!(
        (volume - expected).abs() < 2e-5 * expected,
        "{volume} instead of {expected}"
    );
    // Two flat ends and four sides, all named after the circle.
    assert_eq!(faces.iter().filter(|f| **f == SweepFace::Start).count(), 1);
    assert_eq!(faces.iter().filter(|f| **f == SweepFace::End).count(), 1);
    assert_eq!(
        faces
            .iter()
            .filter(|f| **f
                == SweepFace::Side {
                    loop_index: 0,
                    edge: 0
                })
            .count(),
        4
    );
    // The far end is the profile, square to the path there.
    let end = faces.iter().position(|f| *f == SweepFace::End).unwrap();
    let Surface::Plane(cap) = &solid.faces[end].surface else {
        panic!("the end is flat")
    };
    let last = path.last().unwrap();
    assert!(cap.normal().dot(last.direction(1.0)).abs() > 1.0 - 1e-9);
    assert!(cap.signed_distance(last.point(1.0)).abs() < 1e-9);
}

#[test]
fn a_pipe_has_its_bore() {
    let path = s_curve();
    let plane = square_to(&path);
    let (solid, faces) = swept(&plane, &ring(4.0, 2.5), &path);
    let expected = PI * (16.0 - 6.25) * length(&path);
    let volume = measure::volume(&solid);
    assert!(
        (volume - expected).abs() < 2e-5 * expected,
        "{volume} instead of {expected}"
    );
    for loop_index in [0, 1] {
        assert!(faces.contains(&SweepFace::Side {
            loop_index,
            edge: 0
        }));
    }
    assert_eq!(faces.iter().filter(|f| **f == SweepFace::Start).count(), 1);
}

#[test]
fn a_bar_up_a_helix_does_not_twist() {
    // One and a half turns of a helix of radius 20 and pitch 30.
    let (radius, pitch, turns) = (20.0, 30.0, 1.5);
    let n = 60;
    let points: Vec<DVec3> = (0..=n)
        .map(|k| {
            let a = turns * 2.0 * PI * f64::from(k) / f64::from(n);
            v3(radius * a.cos(), radius * a.sin(), pitch * a / (2.0 * PI))
        })
        .collect();
    let path = vec![PathPiece::nurbs(NurbsCurve::interpolate(&points).unwrap())];
    let plane = square_to(&path);
    let (solid, _) = swept(&plane, &rectangle(6.0, 4.0), &path);
    let expected = 24.0 * length(&path);
    let volume = measure::volume(&solid);
    assert!(
        (volume - expected).abs() < 1e-4 * expected,
        "{volume} instead of {expected}"
    );
}

#[test]
fn what_cannot_be_swept_says_why() {
    let plane = top();
    let up = PathPiece::line(DVec3::ZERO, v3(0.0, 0.0, 20.0)).unwrap();
    // A corner.
    let across = PathPiece::line(v3(0.0, 0.0, 20.0), v3(20.0, 0.0, 20.0)).unwrap();
    let e = sweep(&plane, &circle(3.0), &[up.clone(), across]).unwrap_err();
    assert!(e.to_string().contains("corner"), "{e}");
    // Pieces apart.
    let apart = PathPiece::line(v3(0.0, 0.0, 25.0), v3(0.0, 0.0, 40.0)).unwrap();
    let e = sweep(&plane, &circle(3.0), &[up.clone(), apart]).unwrap_err();
    assert!(e.to_string().contains("end to end"), "{e}");
    // A bend tighter than the profile.
    let tight = NurbsCurve::interpolate(&[
        v3(0.0, 0.0, 0.0),
        v3(0.0, 0.0, 10.0),
        v3(0.0, 0.0, 20.0),
        v3(2.0, 0.0, 22.0),
        v3(12.0, 0.0, 22.0),
        v3(22.0, 0.0, 22.0),
    ])
    .unwrap();
    let path = vec![PathPiece::nurbs(tight)];
    let e = sweep(&square_to(&path), &circle(8.0), &path).unwrap_err();
    assert!(e.to_string().contains("doesn't fit round a bend"), "{e}");
    // A path along the profile's plane.
    let flat = PathPiece::line(DVec3::ZERO, v3(20.0, 0.0, 0.0)).unwrap();
    let e = sweep(&plane, &circle(3.0), &[flat]).unwrap_err();
    assert!(e.to_string().contains("leave the plane"), "{e}");
    // A straight path is an extrusion.
    let (solid, _) = swept(&plane, &rectangle(6.0, 4.0), &[up]);
    assert!((measure::volume(&solid) - 480.0).abs() < 1e-9);
}
