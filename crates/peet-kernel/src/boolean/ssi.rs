//! Surface–surface intersection for planes and cylinders, as exact analytic curves.

use peet_math::tolerance::{self, ANGULAR, LINEAR};
use peet_math::{DVec2, DVec3, Frame, Plane};

use crate::geom::{Circle3, Curve3, Cylinder, Ellipse3, Line3, Surface};

/// How two unbounded surfaces meet.
#[derive(Clone, Debug)]
pub(crate) enum Ssi {
    /// They don't meet.
    None,
    /// They are the same surface.
    Coincident,
    /// They meet in these curves (lines, or one circle or ellipse).
    Curves(Vec<Curve3>),
    /// Cylinders with non-parallel axes: a quartic curve the kernel doesn't represent.
    Unsupported,
}

/// Intersects two surfaces. Lines get their origin near `near` so parameters stay small.
pub(crate) fn intersect(a: &Surface, b: &Surface, near: DVec3) -> Ssi {
    match (a, b) {
        (Surface::Plane(p), Surface::Plane(q)) => plane_plane(p, q, near),
        (Surface::Plane(p), Surface::Cylinder(c)) | (Surface::Cylinder(c), Surface::Plane(p)) => {
            plane_cylinder(p, c, near)
        }
        (Surface::Cylinder(c), Surface::Cylinder(d)) => cylinder_cylinder(c, d, near),
    }
}

fn line(origin: DVec3, dir: DVec3, near: DVec3) -> Curve3 {
    Curve3::Line(Line3 {
        origin: origin + dir * (near - origin).dot(dir),
        dir,
    })
}

fn plane_plane(p: &Plane, q: &Plane, near: DVec3) -> Ssi {
    let (n1, n2) = (p.normal(), q.normal());
    if tolerance::directions_parallel(n1, n2) {
        return if p.signed_distance(q.origin()).abs() <= LINEAR {
            Ssi::Coincident
        } else {
            Ssi::None
        };
    }
    let cross = n1.cross(n2);
    let dir = cross.normalize();
    let (d1, d2) = (n1.dot(p.origin()), n2.dot(q.origin()));
    // The point of the line closest to the world origin.
    let origin = (n2.cross(cross) * d1 + cross.cross(n1) * d2) / cross.length_squared();
    Ssi::Curves(vec![line(origin, dir, near)])
}

fn plane_cylinder(p: &Plane, c: &Cylinder, near: DVec3) -> Ssi {
    let (n, axis, r) = (p.normal(), c.axis(), c.radius);
    let cos = n.dot(axis);
    if cos.abs() <= ANGULAR {
        // Axis parallel to the plane: no, one (tangent) or two lines along the axis.
        let d = p.signed_distance(c.axis_origin());
        if d.abs() > r + LINEAR {
            return Ssi::None;
        }
        let foot = c.axis_origin() - n * d;
        if (d.abs() - r).abs() <= LINEAR {
            return Ssi::Curves(vec![line(foot, axis, near)]);
        }
        let side = axis.cross(n).normalize();
        let w = (r * r - d * d).max(0.0).sqrt();
        return Ssi::Curves(vec![
            line(foot + side * w, axis, near),
            line(foot - side * w, axis, near),
        ]);
    }
    let center = c.axis_origin() + axis * ((p.origin() - c.axis_origin()).dot(n) / cos);
    if tolerance::directions_parallel(n, axis) {
        // The circle shares the cylinder's frame, so its parameter is the cylinder's angle.
        return Ssi::Curves(vec![Curve3::Circle(Circle3 {
            frame: Frame {
                origin: center,
                rotation: c.frame.rotation,
            },
            radius: r,
        })]);
    }
    let minor_dir = axis.cross(n).normalize();
    let major_dir = n.cross(minor_dir);
    match Frame::from_origin_z_x(center, n, major_dir) {
        Some(frame) => Ssi::Curves(vec![Curve3::Ellipse(Ellipse3 {
            frame,
            major: r / cos.abs(),
            minor: r,
        })]),
        None => Ssi::None,
    }
}

fn cylinder_cylinder(c: &Cylinder, d: &Cylinder, near: DVec3) -> Ssi {
    if !tolerance::directions_parallel(c.axis(), d.axis()) {
        return Ssi::Unsupported;
    }
    // Two circles in the plane perpendicular to the axes, in `c`'s frame.
    let local = c.frame.to_local(d.axis_origin());
    let center = DVec2::new(local.x, local.y);
    let dist = center.length();
    let (r1, r2) = (c.radius, d.radius);
    if dist <= LINEAR {
        return if (r1 - r2).abs() <= LINEAR {
            Ssi::Coincident
        } else {
            Ssi::None
        };
    }
    if dist > r1 + r2 + LINEAR || dist < (r1 - r2).abs() - LINEAR {
        return Ssi::None;
    }
    let u = center / dist;
    let to_line = |q: DVec2| line(c.frame.to_world(q.extend(0.0)), c.axis(), near);
    if (dist - (r1 + r2)).abs() <= LINEAR {
        return Ssi::Curves(vec![to_line(u * r1)]);
    }
    if (dist - (r1 - r2).abs()).abs() <= LINEAR {
        // Internally tangent: the touching point is on the far side of the smaller one.
        let s = if r1 >= r2 { 1.0 } else { -1.0 };
        return Ssi::Curves(vec![to_line(u * (r1 * s))]);
    }
    let along = (r1 * r1 - r2 * r2 + dist * dist) / (2.0 * dist);
    let h = (r1 * r1 - along * along).max(0.0).sqrt();
    let perp = u.perp();
    Ssi::Curves(vec![
        to_line(u * along + perp * h),
        to_line(u * along - perp * h),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use peet_math::DQuat;

    fn on_both(a: &Surface, b: &Surface, c: &Curve3) {
        for i in 0..16 {
            let p = c.point(-3.0 + 0.4 * f64::from(i));
            assert!(
                a.signed_distance(p).abs() < 1e-9,
                "{:?}",
                a.signed_distance(p)
            );
            assert!(
                b.signed_distance(p).abs() < 1e-9,
                "{:?}",
                b.signed_distance(p)
            );
        }
    }

    fn curves(a: &Surface, b: &Surface) -> Vec<Curve3> {
        match intersect(a, b, DVec3::ZERO) {
            Ssi::Curves(c) => {
                for k in &c {
                    on_both(a, b, k);
                }
                c
            }
            other => panic!("expected curves, got {other:?}"),
        }
    }

    fn cyl(origin: DVec3, rotation: DQuat, radius: f64) -> Surface {
        Surface::Cylinder(Cylinder {
            frame: Frame { origin, rotation },
            radius,
        })
    }

    #[test]
    fn planes() {
        let a = Surface::Plane(Plane::TOP);
        let b = Surface::Plane(
            Plane::from_origin_normal_x(
                DVec3::new(1.0, 2.0, 3.0),
                DVec3::new(1.0, 0.0, 1.0),
                DVec3::Y,
            )
            .unwrap(),
        );
        assert_eq!(curves(&a, &b).len(), 1);
        let shifted = Surface::Plane(
            Plane::from_origin_normal_x(DVec3::Z * 2.0, -DVec3::Z, DVec3::Y).unwrap(),
        );
        assert!(matches!(intersect(&a, &shifted, DVec3::ZERO), Ssi::None));
        let flipped =
            Surface::Plane(Plane::from_origin_normal_x(DVec3::X, -DVec3::Z, DVec3::Y).unwrap());
        assert!(matches!(
            intersect(&a, &flipped, DVec3::ZERO),
            Ssi::Coincident
        ));
    }

    #[test]
    fn plane_and_cylinder() {
        let c = cyl(DVec3::new(1.0, 2.0, 0.0), DQuat::from_rotation_z(0.3), 2.0);
        // Perpendicular: a circle whose parameter is the cylinder angle.
        let top = Surface::Plane(
            Plane::from_origin_normal_x(DVec3::Z * 5.0, DVec3::Z, DVec3::X).unwrap(),
        );
        let k = curves(&c, &top);
        assert!(matches!(k[0], Curve3::Circle(_)));
        assert!((c.param(k[0].point(1.0)).x - 1.0).abs() < 1e-12);
        // Oblique: an ellipse.
        let slanted = Surface::Plane(
            Plane::from_origin_normal_x(DVec3::Z * 5.0, DVec3::new(0.3, -0.2, 1.0), DVec3::X)
                .unwrap(),
        );
        let k = curves(&c, &slanted);
        assert!(
            matches!(k[0], Curve3::Ellipse(e) if e.major > e.minor && (e.minor - 2.0).abs() < 1e-12)
        );
        // Parallel to the axis: two lines, one tangent line, nothing.
        let side = |x: f64| {
            Surface::Plane(Plane::from_origin_normal_x(DVec3::X * x, DVec3::X, DVec3::Y).unwrap())
        };
        assert_eq!(curves(&c, &side(2.0)).len(), 2);
        assert_eq!(curves(&c, &side(3.0)).len(), 1);
        assert_eq!(curves(&c, &side(-1.0)).len(), 1);
        assert!(matches!(intersect(&c, &side(3.5), DVec3::ZERO), Ssi::None));
    }

    #[test]
    fn cylinders() {
        let a = cyl(DVec3::ZERO, DQuat::IDENTITY, 2.0);
        let at = |x: f64, r: f64| cyl(DVec3::new(x, 0.0, 7.0), DQuat::from_rotation_z(1.0), r);
        assert_eq!(curves(&a, &at(3.0, 2.0)).len(), 2);
        assert_eq!(curves(&a, &at(4.0, 2.0)).len(), 1);
        assert_eq!(curves(&a, &at(1.0, 1.0)).len(), 1);
        assert_eq!(curves(&a, &at(-1.0, 3.0)).len(), 1);
        assert!(matches!(
            intersect(&a, &at(5.0, 2.0), DVec3::ZERO),
            Ssi::None
        ));
        assert!(matches!(
            intersect(&a, &at(0.5, 1.0), DVec3::ZERO),
            Ssi::None
        ));
        assert!(matches!(
            intersect(&a, &at(0.0, 1.0), DVec3::ZERO),
            Ssi::None
        ));
        assert!(matches!(
            intersect(&a, &at(0.0, 2.0), DVec3::ZERO),
            Ssi::Coincident
        ));
        let crossing = cyl(DVec3::ZERO, DQuat::from_rotation_x(1.0), 1.0);
        assert!(matches!(
            intersect(&a, &crossing, DVec3::ZERO),
            Ssi::Unsupported
        ));
    }
}
