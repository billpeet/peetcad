//! Surface–surface intersection for planes, cylinders, cones, spheres and tori, as exact
//! analytic curves (lines, circles and ellipses).

use std::f64::consts::PI;

use peet_math::tolerance::{self, ANGULAR, LINEAR};
use peet_math::{DVec2, DVec3, Frame, Plane};

use std::sync::Arc;

use crate::geom::{Circle3, Cone, Curve3, Cylinder, Ellipse3, Line3, Sphere, Surface, Torus};
use crate::nurbs::{NurbsCurve, NurbsSurface};

/// How two unbounded surfaces meet.
#[derive(Clone, Debug)]
pub(crate) enum Ssi {
    /// They don't meet.
    None,
    /// They are the same surface.
    Coincident,
    /// They meet in these curves (lines, or one circle or ellipse).
    Curves(Vec<Curve3>),
    /// They may meet in a curve the kernel doesn't represent: a quartic (cylinders with
    /// crossing axes), a parabola or hyperbola (a plane along a cone), a spiric section
    /// (a plane cutting a torus at an angle).
    Unsupported,
}

/// Intersects two surfaces. Lines get their origin near `near` so parameters stay small.
///
/// Besides the plane and cylinder cases, every pair of surfaces of revolution about one
/// common axis meets in circles around that axis ([`coaxial`]); a plane meets a sphere
/// in a circle, a cone in a circle, an ellipse or rulings, and a torus in circles when it
/// is square to the axis or contains it.
pub(crate) fn intersect(a: &Surface, b: &Surface, near: DVec3) -> Ssi {
    intersect_in(a, b, near, None)
}

/// [`intersect`], for freeform surfaces only looking within `region`: their curves are
/// traced, and tracing all of a large surface for a small face would be wasted.
pub(crate) fn intersect_in(
    a: &Surface,
    b: &Surface,
    near: DVec3,
    region: Option<&peet_math::Aabb>,
) -> Ssi {
    let found = closed_form(a, b, near, region);
    if matches!(found, Ssi::Unsupported) {
        // No closed form (cylinders that cross askew, a plane along a cone): the curve
        // is traced on one of the two written as a freeform surface.
        return marched(a, b, region);
    }
    found
}

/// The part of a cylinder or a cone near `region`, written exactly as a freeform
/// surface: a full turn between two circles, with its seam on the side away from the
/// region. `None` for other surfaces, and for a cone whose tip is in reach.
fn patch(surface: &Surface, region: &peet_math::Aabb) -> Option<NurbsSurface> {
    let (frame, radius_at): (&Frame, Box<dyn Fn(f64) -> f64>) = match surface {
        Surface::Cylinder(c) => {
            let r = c.radius;
            (&c.frame, Box::new(move |_| r))
        }
        Surface::Cone(c) => {
            let (r, slope) = (c.radius, c.half_angle.tan());
            (&c.frame, Box::new(move |z| r + z * slope))
        }
        _ => return None,
    };
    let size = region.size().length();
    let mut range = (f64::INFINITY, f64::NEG_INFINITY);
    for i in 0..8 {
        let corner = DVec3::new(
            if i & 1 == 0 {
                region.min.x
            } else {
                region.max.x
            },
            if i & 2 == 0 {
                region.min.y
            } else {
                region.max.y
            },
            if i & 4 == 0 {
                region.min.z
            } else {
                region.max.z
            },
        );
        let z = frame.to_local(corner).z;
        range = (range.0.min(z), range.1.max(z));
    }
    let margin = 0.05 * size + 1e-3;
    let (z0, z1) = (range.0 - margin, range.1 + margin);
    if radius_at(z0) <= LINEAR || radius_at(z1) <= LINEAR {
        return None;
    }
    let middle = frame.to_local(0.5 * (region.min + region.max));
    let away = middle.y.atan2(middle.x) + std::f64::consts::PI;
    let ring = |z: f64| {
        NurbsCurve::arc(
            &Frame {
                origin: frame.to_world(DVec3::new(0.0, 0.0, z)),
                rotation: frame.rotation,
            },
            radius_at(z),
            away,
            std::f64::consts::TAU,
        )
    };
    NurbsSurface::skin(&[ring(z0), ring(z1)]).ok()
}

thread_local! {
    /// Whether an intersection of analytic surfaces has been traced on this thread since
    /// [`take_traced`] was last called.
    static TRACED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
thread_local! {
    static TRACINGS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many intersections of analytic surfaces have been traced on this thread, for
/// tests that expect exact results where nothing was.
#[cfg(test)]
pub(super) fn tracings() -> usize {
    TRACINGS.with(std::cell::Cell::get)
}

/// Whether an intersection of two analytic surfaces was traced (it had no closed form)
/// since the last call. Tracing can't follow faces that only touch, or that cross
/// exactly at an edge, so what fails after it is reported as unsupported.
pub(super) fn take_traced() -> bool {
    TRACED.with(|t| t.replace(false))
}

/// The intersection of two analytic surfaces that has no closed form, traced within
/// `region`. Without a region there is nowhere to look.
fn marched(a: &Surface, b: &Surface, region: Option<&peet_math::Aabb>) -> Ssi {
    let Some(region) = region else {
        return Ssi::Unsupported;
    };
    for (s, other) in [(a, b), (b, a)] {
        if let Some(written) = patch(s, region) {
            TRACED.with(|t| t.set(true));
            #[cfg(test)]
            TRACINGS.with(|t| t.set(t.get() + 1));
            return super::freeform::intersect(&Arc::new(written), other, Some(region));
        }
    }
    Ssi::Unsupported
}

fn closed_form(a: &Surface, b: &Surface, near: DVec3, region: Option<&peet_math::Aabb>) -> Ssi {
    match (a, b) {
        (Surface::Nurbs(s), other) | (other, Surface::Nurbs(s)) => {
            super::freeform::intersect(s, other, region)
        }
        (Surface::Plane(p), Surface::Plane(q)) => plane_plane(p, q, near),
        (Surface::Plane(p), Surface::Cylinder(c)) | (Surface::Cylinder(c), Surface::Plane(p)) => {
            plane_cylinder(p, c, near)
        }
        (Surface::Cylinder(c), Surface::Cylinder(d)) => cylinder_cylinder(c, d, near),
        (Surface::Plane(p), Surface::Sphere(s)) | (Surface::Sphere(s), Surface::Plane(p)) => {
            plane_sphere(p, s)
        }
        (Surface::Plane(p), Surface::Cone(c)) | (Surface::Cone(c), Surface::Plane(p)) => {
            plane_cone(p, c, near)
        }
        (Surface::Plane(p), Surface::Torus(t)) | (Surface::Torus(t), Surface::Plane(p)) => {
            plane_torus(p, t)
        }
        _ => coaxial(a, b),
    }
}

fn circle(origin: DVec3, rotation: peet_math::DQuat, radius: f64) -> Curve3 {
    Curve3::Circle(Circle3 {
        frame: Frame { origin, rotation },
        radius,
    })
}

fn plane_sphere(p: &Plane, s: &Sphere) -> Ssi {
    let d = p.signed_distance(s.frame.origin);
    // Touching in a point leaves no curve.
    if d.abs() >= s.radius - LINEAR {
        return Ssi::None;
    }
    let radius = (s.radius * s.radius - d * d).sqrt();
    if radius <= LINEAR {
        return Ssi::None;
    }
    let center = s.frame.origin - p.normal() * d;
    // A plane square to the sphere's axis gives a circle of latitude, with the sphere's
    // own angle as its parameter.
    if tolerance::directions_parallel(p.normal(), s.frame.z_axis()) {
        return Ssi::Curves(vec![circle(center, s.frame.rotation, radius)]);
    }
    Ssi::Curves(vec![circle(center, p.frame.rotation, radius)])
}

/// A plane through a cone's apex within this of touching it (as a cosine) meets it in
/// one ruling.
const TANGENT_RULING: f64 = 1e-9;

fn plane_cone(p: &Plane, c: &Cone, near: DVec3) -> Ssi {
    let (axis, k) = (c.axis(), c.slope());
    let n = c.frame.vector_to_local(p.normal());
    let nh = n.x.hypot(n.y);
    if nh <= ANGULAR {
        // Square to the axis: a circle, unless the plane passes the apex or beyond it.
        let v = (p.origin() - c.axis_origin()).dot(axis);
        let radius = c.radius_at(v);
        return if radius > LINEAR {
            Ssi::Curves(vec![circle(
                c.axis_origin() + axis * v,
                c.frame.rotation,
                radius,
            )])
        } else {
            Ssi::None
        };
    }
    // In the cone's frame with the apex at the origin: n · x = d.
    let d = p.signed_distance(c.apex());
    let sign = k.signum();
    if d.abs() <= LINEAR {
        // Through the apex: the rulings in the plane, at angles u with
        // nx cos u + ny sin u = −nz / k.
        let cos = -n.z / (k * nh);
        let phi = n.y.atan2(n.x);
        // Touching along one ruling: within what the plane's and the cone's own
        // directions are good for (a drafted wall next to a drafted round corner).
        let angles = if (cos.abs() - 1.0).abs() <= TANGENT_RULING {
            vec![if cos > 0.0 { phi } else { phi + PI }]
        } else if cos.abs() < 1.0 {
            let w = cos.acos();
            vec![phi - w, phi + w]
        } else {
            return Ssi::None;
        };
        let apex = c.apex();
        return Ssi::Curves(
            angles
                .into_iter()
                .map(|u| {
                    let (s, co) = u.sin_cos();
                    let dir = c
                        .frame
                        .vector_to_world(DVec3::new(k * co, k * s, 1.0).normalize() * sign);
                    line(apex, dir, near)
                })
                .collect(),
        );
    }
    // A plane steeper than the rulings cuts every ruling of one nappe: an ellipse.
    if n.z.abs() <= nh * k.abs() * (1.0 + 1e-9) {
        return Ssi::Unsupported;
    }
    // Rotate about the axis so the normal is (nh, 0, nz). The ellipse's long axis is in
    // that plane of symmetry, between the rulings at u = 0 and u = π.
    let z1 = -d / (n.z + nh * k);
    let z2 = -d / (n.z - nh * k);
    if z1 * sign <= 0.0 {
        return Ssi::None; // on the other nappe
    }
    let (p1, p2) = (DVec3::new(z1 * k, 0.0, z1), DVec3::new(-z2 * k, 0.0, z2));
    let center = 0.5 * (p1 + p2);
    let a = 0.5 * p1.distance(p2);
    let b2 = center.z * center.z * k * k - center.x * center.x;
    if b2 <= 0.0 || a <= LINEAR {
        return Ssi::None;
    }
    let b = b2.sqrt();
    // Back to the cone's frame, then to the world.
    let turn = DVec3::new(n.x / nh, n.y / nh, 0.0);
    let side = DVec3::new(-turn.y, turn.x, 0.0);
    let to_world = |q: DVec3| {
        c.frame
            .to_world(turn * q.x + side * q.y + DVec3::new(0.0, 0.0, q.z + c.apex_v()))
    };
    let along = (to_world(p1) - to_world(p2)).normalize();
    let across = c.frame.vector_to_world(side);
    let (major, minor, x) = if a >= b {
        (a, b, along)
    } else {
        (b, a, across)
    };
    match Frame::from_origin_z_x(to_world(center), p.normal(), x) {
        Some(frame) => Ssi::Curves(vec![Curve3::Ellipse(Ellipse3 {
            frame,
            major,
            minor,
        })]),
        None => Ssi::None,
    }
}

fn plane_torus(p: &Plane, t: &Torus) -> Ssi {
    let axis = t.frame.z_axis();
    let n = p.normal();
    let h = p.signed_distance(t.frame.origin);
    let nh = n.cross(axis).length();
    // The torus reaches this far towards the plane.
    if h.abs() > t.major * nh + t.minor + LINEAR {
        return Ssi::None;
    }
    if nh <= ANGULAR {
        // Square to the axis, at height `z` above the torus's middle plane.
        let z = -h * n.dot(axis).signum();
        let center = t.frame.origin + axis * z;
        if (z.abs() - t.minor).abs() <= LINEAR {
            return Ssi::Curves(vec![circle(center, t.frame.rotation, t.major)]);
        }
        let w = (t.minor * t.minor - z * z).max(0.0).sqrt();
        return Ssi::Curves(
            [t.major + w, t.major - w]
                .into_iter()
                .filter(|&r| r > LINEAR)
                .map(|r| circle(center, t.frame.rotation, r))
                .collect(),
        );
    }
    if n.dot(axis).abs() <= ANGULAR && h.abs() <= LINEAR {
        // Containing the axis: the two circles of the tube, parametrised by the tube's
        // angle `v` (x away from the axis, y along it).
        let out = axis.cross(n).normalize();
        return Ssi::Curves(
            [out, -out]
                .into_iter()
                .filter_map(|e| {
                    let frame =
                        Frame::from_origin_z_x(t.frame.origin + e * t.major, e.cross(axis), e)?;
                    Some(Curve3::Circle(Circle3 {
                        frame,
                        radius: t.minor,
                    }))
                })
                .collect(),
        );
    }
    Ssi::Unsupported
}

/// Cylinders whose axes are not parallel. In general they meet in a quartic curve, but
/// two of the same radius whose axes cross meet in two ellipses, in the planes through
/// the crossing point that halve the angles between the axes: the mitres where two
/// fillets of one radius meet, or two pipes of one bore.
fn crossing_cylinders(c: &Cylinder, d: &Cylinder) -> Ssi {
    let (a1, a2) = (c.axis(), d.axis());
    let normal = a1.cross(a2);
    let w = d.axis_origin() - c.axis_origin();
    let gap = w.dot(normal) / normal.length();
    if gap.abs() > LINEAR || (c.radius - d.radius).abs() > LINEAR {
        return Ssi::Unsupported;
    }
    // Where the axes cross.
    let s = w.cross(a2).dot(normal) / normal.length_squared();
    let center = c.axis_origin() + a1 * s;
    let mut curves = Vec::with_capacity(2);
    for (plane_normal, along) in [(a1 - a2, a1 + a2), (a1 + a2, a1 - a2)] {
        let along = along.normalize();
        // A point this far along the mitre is one radius from each axis.
        let sin = along.cross(a1).length();
        let Some(frame) = Frame::from_origin_z_x(center, plane_normal, along) else {
            return Ssi::Unsupported;
        };
        curves.push(Curve3::Ellipse(Ellipse3 {
            frame,
            major: c.radius / sin,
            minor: c.radius,
        }));
    }
    Ssi::Curves(curves)
}

/// The meridian of a surface of revolution in the `(ρ, z)` half-plane of a common axis.
#[derive(Clone, Copy, Debug)]
enum Profile {
    /// A straight meridian through `at` with unit direction `dir` (cylinder, cone).
    Line { at: DVec2, dir: DVec2 },
    /// A circular meridian (sphere: centred on the axis; torus).
    Circle { center: DVec2, radius: f64 },
}

/// The axis two surfaces of revolution share, as a frame, if they do: both axes on one
/// line. A sphere has every line through its centre as an axis.
fn common_axis(a: &Surface, b: &Surface) -> Option<Frame> {
    let fixed = |s: &Surface| match s {
        Surface::Sphere(_) | Surface::Plane(_) => None,
        _ => s.revolution_frame().copied(),
    };
    let on_axis = |f: &Frame, p: DVec3| {
        let w = p - f.origin;
        (w - f.z_axis() * w.dot(f.z_axis())).length() <= LINEAR
    };
    match (fixed(a), fixed(b)) {
        (Some(fa), Some(fb)) => (tolerance::directions_parallel(fa.z_axis(), fb.z_axis())
            && on_axis(&fa, fb.origin))
        .then_some(fa),
        (Some(f), None) => on_axis(&f, b.revolution_frame()?.origin).then_some(f),
        (None, Some(f)) => on_axis(&f, a.revolution_frame()?.origin).then_some(f),
        (None, None) => {
            // Two spheres: the line through their centres (any, if they are concentric).
            let (fa, fb) = (a.revolution_frame()?, b.revolution_frame()?);
            let z = (fb.origin - fa.origin)
                .try_normalize()
                .unwrap_or(fa.z_axis());
            Frame::from_origin_z_x(fa.origin, z, z.any_orthonormal_vector())
        }
    }
}

/// The meridian of `s` in the `(ρ, z)` coordinates of `axis`.
fn profile(s: &Surface, axis: &Frame) -> Option<Profile> {
    let frame = s.revolution_frame()?;
    let z0 = (frame.origin - axis.origin).dot(axis.z_axis());
    // The surface's own axis may point the other way.
    let flip = frame.z_axis().dot(axis.z_axis()).signum();
    Some(match s {
        Surface::Plane(_) | Surface::Nurbs(_) => return None,
        Surface::Cylinder(c) => Profile::Line {
            at: DVec2::new(c.radius, z0),
            dir: DVec2::Y,
        },
        Surface::Cone(c) => Profile::Line {
            at: DVec2::new(c.radius, z0),
            dir: DVec2::new(c.slope(), flip).normalize(),
        },
        Surface::Sphere(s) => Profile::Circle {
            center: DVec2::new(0.0, z0),
            radius: s.radius,
        },
        Surface::Torus(t) => Profile::Circle {
            center: DVec2::new(t.major, z0),
            radius: t.minor,
        },
    })
}

/// Surfaces of revolution about one axis meet in circles around it: one for each point
/// where their meridians meet.
fn coaxial(a: &Surface, b: &Surface) -> Ssi {
    let Some(axis) = common_axis(a, b) else {
        // Far apart is still an answer.
        return if apart(a, b) {
            Ssi::None
        } else {
            Ssi::Unsupported
        };
    };
    let (Some(pa), Some(pb)) = (profile(a, &axis), profile(b, &axis)) else {
        return Ssi::Unsupported;
    };
    let points = match (pa, pb) {
        (Profile::Line { at, dir }, Profile::Line { at: bt, dir: bd }) => {
            let cross = dir.perp_dot(bd);
            if cross.abs() <= ANGULAR {
                return if (bt - at).perp_dot(dir).abs() <= LINEAR {
                    Ssi::Coincident
                } else {
                    Ssi::None
                };
            }
            vec![at + dir * ((bt - at).perp_dot(bd) / cross)]
        }
        (Profile::Line { at, dir }, Profile::Circle { center, radius })
        | (Profile::Circle { center, radius }, Profile::Line { at, dir }) => {
            let along = (center - at).dot(dir);
            let foot = at + dir * along;
            let h = foot.distance(center);
            if (h - radius).abs() <= LINEAR {
                vec![foot]
            } else if h > radius {
                Vec::new()
            } else {
                let w = (radius * radius - h * h).sqrt();
                vec![foot - dir * w, foot + dir * w]
            }
        }
        (
            Profile::Circle { center, radius },
            Profile::Circle {
                center: c2,
                radius: r2,
            },
        ) => {
            let dist = center.distance(c2);
            if dist <= LINEAR {
                return if (radius - r2).abs() <= LINEAR {
                    Ssi::Coincident
                } else {
                    Ssi::None
                };
            }
            let u = (c2 - center) / dist;
            if dist > radius + r2 + LINEAR || dist < (radius - r2).abs() - LINEAR {
                Vec::new()
            } else if (dist - (radius + r2)).abs() <= LINEAR {
                vec![center + u * radius]
            } else if (dist - (radius - r2).abs()).abs() <= LINEAR {
                let s = if radius >= r2 { 1.0 } else { -1.0 };
                vec![center + u * (radius * s)]
            } else {
                let along = (radius * radius - r2 * r2 + dist * dist) / (2.0 * dist);
                let h = (radius * radius - along * along).max(0.0).sqrt();
                vec![
                    center + u * along + u.perp() * h,
                    center + u * along - u.perp() * h,
                ]
            }
        }
    };
    // Each meeting point off the axis is a circle (a torus's meridian circle also has a
    // mirror image across the axis, which is the same circle).
    let mut circles: Vec<Curve3> = Vec::new();
    for q in points {
        let rho = q.x.abs();
        if rho <= LINEAR {
            continue;
        }
        let origin = axis.origin + axis.z_axis() * q.y;
        let known = circles.iter().any(|c| {
            matches!(c, Curve3::Circle(k)
                if k.frame.origin.distance(origin) <= LINEAR && (k.radius - rho).abs() <= LINEAR)
        });
        if !known {
            circles.push(circle(origin, axis.rotation, rho));
        }
    }
    // Only the part of each surface at a positive distance from its axis exists.
    circles.retain(|c| {
        let at = c.point(0.0);
        a.signed_distance(at).abs() <= 10.0 * LINEAR && b.signed_distance(at).abs() <= 10.0 * LINEAR
    });
    if circles.is_empty() {
        Ssi::None
    } else {
        Ssi::Curves(circles)
    }
}

/// Whether two surfaces are certainly too far apart to meet: bounded ones by their
/// bounding spheres.
fn apart(a: &Surface, b: &Surface) -> bool {
    let ball = |s: &Surface| match s {
        Surface::Sphere(s) => Some((s.frame.origin, s.radius)),
        Surface::Torus(t) => Some((t.frame.origin, t.major + t.minor)),
        _ => None,
    };
    match (ball(a), ball(b)) {
        (Some((ca, ra)), Some((cb, rb))) => ca.distance(cb) > ra + rb + LINEAR,
        (Some((c, r)), None) => b.signed_distance(c) > r + LINEAR,
        (None, Some((c, r))) => a.signed_distance(c) > r + LINEAR,
        (None, None) => false,
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
        return crossing_cylinders(c, d);
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
