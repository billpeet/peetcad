//! Rigid placement of geometry and solids.
//!
//! A rigid motion (rotation plus translation, a [`Frame`]) keeps every surface and curve
//! of the same kind and every parameter the same, so a moved solid has exactly the same
//! topology and parameter ranges as the original.

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
}
