//! Analytic geometry: surfaces (plane, cylinder) and 3D curves (line, circle, ellipse).
//!
//! Everything is exact `f64` geometry in model space (mm). Topology (which part of a
//! surface is a face, which part of a curve is an edge) lives in [`crate::topo`].
//!
//! **Parameters.**
//! - Plane: `(u, v)` are the plane frame's local X/Y coordinates.
//! - Cylinder: `u` is the angle around the axis in radians (0 along the frame's X axis,
//!   counter-clockwise about +Z), `v` the height along the axis from the frame origin.
//!   [`Surface::param`] returns `u` in `(-π, π]`; callers working across the seam unwrap it.
//! - Line: `t` is the distance from `origin` along the unit `dir`.
//! - Circle: `t` is the angle in radians from the frame's X axis, counter-clockwise about +Z.
//! - Ellipse: `point(t) = center + x·a·cos t + y·b·sin t`.

use std::f64::consts::TAU;

use peet_math::{DVec2, DVec3, Frame, Plane};
use serde::{Deserialize, Serialize};

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

/// An unbounded analytic surface. Its natural normal points away from the plane's front
/// (along the frame's Z) or radially outwards from a cylinder's axis.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Surface {
    Plane(Plane),
    Cylinder(Cylinder),
}

impl Surface {
    /// The point at surface parameters `uv`.
    pub fn point(&self, uv: DVec2) -> DVec3 {
        match self {
            Self::Plane(p) => p.from_plane_coords(uv),
            Self::Cylinder(c) => {
                let (s, co) = uv.x.sin_cos();
                c.frame
                    .to_world(DVec3::new(co * c.radius, s * c.radius, uv.y))
            }
        }
    }

    /// Parameters of the closest point on the surface to `p`. Cylinder `u` is in `(-π, π]`.
    pub fn param(&self, p: DVec3) -> DVec2 {
        match self {
            Self::Plane(pl) => pl.to_plane_coords(p),
            Self::Cylinder(c) => {
                let l = c.frame.to_local(p);
                DVec2::new(l.y.atan2(l.x), l.z)
            }
        }
    }

    /// The natural (unoriented-face) normal at the surface point closest to `p`.
    pub fn normal_at(&self, p: DVec3) -> DVec3 {
        match self {
            Self::Plane(pl) => pl.normal(),
            Self::Cylinder(c) => {
                let l = c.frame.to_local(p);
                let radial = DVec3::new(l.x, l.y, 0.0)
                    .try_normalize()
                    .unwrap_or(DVec3::X);
                c.frame.vector_to_world(radial)
            }
        }
    }

    /// Normal at parameters `uv`.
    pub fn normal(&self, uv: DVec2) -> DVec3 {
        match self {
            Self::Plane(pl) => pl.normal(),
            Self::Cylinder(c) => {
                let (s, co) = uv.x.sin_cos();
                c.frame.vector_to_world(DVec3::new(co, s, 0.0))
            }
        }
    }

    /// Signed distance from `p`: positive on the normal side (in front of a plane,
    /// outside a cylinder).
    pub fn signed_distance(&self, p: DVec3) -> f64 {
        match self {
            Self::Plane(pl) => pl.signed_distance(p),
            Self::Cylinder(c) => {
                let l = c.frame.to_local(p);
                DVec2::new(l.x, l.y).length() - c.radius
            }
        }
    }

    /// The closest point on the surface to `p`.
    pub fn project(&self, p: DVec3) -> DVec3 {
        self.point(self.param(p))
    }

    /// Whether `u` wraps around (cylinders).
    pub fn is_periodic_u(&self) -> bool {
        matches!(self, Self::Cylinder(_))
    }

    /// Partial derivatives `(dP/du, dP/dv)` at `uv`.
    pub fn derivatives(&self, uv: DVec2) -> (DVec3, DVec3) {
        match self {
            Self::Plane(pl) => (pl.frame.x_axis(), pl.frame.y_axis()),
            Self::Cylinder(c) => {
                let (s, co) = uv.x.sin_cos();
                (
                    c.frame
                        .vector_to_world(DVec3::new(-s * c.radius, co * c.radius, 0.0)),
                    c.axis(),
                )
            }
        }
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Curve3 {
    Line(Line3),
    Circle(Circle3),
    Ellipse(Ellipse3),
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

    /// Unit tangent at `t`, in the direction of increasing `t`.
    pub fn tangent(&self, t: f64) -> DVec3 {
        self.derivative(t).normalize_or(DVec3::X)
    }

    /// Parameter of the curve point closest to `p`. Periodic curves return `t` in
    /// `(-π, π]`.
    pub fn param(&self, p: DVec3) -> f64 {
        match self {
            Self::Line(l) => (p - l.origin).dot(l.dir),
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
            Self::Line(_) => None,
            Self::Circle(_) | Self::Ellipse(_) => Some(TAU),
        }
    }

    /// The plane the curve lies in (circles and ellipses only).
    pub fn plane(&self) -> Option<Plane> {
        match self {
            Self::Line(_) => None,
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
