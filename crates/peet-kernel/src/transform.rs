//! Rigid placement of geometry and solids, and their mirror images.
//!
//! A rigid motion (rotation plus translation, a [`Frame`]) keeps every surface and curve
//! of the same kind and every parameter the same, so a moved solid has exactly the same
//! topology and parameter ranges as the original. A reflection ([`mirrored`]) keeps the
//! topology too, with every loop walked the other way.

use peet_math::{DVec3, Frame, Plane};

use crate::Solid;
use crate::geom::{Circle3, Cone, Curve3, Cylinder, Ellipse3, Line3, Sphere, Surface, Torus};

/// `surface` placed by `frame` (its coordinates are taken as local to `frame`).
pub fn surface(s: &Surface, frame: &Frame) -> Surface {
    match s {
        Surface::Plane(p) => Surface::Plane(Plane {
            frame: frame.compose(&p.frame),
        }),
        Surface::Cylinder(c) => Surface::Cylinder(Cylinder {
            frame: frame.compose(&c.frame),
            radius: c.radius,
        }),
        Surface::Cone(c) => Surface::Cone(Cone {
            frame: frame.compose(&c.frame),
            ..*c
        }),
        Surface::Sphere(s) => Surface::Sphere(Sphere {
            frame: frame.compose(&s.frame),
            ..*s
        }),
        Surface::Torus(t) => Surface::Torus(Torus {
            frame: frame.compose(&t.frame),
            ..*t
        }),
        Surface::Nurbs(s) => Surface::Nurbs(std::sync::Arc::new(s.transformed(frame))),
    }
}

/// `curve` placed by `frame`. Parameters are unchanged.
pub fn curve(c: &Curve3, frame: &Frame) -> Curve3 {
    match c {
        Curve3::Line(l) => Curve3::Line(Line3 {
            origin: frame.to_world(l.origin),
            dir: frame.vector_to_world(l.dir),
        }),
        Curve3::Circle(c) => Curve3::Circle(Circle3 {
            frame: frame.compose(&c.frame),
            radius: c.radius,
        }),
        Curve3::Ellipse(e) => Curve3::Ellipse(Ellipse3 {
            frame: frame.compose(&e.frame),
            major: e.major,
            minor: e.minor,
        }),
        Curve3::Nurbs(c) => Curve3::Nurbs(std::sync::Arc::new(c.transformed(frame))),
    }
}

/// `solid` placed by `frame`: same topology, moved geometry.
pub fn solid(s: &Solid, frame: &Frame) -> Solid {
    let mut out = s.clone();
    for v in &mut out.vertices {
        v.point = frame.to_world(v.point);
    }
    for e in &mut out.edges {
        e.curve = curve(&e.curve, frame);
    }
    for f in &mut out.faces {
        f.surface = surface(&f.surface, frame);
    }
    out
}

/// A point placed by `frame` (for symmetry with the functions above).
pub fn point(p: DVec3, frame: &Frame) -> DVec3 {
    frame.to_world(p)
}

/// A reflection in the plane through `point` with the normal `normal` (a unit vector).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reflection {
    pub point: DVec3,
    pub normal: DVec3,
}

impl Reflection {
    pub fn point(&self, p: DVec3) -> DVec3 {
        p - self.normal * (2.0 * (p - self.point).dot(self.normal))
    }

    pub fn vector(&self, v: DVec3) -> DVec3 {
        v - self.normal * (2.0 * v.dot(self.normal))
    }

    /// The reflection of `frame`, made right-handed again by turning its Y axis round:
    /// its origin, X and Z are reflected. A point at `(x, y, z)` in `frame` is at
    /// `(x, −y, z)` in the result.
    fn frame(&self, frame: &Frame) -> Frame {
        let x = self.vector(frame.x_axis());
        let z = self.vector(frame.z_axis());
        Frame::from_origin_z_x(self.point(frame.origin), z, x)
            .expect("a reflected frame keeps its axes square")
    }
}

/// `surface` reflected. The analytic surfaces keep their natural normal (a plane's along
/// its reflected Z, the others' away from their axis or centre), so faces on them keep
/// their orientation; a freeform surface's natural normal is turned round.
fn reflected_surface(s: &Surface, r: &Reflection) -> Surface {
    match s {
        Surface::Plane(p) => Surface::Plane(Plane {
            frame: r.frame(&p.frame),
        }),
        Surface::Cylinder(c) => Surface::Cylinder(Cylinder {
            frame: r.frame(&c.frame),
            radius: c.radius,
        }),
        Surface::Cone(c) => Surface::Cone(Cone {
            frame: r.frame(&c.frame),
            ..*c
        }),
        Surface::Sphere(s) => Surface::Sphere(Sphere {
            frame: r.frame(&s.frame),
            ..*s
        }),
        Surface::Torus(t) => Surface::Torus(Torus {
            frame: r.frame(&t.frame),
            ..*t
        }),
        Surface::Nurbs(s) => Surface::Nurbs(std::sync::Arc::new(s.mapped(|p| r.point(p)))),
    }
}

/// `curve` reflected, and whether its parameter now runs the other way (a circle's or
/// an ellipse's: `t` becomes `−t`).
fn reflected_curve(c: &Curve3, r: &Reflection) -> (Curve3, bool) {
    match c {
        Curve3::Line(l) => (
            Curve3::Line(Line3 {
                origin: r.point(l.origin),
                dir: r.vector(l.dir),
            }),
            false,
        ),
        Curve3::Circle(c) => (
            Curve3::Circle(Circle3 {
                frame: r.frame(&c.frame),
                radius: c.radius,
            }),
            true,
        ),
        Curve3::Ellipse(e) => (
            Curve3::Ellipse(Ellipse3 {
                frame: r.frame(&e.frame),
                major: e.major,
                minor: e.minor,
            }),
            true,
        ),
        Curve3::Nurbs(n) => (
            Curve3::Nurbs(std::sync::Arc::new(n.mapped(|p| r.point(p)))),
            false,
        ),
    }
}

/// `solid` reflected in a plane: its mirror image, a valid solid with its faces facing
/// out. A reflection turns every loop round, so each loop is walked the other way.
pub fn mirrored(s: &Solid, reflection: &Reflection) -> Solid {
    use std::f64::consts::TAU;
    let mut out = s.clone();
    for v in &mut out.vertices {
        v.point = reflection.point(v.point);
    }
    // Whether each edge now runs from its old end to its old start.
    let mut turned = vec![false; out.edges.len()];
    for (i, e) in out.edges.iter_mut().enumerate() {
        let (curve, backwards) = reflected_curve(&e.curve, reflection);
        e.curve = curve;
        if backwards {
            // `t` is `−t` now: the range runs from −t1 to −t0, kept in [0, 2π) at its start
            let shift = (-e.t1).div_euclid(TAU) * TAU;
            let (t0, t1) = (-e.t1 - shift, -e.t0 - shift);
            e.t0 = t0;
            e.t1 = t1;
            std::mem::swap(&mut e.start, &mut e.end);
            turned[i] = true;
        }
    }
    for f in &mut out.faces {
        if matches!(f.surface, Surface::Nurbs(_)) {
            f.reversed = !f.reversed;
        }
        f.surface = reflected_surface(&f.surface, reflection);
    }
    // Each loop is walked the other way: every coedge goes the other way along its
    // edge (which an edge turned round undoes), and next and previous swap.
    for c in &mut out.coedges {
        if !turned[c.edge.index()] {
            c.reversed = !c.reversed;
        }
        std::mem::swap(&mut c.next, &mut c.prev);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topo::test_shapes::cuboid;
    use crate::validate::{measure, validate};
    use peet_math::DQuat;

    #[test]
    fn moved_box_stays_valid() {
        let b = cuboid(DVec3::ZERO, DVec3::new(1.0, 2.0, 3.0));
        let f = Frame {
            origin: DVec3::new(5.0, -2.0, 1.0),
            rotation: DQuat::from_euler(peet_math::EulerRot::XYZ, 0.3, 1.1, -0.4),
        };
        let m = solid(&b, &f);
        assert!(validate(&m).is_ok());
        assert!((measure::volume(&m) - 6.0).abs() < 1e-9);
        assert!(
            m.vertices[7]
                .point
                .abs_diff_eq(f.to_world(DVec3::new(1.0, 2.0, 3.0)), 1e-12)
        );
    }

    /// Mirrors `s` in a slanted plane and checks the image: valid, meshable, the same
    /// volume, and every vertex the reflection of the original's.
    #[track_caller]
    fn check_mirror(s: &Solid) -> Solid {
        let r = Reflection {
            point: DVec3::new(3.0, -1.0, 2.0),
            normal: DVec3::new(1.0, 2.0, -0.5).normalize(),
        };
        let m = mirrored(s, &r);
        crate::validate::assert_valid(&m);
        if let Err(e) = crate::tessellate::tessellate(&m, 0.05) {
            panic!("the mirror image can't be tessellated: {e}");
        }
        let (a, b) = (measure::volume(s), measure::volume(&m));
        assert!(
            (a - b).abs() < 1e-6 * a.abs().max(1.0),
            "{b} instead of {a}"
        );
        for (v, w) in s.vertices.iter().zip(&m.vertices) {
            assert!(w.point.abs_diff_eq(r.point(v.point), 1e-12));
        }
        // Every edge's ends are where its curve has them
        for e in &m.edges {
            let at = |t: f64| e.curve.point(t);
            assert!(at(e.t0).distance(m.vertices[e.start.index()].point) < 1e-9);
            assert!(at(e.t1).distance(m.vertices[e.end.index()].point) < 1e-9);
        }
        m
    }

    #[test]
    fn mirrored_box_cylinders_and_ball_stay_valid() {
        use crate::validate::test_solids::{oblique_cylinder, pocketed_cylinder};
        check_mirror(&cuboid(DVec3::ZERO, DVec3::new(1.0, 2.0, 3.0)));
        check_mirror(&oblique_cylinder(2.0, 5.0, 0.4));
        check_mirror(&pocketed_cylinder());
        let ball = crate::primitive::ball(&Frame::default(), 3.0).unwrap();
        check_mirror(&ball);
    }

    #[test]
    fn mirrored_arcs_and_freeform_faces_stay_valid() {
        use peet_math::DVec2;
        use peet_sketch::Sketch;
        use peet_sketch::region::find_regions;
        // A slot: two arcs and two lines, extruded
        let mut sketch = Sketch::new();
        sketch.add_line(DVec2::new(0.0, 0.0), DVec2::new(10.0, 0.0));
        sketch.add_arc(
            DVec2::new(10.0, 3.0),
            DVec2::new(10.0, 0.0),
            DVec2::new(10.0, 6.0),
        );
        sketch.add_line(DVec2::new(10.0, 6.0), DVec2::new(0.0, 6.0));
        sketch.add_arc(
            DVec2::new(0.0, 3.0),
            DVec2::new(0.0, 6.0),
            DVec2::new(0.0, 0.0),
        );
        let regions = find_regions(&sketch).regions;
        let slot = crate::extrude::extrude(&Plane::TOP, &regions, 0.0, 4.0).unwrap();
        check_mirror(&slot);
        // A loft from a square to a circle: freeform sides
        let mut square = Sketch::new();
        peet_sketch::shapes::rectangle(&mut square, DVec2::splat(-2.0), DVec2::splat(2.0));
        let mut round = Sketch::new();
        round.add_circle(DVec2::ZERO, 1.5);
        let (a, b) = (
            find_regions(&square).regions.remove(0),
            find_regions(&round).regions.remove(0),
        );
        let top =
            Plane::from_origin_normal_x(DVec3::new(0.0, 0.0, 5.0), DVec3::Z, DVec3::X).unwrap();
        let lofted = crate::loft::loft(&[
            crate::loft::LoftSection {
                plane: &Plane::TOP,
                region: &a,
            },
            crate::loft::LoftSection {
                plane: &top,
                region: &b,
            },
        ])
        .unwrap();
        let m = check_mirror(&lofted);
        // Mirrored twice, it is the original again
        let r = Reflection {
            point: DVec3::new(3.0, -1.0, 2.0),
            normal: DVec3::new(1.0, 2.0, -0.5).normalize(),
        };
        let back = mirrored(&m, &r);
        for (v, w) in lofted.vertices.iter().zip(&back.vertices) {
            assert!(v.point.abs_diff_eq(w.point, 1e-9));
        }
    }
}
