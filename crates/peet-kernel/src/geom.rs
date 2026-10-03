//! Analytic geometry: surfaces (plane, cylinder, cone, sphere, torus) and 3D curves (line,
//! circle, ellipse).
//!
//! Everything is exact `f64` geometry in model space (mm). Topology (which part of a
//! surface is a face, which part of a curve is an edge) lives in [`crate::topo`].
//!
//! **Parameters.**
//! - Plane: `(u, v)` are the plane frame's local X/Y coordinates.
//! - Every other surface is a surface of revolution about its frame's Z axis: `u` is the
//!   angle around the axis in radians (0 along the frame's X axis, counter-clockwise about
//!   +Z) and `v` runs along the meridian ([`Surface::meridian`]). [`Surface::param`]
//!   returns `u` in `(-π, π]`; callers working across the seam unwrap it.
//! - Cylinder and cone: `v` is the height along the axis from the frame origin.
//! - Sphere: `v` is the latitude, from `−π/2` at the south pole to `π/2` at the north.
//! - Torus: `v` is the angle around the tube, 0 on the outer equator, `π/2` on top.
//!
//! In every case `(dP/du, dP/dv, natural normal)` is right-handed, so a loop that runs
//! counter-clockwise in `(u, v)` runs counter-clockwise seen from the natural normal side.
//! - Line: `t` is the distance from `origin` along the unit `dir`.
//! - Circle: `t` is the angle in radians from the frame's X axis, counter-clockwise about +Z.
//! - Ellipse: `point(t) = center + x·a·cos t + y·b·sin t`.

use std::f64::consts::{FRAC_PI_2, TAU};

use std::sync::Arc;

use peet_math::{DVec2, DVec3, Frame, Plane, tolerance};
use serde::{Deserialize, Serialize};

use crate::nurbs::{NurbsCurve, NurbsSurface};

/// A right circular cylinder of infinite length. The frame's Z axis is the cylinder axis
/// and its origin lies on the axis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cylinder {
    pub frame: Frame,
    pub radius: f64,
}

impl Cylinder {
    pub fn axis_origin(&self) -> DVec3 {
        self.frame.origin
    }

    pub fn axis(&self) -> DVec3 {
        self.frame.z_axis()
    }
}

/// One nappe of a right circular cone. The frame's Z axis is the cone's axis; the radius
/// is `radius` at the frame origin and changes by `tan(half_angle)` per unit of height, so
/// the apex is at height `−radius / tan(half_angle)`. Only the nappe with a non-negative
/// radius is the surface.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cone {
    pub frame: Frame,
    /// Radius at the frame origin (`v = 0`); zero puts the apex there.
    pub radius: f64,
    /// Angle between the axis and the rulings, in `(−π/2, π/2)` and not zero. Positive:
    /// the cone widens along +Z.
    pub half_angle: f64,
}

impl Cone {
    pub fn axis_origin(&self) -> DVec3 {
        self.frame.origin
    }

    pub fn axis(&self) -> DVec3 {
        self.frame.z_axis()
    }

    /// Radius change per unit of height.
    pub fn slope(&self) -> f64 {
        self.half_angle.tan()
    }

    /// Radius at height `v`.
    pub fn radius_at(&self, v: f64) -> f64 {
        self.radius + v * self.slope()
    }

    /// Height of the apex along the axis.
    pub fn apex_v(&self) -> f64 {
        -self.radius / self.slope()
    }

    pub fn apex(&self) -> DVec3 {
        self.frame.origin + self.axis() * self.apex_v()
    }
}

/// A sphere. The frame's Z axis runs through the poles of its parametrisation.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sphere {
    pub frame: Frame,
    pub radius: f64,
}

/// A torus: a circle of radius `minor` swept around the frame's Z axis at distance `major`.
/// `major` may be smaller than `minor` (the fillet of a thin rod); the surface is then the
/// part away from the axis, where the distance from the axis is positive.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Torus {
    pub frame: Frame,
    pub major: f64,
    pub minor: f64,
}

/// An unbounded analytic surface. Its natural normal points away from the plane's front
/// (along the frame's Z), radially outwards from a cylinder's or cone's axis, away from a
/// sphere's centre, or away from a torus's tube centre.
///
/// New kinds go at the end: the B-rep cache stores the variant's index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Surface {
    Plane(Plane),
    Cylinder(Cylinder),
    Cone(Cone),
    Sphere(Sphere),
    Torus(Torus),
    /// A freeform surface ([`crate::nurbs`]). Its natural normal is `∂S/∂u × ∂S/∂v`.
    /// Shared, not copied: cloning a surface is cheap for every kind.
    Nurbs(Arc<NurbsSurface>),
}

/// A point of a surface of revolution's meridian (the curve that is turned about the
/// axis): distance from the axis, height along it, and their derivatives by `v`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meridian {
    pub rho: f64,
    pub z: f64,
    pub drho: f64,
    pub dz: f64,
}

/// A singular point of a surface's parametrisation: a sphere's pole or a cone's apex,
/// where every `u` gives the same point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pole {
    /// The `v` of the pole.
    pub v: f64,
    /// Whether the surface lies below it in `v` (a sphere's north pole; the apex of a
    /// cone that narrows along +Z).
    pub top: bool,
}

impl Surface {
    /// The frame of a surface of revolution (every kind but the plane): `u` is the angle
    /// about its Z axis.
    pub fn revolution_frame(&self) -> Option<&Frame> {
        match self {
            Self::Plane(_) | Self::Nurbs(_) => None,
            Self::Cylinder(c) => Some(&c.frame),
            Self::Cone(c) => Some(&c.frame),
            Self::Sphere(s) => Some(&s.frame),
            Self::Torus(t) => Some(&t.frame),
        }
    }

    /// The meridian of a surface of revolution at `v` (`None` for planes).
    pub fn meridian(&self, v: f64) -> Option<Meridian> {
        Some(match self {
            Self::Plane(_) | Self::Nurbs(_) => return None,
            Self::Cylinder(c) => Meridian {
                rho: c.radius,
                z: v,
                drho: 0.0,
                dz: 1.0,
            },
            Self::Cone(c) => Meridian {
                rho: c.radius_at(v),
                z: v,
                drho: c.slope(),
                dz: 1.0,
            },
            Self::Sphere(s) => {
                let (sin, cos) = v.sin_cos();
                Meridian {
                    rho: s.radius * cos,
                    z: s.radius * sin,
                    drho: -s.radius * sin,
                    dz: s.radius * cos,
                }
            }
            Self::Torus(t) => {
                let (sin, cos) = v.sin_cos();
                Meridian {
                    rho: t.major + t.minor * cos,
                    z: t.minor * sin,
                    drho: -t.minor * sin,
                    dz: t.minor * cos,
                }
            }
        })
    }

    /// The point at surface parameters `uv`.
    pub fn point(&self, uv: DVec2) -> DVec3 {
        match self {
            Self::Plane(p) => p.from_plane_coords(uv),
            Self::Nurbs(s) => s.point(uv),
            _ => {
                let (Some(frame), Some(m)) = (self.revolution_frame(), self.meridian(uv.y)) else {
                    unreachable!("every other surface is a surface of revolution")
                };
                let (s, co) = uv.x.sin_cos();
                frame.to_world(DVec3::new(co * m.rho, s * m.rho, m.z))
            }
        }
    }

    /// Parameters of the closest point on the surface to `p`. Angles are in `(-π, π]`
    /// (`u` of every surface of revolution, `v` of a torus); a sphere's `v` is the
    /// latitude in `[-π/2, π/2]`.
    pub fn param(&self, p: DVec3) -> DVec2 {
        match self {
            Self::Plane(pl) => pl.to_plane_coords(p),
            Self::Nurbs(s) => s.param(p),
            Self::Cylinder(c) => {
                let l = c.frame.to_local(p);
                DVec2::new(l.y.atan2(l.x), l.z)
            }
            Self::Cone(c) => {
                let l = c.frame.to_local(p);
                let rho = l.x.hypot(l.y);
                // The foot of the perpendicular on the ruling, in the half-plane of `p`.
                let (sin, cos) = c.half_angle.sin_cos();
                let slant = (rho - c.radius) * sin + l.z * cos;
                DVec2::new(l.y.atan2(l.x), slant * cos)
            }
            Self::Sphere(s) => {
                let l = s.frame.to_local(p);
                DVec2::new(l.y.atan2(l.x), l.z.atan2(l.x.hypot(l.y)))
            }
            Self::Torus(t) => {
                let l = t.frame.to_local(p);
                DVec2::new(l.y.atan2(l.x), l.z.atan2(l.x.hypot(l.y) - t.major))
            }
        }
    }

    /// The natural (unoriented-face) normal at the surface point closest to `p`. At a
    /// cone's apex, where the surface has no normal, this is the axis direction pointing
    /// away from the cone.
    pub fn normal_at(&self, p: DVec3) -> DVec3 {
        match self {
            Self::Plane(pl) => pl.normal(),
            Self::Cone(c) if self.pole_at(p).is_some() => -c.axis() * c.half_angle.signum(),
            Self::Sphere(s) => (p - s.frame.origin)
                .try_normalize()
                .unwrap_or(s.frame.z_axis()),
            _ => self.normal(self.param(p)),
        }
    }

    /// Normal at parameters `uv`.
    pub fn normal(&self, uv: DVec2) -> DVec3 {
        match self {
            Self::Plane(pl) => pl.normal(),
            Self::Nurbs(s) => s.normal(uv),
            _ => {
                let (Some(frame), Some(m)) = (self.revolution_frame(), self.meridian(uv.y)) else {
                    unreachable!("every other surface is a surface of revolution")
                };
                let (s, co) = uv.x.sin_cos();
                let len = m.dz.hypot(m.drho);
                frame.vector_to_world(DVec3::new(co * m.dz, s * m.dz, -m.drho) / len)
            }
        }
    }

    /// Signed distance from `p`: positive on the normal side (in front of a plane,
    /// outside a cylinder, cone, sphere or torus tube).
    pub fn signed_distance(&self, p: DVec3) -> f64 {
        match self {
            Self::Plane(pl) => pl.signed_distance(p),
            Self::Nurbs(s) => {
                let uv = s.param(p);
                (p - s.point(uv)).dot(s.normal(uv))
            }
            Self::Cylinder(c) => {
                let l = c.frame.to_local(p);
                DVec2::new(l.x, l.y).length() - c.radius
            }
            Self::Cone(c) => {
                let l = c.frame.to_local(p);
                (l.x.hypot(l.y) - c.radius_at(l.z)) * c.half_angle.cos()
            }
            Self::Sphere(s) => p.distance(s.frame.origin) - s.radius,
            Self::Torus(t) => {
                let l = t.frame.to_local(p);
                (l.x.hypot(l.y) - t.major).hypot(l.z) - t.minor
            }
        }
    }

    /// The closest point on the surface to `p`.
    pub fn project(&self, p: DVec3) -> DVec3 {
        self.point(self.param(p))
    }

    /// Whether `u` wraps around (every surface of revolution).
    pub fn is_periodic_u(&self) -> bool {
        self.revolution_frame().is_some()
    }

    /// Whether `v` wraps around (tori).
    pub fn is_periodic_v(&self) -> bool {
        matches!(self, Self::Torus(_))
    }

    /// The poles of the parametrisation: a sphere's two, a cone's apex.
    pub fn poles(&self) -> [Option<Pole>; 2] {
        match self {
            Self::Sphere(_) => [
                Some(Pole {
                    v: -FRAC_PI_2,
                    top: false,
                }),
                Some(Pole {
                    v: FRAC_PI_2,
                    top: true,
                }),
            ],
            Self::Cone(c) => [
                Some(Pole {
                    v: c.apex_v(),
                    top: c.half_angle < 0.0,
                }),
                None,
            ],
            _ => [None, None],
        }
    }

    /// The pole `p` (a point of the surface) is at, if any.
    pub fn pole_at(&self, p: DVec3) -> Option<Pole> {
        let frame = match self {
            Self::Sphere(s) => &s.frame,
            Self::Cone(c) => &c.frame,
            _ => return None,
        };
        let l = frame.to_local(p);
        if l.x.hypot(l.y) > tolerance::LINEAR {
            return None;
        }
        self.poles()
            .into_iter()
            .flatten()
            .min_by(|a, b| {
                let d = |pole: &Pole| self.point(DVec2::new(0.0, pole.v)).distance_squared(p);
                d(a).total_cmp(&d(b))
            })
            .filter(|pole| {
                self.point(DVec2::new(0.0, pole.v)).distance(p) <= 16.0 * tolerance::LINEAR
            })
    }

    /// Partial derivatives `(dP/du, dP/dv)` at `uv`.
    pub fn derivatives(&self, uv: DVec2) -> (DVec3, DVec3) {
        match self {
            Self::Plane(pl) => (pl.frame.x_axis(), pl.frame.y_axis()),
            Self::Nurbs(s) => {
                let [_, su, sv, ..] = s.evaluate(uv);
                (su, sv)
            }
            _ => {
                let (Some(frame), Some(m)) = (self.revolution_frame(), self.meridian(uv.y)) else {
                    unreachable!("every other surface is a surface of revolution")
                };
                let (s, co) = uv.x.sin_cos();
                (
                    frame.vector_to_world(DVec3::new(-s * m.rho, co * m.rho, 0.0)),
                    frame.vector_to_world(DVec3::new(co * m.drho, s * m.drho, m.dz)),
                )
            }
        }
    }

    /// Curvature vector of the surface's normal section at `p` in the unit tangent
    /// direction `dir`: a point moving that way along the surface accelerates by this.
    pub fn section_curvature(&self, p: DVec3, dir: DVec3) -> DVec3 {
        if let Self::Nurbs(s) = self {
            // The second fundamental form along `dir = a Su + b Sv`.
            let uv = s.param(p);
            let [_, su, sv, suu, suv, svv] = s.evaluate(uv);
            let n = s.normal(uv);
            let (e, f, g) = (su.dot(su), su.dot(sv), sv.dot(sv));
            let det = e * g - f * f;
            if det.abs() <= f64::MIN_POSITIVE {
                return DVec3::ZERO;
            }
            let (du, dv) = (dir.dot(su), dir.dot(sv));
            let (a, b) = ((g * du - f * dv) / det, (e * dv - f * du) / det);
            return n * (suu.dot(n) * a * a + 2.0 * suv.dot(n) * a * b + svv.dot(n) * b * b);
        }
        let Some(frame) = self.revolution_frame() else {
            return DVec3::ZERO;
        };
        let uv = self.param(p);
        let Some(m) = self.meridian(uv.y) else {
            return DVec3::ZERO;
        };
        let normal = self.normal(uv);
        let (s, co) = uv.x.sin_cos();
        let around = frame.vector_to_world(DVec3::new(-s, co, 0.0));
        let radial = frame.vector_to_world(DVec3::new(co, s, 0.0));
        // Principal curvatures: around the axis, and along the meridian.
        let k_around = if m.rho > tolerance::LINEAR {
            normal.dot(radial) / m.rho
        } else {
            0.0
        };
        let k_meridian = match self {
            Self::Sphere(s) => 1.0 / s.radius,
            Self::Torus(t) => 1.0 / t.minor,
            _ => 0.0,
        };
        let a = dir.dot(around);
        -normal * (k_around * a * a + k_meridian * (1.0 - a * a).max(0.0))
    }
}

/// An infinite straight line.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line3 {
    pub origin: DVec3,
    /// Unit direction.
    pub dir: DVec3,
}

/// A full circle; edges use a parameter range of it. The frame's Z is the circle's axis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Circle3 {
    pub frame: Frame,
    pub radius: f64,
}

/// A full ellipse in the frame's XY plane, semi-axis `major` along X and `minor` along Y.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ellipse3 {
    pub frame: Frame,
    pub major: f64,
    pub minor: f64,
}

/// An unbounded 3D curve. Edges are parameter ranges of these.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Curve3 {
    Line(Line3),
    Circle(Circle3),
    Ellipse(Ellipse3),
    /// A freeform curve ([`crate::nurbs`]), shared rather than copied. Its parameter runs
    /// over its knot range.
    Nurbs(Arc<NurbsCurve>),
}

impl Curve3 {
    /// A line through two points (parameterised by distance from `a`). `None` if they
    /// coincide.
    pub fn line_through(a: DVec3, b: DVec3) -> Option<Self> {
        Some(Self::Line(Line3 {
            origin: a,
            dir: (b - a).try_normalize()?,
        }))
    }

    pub fn point(&self, t: f64) -> DVec3 {
        match self {
            Self::Line(l) => l.origin + l.dir * t,
            Self::Nurbs(c) => c.point(t),
            Self::Circle(c) => {
                let (s, co) = t.sin_cos();
                c.frame
                    .to_world(DVec3::new(co * c.radius, s * c.radius, 0.0))
            }
            Self::Ellipse(e) => {
                let (s, co) = t.sin_cos();
                e.frame.to_world(DVec3::new(co * e.major, s * e.minor, 0.0))
            }
        }
    }

    /// First derivative at `t` (not normalized).
    pub fn derivative(&self, t: f64) -> DVec3 {
        match self {
            Self::Line(l) => l.dir,
            Self::Nurbs(c) => c.evaluate(t)[1],
            Self::Circle(c) => {
                let (s, co) = t.sin_cos();
                c.frame
                    .vector_to_world(DVec3::new(-s * c.radius, co * c.radius, 0.0))
            }
            Self::Ellipse(e) => {
                let (s, co) = t.sin_cos();
                e.frame
                    .vector_to_world(DVec3::new(-s * e.major, co * e.minor, 0.0))
            }
        }
    }

    /// Second derivative at `t`.
    pub fn second_derivative(&self, t: f64) -> DVec3 {
        match self {
            Self::Line(_) => DVec3::ZERO,
            Self::Circle(c) => c.frame.origin - self.point(t),
            Self::Ellipse(e) => e.frame.origin - self.point(t),
            Self::Nurbs(c) => c.evaluate(t)[2],
        }
    }

    /// The parameter range of a curve that has ends of its own (a freeform curve).
    pub fn domain(&self) -> Option<(f64, f64)> {
        match self {
            Self::Nurbs(c) => Some(c.domain()),
            _ => None,
        }
    }

    /// Unit tangent at `t`, in the direction of increasing `t`.
    pub fn tangent(&self, t: f64) -> DVec3 {
        self.derivative(t).normalize_or(DVec3::X)
    }

    /// Parameter of the curve point closest to `p`. Periodic curves return `t` in
    /// `(-π, π]`.
    pub fn param(&self, p: DVec3) -> f64 {
        match self {
            Self::Line(l) => (p - l.origin).dot(l.dir),
            Self::Nurbs(c) => c.param(p),
            Self::Circle(c) => {
                let q = c.frame.to_local(p);
                q.y.atan2(q.x)
            }
            Self::Ellipse(e) => {
                // Start from the eccentric angle of the point scaled onto a circle, then
                // refine the closest-point condition with Newton steps.
                let q = e.frame.to_local(p);
                let mut t = (q.y / e.minor.max(1e-300)).atan2(q.x / e.major.max(1e-300));
                for _ in 0..16 {
                    let (s, co) = t.sin_cos();
                    let d = DVec2::new(co * e.major - q.x, s * e.minor - q.y);
                    let d1 = DVec2::new(-s * e.major, co * e.minor);
                    let d2 = DVec2::new(-co * e.major, -s * e.minor);
                    let f = d.dot(d1);
                    let df = d1.dot(d1) + d.dot(d2);
                    if df.abs() < 1e-300 {
                        break;
                    }
                    let step = f / df;
                    t -= step;
                    if step.abs() < 1e-15 {
                        break;
                    }
                }
                let (s, co) = t.sin_cos();
                s.atan2(co)
            }
        }
    }

    /// Period of the parameter for closed curves.
    pub fn period(&self) -> Option<f64> {
        match self {
            Self::Line(_) | Self::Nurbs(_) => None,
            Self::Circle(_) | Self::Ellipse(_) => Some(TAU),
        }
    }

    /// The plane the curve lies in (circles and ellipses only).
    pub fn plane(&self) -> Option<Plane> {
        match self {
            Self::Line(_) | Self::Nurbs(_) => None,
            Self::Circle(c) => Some(Plane { frame: c.frame }),
            Self::Ellipse(e) => Some(Plane { frame: e.frame }),
        }
    }

    /// Distance from `p` to the unbounded curve.
    pub fn distance(&self, p: DVec3) -> f64 {
        self.point(self.param(p)).distance(p)
    }
}

/// Unwraps angle `a` to the representative closest to `reference` (differs by multiples of 2π).
pub fn unwrap_angle(a: f64, reference: f64) -> f64 {
    a + ((reference - a) / TAU).round() * TAU
}

// ---- Lifting loops into parameter space ----
//
// A face on a surface of revolution is a region of the `(u, v)` plane once its loops are
// followed continuously ("lifted"): each point takes the parameters closest to those of
// the point before it. Seam edges keep full turns simply connected, and a loop through a
// pole runs along the pole's line `v = const` there, from the `u` it arrives with to the
// `u` it leaves with. The face lies to the left of its loops, so that run goes towards
// smaller `u` at a top pole and towards larger `u` at a bottom pole (the other way round
// for a face whose outward normal opposes the surface's). The boolean code, the
// tessellation and the measurements all lift loops with the helpers below, so they agree
// on what a face is.

impl Surface {
    /// Parameters of `curve` (which lies on the surface) at `t`. At a pole, where `u`
    /// means nothing, `u` is its limit along the curve towards the parameter `toward`;
    /// the pole is returned too.
    pub fn param_toward(&self, curve: &Curve3, t: f64, toward: f64) -> (DVec2, Option<Pole>) {
        let p = curve.point(t);
        let Some(pole) = self.pole_at(p) else {
            return (self.param(p), None);
        };
        let frame = self
            .revolution_frame()
            .expect("only surfaces of revolution have poles");
        // The direction in which the curve leaves the pole.
        let d = frame.vector_to_local(curve.derivative(t)) * (toward - t).signum();
        let u = if d.x.hypot(d.y) > 1e-9 * d.length() {
            d.y.atan2(d.x)
        } else {
            self.param(curve.point(t + (toward - t) * 1e-3)).x
        };
        (DVec2::new(u, pole.v), Some(pole))
    }

    /// [`Surface::param`] for a point `p` of the surface, given the parameters `near` of
    /// a point close by (the one before it along an edge). On a freeform surface that
    /// saves the search for where to start.
    pub fn param_from(&self, p: DVec3, near: DVec2) -> DVec2 {
        if let Self::Nurbs(s) = self {
            let uv = s.param_from(p, near);
            if s.point(uv).distance_squared(p) <= 1e-10 {
                return uv;
            }
        }
        self.param(p)
    }

    /// `uv` moved by whole turns so it is as close as possible to `near`.
    pub fn param_near(&self, uv: DVec2, near: DVec2) -> DVec2 {
        DVec2::new(
            if self.is_periodic_u() {
                unwrap_angle(uv.x, near.x)
            } else {
                uv.x
            },
            if self.is_periodic_v() {
                unwrap_angle(uv.y, near.y)
            } else {
                uv.y
            },
        )
    }
}

/// The `u` a loop leaves a pole with, lifted: it arrived with `u_in` and leaves in the
/// direction `u_out` (any representative). `ccw` says whether the face's loops run
/// counter-clockwise in the coordinates used (true for `(u, v)` itself unless the face is
/// reversed). Arriving and leaving along the same meridian (a seam) is a full turn.
pub fn pole_exit(u_in: f64, u_out: f64, top: bool, ccw: bool) -> f64 {
    // Angles this close are the same meridian.
    const SAME: f64 = 1e-9;
    let decreasing = top == ccw;
    let delta = if decreasing {
        u_in - u_out
    } else {
        u_out - u_in
    };
    let mut step = delta.rem_euclid(TAU);
    if step < SAME || TAU - step < SAME {
        step = TAU;
    }
    if decreasing { u_in - step } else { u_in + step }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peet_math::DQuat;

    fn close(a: DVec3, b: DVec3) -> bool {
        a.abs_diff_eq(b, 1e-12)
    }

    #[test]
    fn cylinder_round_trip() {
        let c = Surface::Cylinder(Cylinder {
            frame: Frame {
                origin: DVec3::new(1.0, 2.0, 3.0),
                rotation: DQuat::from_rotation_x(0.4),
            },
            radius: 5.0,
        });
        let uv = DVec2::new(2.5, -4.0);
        let p = c.point(uv);
        assert!(c.signed_distance(p).abs() < 1e-12);
        assert!(c.param(p).abs_diff_eq(uv, 1e-12));
        let n = c.normal(uv);
        assert!(c.signed_distance(p + n) - 1.0 < 1e-12);
        let (du, dv) = c.derivatives(uv);
        assert!(du.dot(n).abs() < 1e-12 && dv.dot(n).abs() < 1e-12);
    }

    #[test]
    fn curve_params() {
        let f = Frame::from_origin_z_x(DVec3::new(0.0, 0.0, 2.0), DVec3::Z, DVec3::X).unwrap();
        let circle = Curve3::Circle(Circle3 {
            frame: f,
            radius: 3.0,
        });
        let p = circle.point(1.2);
        assert!((circle.param(p) - 1.2).abs() < 1e-12);
        assert!(circle.derivative(0.0).abs_diff_eq(DVec3::Y * 3.0, 1e-12));
        let ellipse = Curve3::Ellipse(Ellipse3 {
            frame: f,
            major: 4.0,
            minor: 2.0,
        });
        for t in [-3.0, -1.0, 0.3, 2.9] {
            assert!((ellipse.param(ellipse.point(t)) - t).abs() < 1e-9, "{t}");
        }
        let line = Curve3::line_through(DVec3::ZERO, DVec3::new(0.0, 3.0, 4.0)).unwrap();
        assert!(close(line.point(5.0), DVec3::new(0.0, 3.0, 4.0)));
        assert_eq!(line.param(DVec3::new(9.0, 3.0, 4.0)), 5.0);
    }

    #[test]
    fn angle_unwrap() {
        assert!((unwrap_angle(-3.0, 3.0) - (-3.0 + TAU)).abs() < 1e-12);
        assert_eq!(unwrap_angle(1.0, 1.5), 1.0);
    }
}
