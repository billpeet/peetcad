//! Hand-built solids with features extrusion alone can't produce (ellipse edges, a hole
//! loop in a cylinder face), for the validation, measurement and tessellation tests.

use std::f64::consts::{PI, TAU};

use peet_math::{DQuat, DVec3, Frame, Plane};

use crate::Solid;
use crate::geom::{Circle3, Curve3, Cylinder, Ellipse3, Surface};

fn z_frame(origin: DVec3) -> Frame {
    Frame {
        origin,
        rotation: DQuat::IDENTITY,
    }
}

/// A cylinder (axis Z, radius `r`) from `z = 0` up to the oblique plane
/// `z = h + x tan(tilt)`: its top edge is an ellipse. Volume `π r² h`.
/// Faces: 0 side, 1 oblique cap, 2 base.
pub fn oblique_cylinder(r: f64, h: f64, tilt: f64) -> Solid {
    let mut s = Solid::new();
    let shell = s.add_shell();
    let vb = s.add_vertex(DVec3::new(r, 0.0, 0.0));
    let vt = s.add_vertex(DVec3::new(r, 0.0, h + r * tilt.tan()));
    let bottom = s.add_edge(
        Curve3::Circle(Circle3 {
            frame: z_frame(DVec3::ZERO),
            radius: r,
        }),
        vb,
        vb,
        0.0,
        TAU,
    );
    let (sin, cos) = tilt.sin_cos();
    let top_frame = Frame::from_origin_z_x(
        DVec3::new(0.0, 0.0, h),
        DVec3::new(-sin, 0.0, cos),
        DVec3::new(cos, 0.0, sin),
    )
    .expect("valid frame");
    let top = s.add_edge(
        Curve3::Ellipse(Ellipse3 {
            frame: top_frame,
            major: r / cos,
            minor: r,
        }),
        vt,
        vt,
        0.0,
        TAU,
    );
    let seam = s.add_line_edge(vb, vt);
    let side = s.add_face(
        shell,
        Surface::Cylinder(Cylinder {
            frame: z_frame(DVec3::ZERO),
            radius: r,
        }),
        false,
    );
    s.add_loop(
        side,
        &[(bottom, false), (seam, false), (top, true), (seam, true)],
    );
    let cap = s.add_face(shell, Surface::Plane(Plane { frame: top_frame }), false);
    s.add_loop(cap, &[(top, false)]);
    let base = s.add_face(
        shell,
        Surface::Plane(Plane {
            frame: z_frame(DVec3::ZERO),
        }),
        true,
    );
    s.add_loop(base, &[(bottom, true)]);
    s
}

/// Volume of [`pocketed_cylinder`].
pub fn pocketed_cylinder_volume() -> f64 {
    let pocket_area = 2.0 * 21f64.sqrt() + 25.0 * 0.4f64.asin() - 12.0;
    PI * 25.0 * 10.0 - 4.0 * pocket_area
}

/// A cylinder (axis Z, radius 5, height 10, seam at angle π) with a rectangular pocket cut
/// into its side: `x ≥ 3`, `|y| ≤ 2`, `3 ≤ z ≤ 7`. The cylinder face (face 0) has a hole
/// loop (two arcs and two lines) a whole turn away from where its outer loop starts.
/// Vertices 0 and 1 are the seam's ends; edges 0, 1, 2 the bottom circle, top circle, seam.
pub fn pocketed_cylinder() -> Solid {
    let (r, h) = (5.0, 10.0);
    let mut s = Solid::new();
    let shell = s.add_shell();
    let circle = |z: f64| {
        Curve3::Circle(Circle3 {
            frame: z_frame(DVec3::new(0.0, 0.0, z)),
            radius: r,
        })
    };
    // The plain cylinder, seam at angle π.
    let vb = s.add_vertex(DVec3::new(-r, 0.0, 0.0));
    let vt = s.add_vertex(DVec3::new(-r, 0.0, h));
    let bottom = s.add_edge(circle(0.0), vb, vb, PI, 3.0 * PI);
    let top = s.add_edge(circle(h), vt, vt, PI, 3.0 * PI);
    let seam = s.add_line_edge(vb, vt);
    // Pocket corners: A on the cylinder, B on the pocket floor (x = 3).
    let phi = (2.0f64 / r).asin();
    let xa = r * phi.cos();
    let a1 = s.add_vertex(DVec3::new(xa, -2.0, 3.0));
    let a2 = s.add_vertex(DVec3::new(xa, 2.0, 3.0));
    let a3 = s.add_vertex(DVec3::new(xa, 2.0, 7.0));
    let a4 = s.add_vertex(DVec3::new(xa, -2.0, 7.0));
    let b1 = s.add_vertex(DVec3::new(3.0, -2.0, 3.0));
    let b2 = s.add_vertex(DVec3::new(3.0, 2.0, 3.0));
    let b3 = s.add_vertex(DVec3::new(3.0, 2.0, 7.0));
    let b4 = s.add_vertex(DVec3::new(3.0, -2.0, 7.0));
    let arc_lo = s.add_edge(circle(3.0), a1, a2, -phi, phi);
    let arc_hi = s.add_edge(circle(7.0), a4, a3, -phi, phi);
    let line_l = s.add_line_edge(a1, a4);
    let line_r = s.add_line_edge(a2, a3);
    let ab1 = s.add_line_edge(a1, b1);
    let ab2 = s.add_line_edge(a2, b2);
    let ab3 = s.add_line_edge(a3, b3);
    let ab4 = s.add_line_edge(a4, b4);
    let b12 = s.add_line_edge(b1, b2);
    let b23 = s.add_line_edge(b2, b3);
    let b43 = s.add_line_edge(b4, b3);
    let b14 = s.add_line_edge(b1, b4);

    let side = s.add_face(
        shell,
        Surface::Cylinder(Cylinder {
            frame: z_frame(DVec3::ZERO),
            radius: r,
        }),
        false,
    );
    s.add_loop(
        side,
        &[(bottom, false), (seam, false), (top, true), (seam, true)],
    );
    s.add_loop(
        side,
        &[
            (line_l, false),
            (arc_hi, false),
            (line_r, true),
            (arc_lo, true),
        ],
    );
    let cap = s.add_face(
        shell,
        Surface::Plane(Plane {
            frame: z_frame(DVec3::new(0.0, 0.0, h)),
        }),
        false,
    );
    s.add_loop(cap, &[(top, false)]);
    let base = s.add_face(
        shell,
        Surface::Plane(Plane {
            frame: z_frame(DVec3::ZERO),
        }),
        true,
    );
    s.add_loop(base, &[(bottom, true)]);

    let plane = |origin: DVec3, normal: DVec3, x: DVec3| {
        Surface::Plane(Plane::from_origin_normal_x(origin, normal, x).expect("valid plane"))
    };
    let at = |s: &Solid, v| s.vertex(v).point;
    // Pocket floor (faces +X), walls at z = 3 (faces up), z = 7 (faces down), y = ±2.
    let surface = plane(at(&s, b1), DVec3::X, DVec3::Y);
    let floor = s.add_face(shell, surface, false);
    s.add_loop(
        floor,
        &[(b12, false), (b23, false), (b43, true), (b14, true)],
    );
    let surface = plane(at(&s, b1), DVec3::Z, DVec3::X);
    let lower = s.add_face(shell, surface, false);
    s.add_loop(
        lower,
        &[(ab1, true), (arc_lo, false), (ab2, false), (b12, true)],
    );
    let surface = plane(at(&s, b4), -DVec3::Z, DVec3::X);
    let upper = s.add_face(shell, surface, false);
    s.add_loop(
        upper,
        &[(b43, false), (ab3, true), (arc_hi, true), (ab4, false)],
    );
    let surface = plane(at(&s, b2), -DVec3::Y, DVec3::X);
    let right = s.add_face(shell, surface, false);
    s.add_loop(
        right,
        &[(ab2, true), (line_r, false), (ab3, false), (b23, true)],
    );
    let surface = plane(at(&s, b1), DVec3::Y, DVec3::X);
    let left = s.add_face(shell, surface, false);
    s.add_loop(
        left,
        &[(ab1, false), (b14, false), (ab4, true), (line_l, true)],
    );
    s
}
