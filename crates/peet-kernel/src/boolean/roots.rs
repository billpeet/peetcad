//! Where a curve lying on a surface meets another curve on the same surface.
//!
//! The second curve (a face's boundary edge) is written as the zero set of an implicit
//! function restricted to the surface: a plane, a sphere or an elliptic cylinder. That
//! turns curve–curve intersection into a root search of one function of the first curve's
//! parameter, which is linear, quadratic or `A + B cos t + C sin t` in every case that
//! planes and parallel cylinders produce, so tangencies come out exactly. Only an ellipse
//! against another conic in a plane needs the numeric fallback.

use std::f64::consts::TAU;

use peet_math::tolerance::{ANGULAR, LINEAR};
use peet_math::{DVec2, DVec3, Frame};

use crate::geom::{Curve3, Surface};

/// An implicit function whose zero set, cut with the host surface, is an edge's curve.
/// Values are scaled to be about the distance from the curve near it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Implicit {
    Plane { origin: DVec3, normal: DVec3 },
    Sphere { center: DVec3, radius: f64 },
    Elliptic { frame: Frame, a: f64, b: f64 },
}

impl Implicit {
    /// The implicit form of `curve`, which lies on `host`.
    pub fn of(curve: &Curve3, host: &Surface) -> Option<Self> {
        Some(match (curve, host) {
            (Curve3::Line(l), Surface::Plane(p)) => Self::Plane {
                origin: l.origin,
                normal: p.normal().cross(l.dir).try_normalize()?,
            },
            // A ruling: the plane through it and the axis (which also contains the
            // opposite ruling; callers check hits against the edge itself).
            (Curve3::Line(l), Surface::Cylinder(c)) => Self::Plane {
                origin: c.axis_origin(),
                normal: c.axis().cross(l.origin - c.axis_origin()).try_normalize()?,
            },
            (Curve3::Circle(ci), Surface::Plane(_)) => Self::Sphere {
                center: ci.frame.origin,
                radius: ci.radius,
            },
            (Curve3::Circle(ci), Surface::Cylinder(_)) => Self::Plane {
                origin: ci.frame.origin,
                normal: ci.frame.z_axis(),
            },
            (Curve3::Ellipse(e), Surface::Cylinder(_)) => Self::Plane {
                origin: e.frame.origin,
                normal: e.frame.z_axis(),
            },
            (Curve3::Ellipse(e), Surface::Plane(_)) => Self::Elliptic {
                frame: e.frame,
                a: e.major,
                b: e.minor,
            },
            // A ruling of a cone: the plane through it and the axis.
            (Curve3::Line(l), Surface::Cone(c)) => Self::Plane {
                origin: c.axis_origin(),
                normal: c.axis().cross(l.dir).try_normalize()?,
            },
            (Curve3::Line(_), Surface::Sphere(_) | Surface::Torus(_)) => return None,
            // On a curved surface a circle or an ellipse is cut out by its own plane
            // (which may cut out more: callers check hits against the edge itself).
            (Curve3::Circle(ci), _) => Self::Plane {
                origin: ci.frame.origin,
                normal: ci.frame.z_axis(),
            },
            (Curve3::Ellipse(e), _) => Self::Plane {
                origin: e.frame.origin,
                normal: e.frame.z_axis(),
            },
        })
    }

    fn eval(&self, p: DVec3) -> f64 {
        match self {
            Self::Plane { origin, normal } => (p - *origin).dot(*normal),
            Self::Sphere { center, radius } => {
                (p.distance_squared(*center) - radius * radius) / (2.0 * radius)
            }
            Self::Elliptic { frame, a, b } => {
                let l = frame.to_local(p);
                ((l.x / a).powi(2) + (l.y / b).powi(2) - 1.0) * 0.5 * a.min(*b)
            }
        }
    }
}

/// Parameters where a curve meets an implicit curve.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Roots {
    At(Vec<f64>),
    /// The curve lies in the zero set over its whole length.
    Coincident,
}

/// Roots of `implicit` along `curve`. For periodic curves they lie anywhere in one period;
/// for lines, `range` bounds the numeric search (closed forms return every root).
pub(crate) fn solve(curve: &Curve3, implicit: &Implicit, range: (f64, f64)) -> Roots {
    match (curve, implicit) {
        (Curve3::Line(l), Implicit::Plane { origin, normal }) => {
            let a = (l.origin - *origin).dot(*normal);
            let b = l.dir.dot(*normal);
            if b.abs() <= ANGULAR {
                if a.abs() <= LINEAR {
                    Roots::Coincident
                } else {
                    Roots::At(Vec::new())
                }
            } else {
                Roots::At(vec![-a / b])
            }
        }
        (Curve3::Line(l), Implicit::Sphere { center, radius }) => {
            let tc = (*center - l.origin).dot(l.dir);
            let h = (l.origin + l.dir * tc).distance(*center);
            Roots::At(chord(tc, h, *radius, 1.0))
        }
        (Curve3::Line(l), Implicit::Elliptic { frame, a, b }) => {
            // Scale the ellipse to a unit circle; the line stays a line.
            let o = frame.to_local(l.origin);
            let d = frame.vector_to_local(l.dir);
            let o2 = DVec2::new(o.x / a, o.y / b);
            let d2 = DVec2::new(d.x / a, d.y / b);
            let len = d2.length();
            if len <= ANGULAR / a.max(*b) {
                return Roots::At(Vec::new());
            }
            let u = d2 / len;
            let sc = -o2.dot(u);
            let h = (o2 + u * sc).length();
            // `s` is arc length in the scaled plane; `t = s / len`.
            let scale = a.min(*b);
            Roots::At(
                chord(sc, h * scale, scale, 1.0 / scale)
                    .into_iter()
                    .map(|s| s / len)
                    .collect(),
            )
        }
        (Curve3::Circle(_) | Curve3::Ellipse(_), Implicit::Plane { origin, normal }) => {
            let (c, x, y) = conic_axes(curve);
            trig_linear((c - *origin).dot(*normal), x.dot(*normal), y.dot(*normal))
        }
        (Curve3::Circle(ci), Implicit::Sphere { center, radius }) => {
            // Circle against circle in one plane: |c + X cos t + Y sin t − m|² = r², which
            // is linear in (cos t, sin t) because |X| = |Y| and X ⟂ Y.
            let (c, x, y) = conic_axes(curve);
            let w = c - *center;
            let k = 1.0 / (2.0 * radius);
            trig_linear(
                (w.length_squared() + ci.radius * ci.radius - radius * radius) * k,
                2.0 * w.dot(x) * k,
                2.0 * w.dot(y) * k,
            )
        }
        _ => numeric(
            |t| implicit.eval(curve.point(t)),
            range,
            curve.period().is_some(),
        ),
    }
}

/// Centre and the two (scaled) axis vectors of a circle or ellipse:
/// `point(t) = c + x cos t + y sin t`.
fn conic_axes(curve: &Curve3) -> (DVec3, DVec3, DVec3) {
    match curve {
        Curve3::Circle(c) => (
            c.frame.origin,
            c.frame.x_axis() * c.radius,
            c.frame.y_axis() * c.radius,
        ),
        Curve3::Ellipse(e) => (
            e.frame.origin,
            e.frame.x_axis() * e.major,
            e.frame.y_axis() * e.minor,
        ),
        Curve3::Line(l) => (l.origin, l.dir, DVec3::ZERO),
    }
}

/// Where a line crosses a circle of radius `r`, given the parameter `tc` of the line point
/// closest to the centre and its distance `h` from it. `unit` converts a length along the
/// line into parameter. Touching within tolerance gives the single tangent point.
fn chord(tc: f64, h: f64, r: f64, unit: f64) -> Vec<f64> {
    if (h - r).abs() <= LINEAR {
        vec![tc]
    } else if h > r {
        Vec::new()
    } else {
        let w = (r * r - h * h).sqrt() * unit;
        vec![tc - w, tc + w]
    }
}

/// Roots of `a + b cos t + c sin t` (a distance-like function).
fn trig_linear(a: f64, b: f64, c: f64) -> Roots {
    let amp = b.hypot(c);
    if amp <= LINEAR {
        return if a.abs() <= LINEAR {
            Roots::Coincident
        } else {
            Roots::At(Vec::new())
        };
    }
    // a + amp cos(t − phi)
    let phi = c.atan2(b);
    if (a - amp).abs() <= LINEAR {
        Roots::At(vec![phi + std::f64::consts::PI])
    } else if (a + amp).abs() <= LINEAR {
        Roots::At(vec![phi])
    } else if a.abs() < amp {
        let d = (-a / amp).acos();
        Roots::At(vec![phi - d, phi + d])
    } else {
        Roots::At(Vec::new())
    }
}

/// Samples per search range of the numeric fallback.
const NUMERIC_SAMPLES: usize = 128;

/// Sign changes (bisected) and touching points (minima of `|f|` within tolerance) of a
/// distance-like function.
fn numeric(f: impl Fn(f64) -> f64, range: (f64, f64), periodic: bool) -> Roots {
    let (lo, hi) = if periodic { (0.0, TAU) } else { range };
    let n = NUMERIC_SAMPLES;
    let at = |i: usize| lo + (hi - lo) * i as f64 / n as f64;
    let values: Vec<f64> = (0..=n).map(|i| f(at(i))).collect();
    if values.iter().all(|v| v.abs() <= LINEAR) {
        return Roots::Coincident;
    }
    let mut roots = Vec::new();
    for i in 0..n {
        let (a, b) = (values[i], values[i + 1]);
        if a == 0.0 {
            roots.push(at(i));
        } else if a * b < 0.0 {
            let (mut x0, mut x1, mut f0) = (at(i), at(i + 1), a);
            for _ in 0..80 {
                let m = 0.5 * (x0 + x1);
                let fm = f(m);
                if (fm < 0.0) == (f0 < 0.0) {
                    x0 = m;
                    f0 = fm;
                } else {
                    x1 = m;
                }
            }
            roots.push(0.5 * (x0 + x1));
        }
    }
    // Touching without crossing: a local minimum of |f| between samples of one sign.
    let first = if periodic { 0 } else { 1 };
    for i in first..n {
        let (prev, here, next) = (values[(i + n - 1) % n], values[i], values[i + 1]);
        if here == 0.0 || prev * here < 0.0 || here * next < 0.0 {
            continue;
        }
        if here.abs() <= prev.abs() && here.abs() <= next.abs() {
            let step = (hi - lo) / n as f64;
            let (mut x0, mut x1) = (at(i) - step, at(i) + step);
            const GOLD: f64 = 0.618_033_988_749_894_8;
            for _ in 0..80 {
                let (m0, m1) = (x1 - GOLD * (x1 - x0), x0 + GOLD * (x1 - x0));
                if f(m0).abs() < f(m1).abs() {
                    x1 = m1;
                } else {
                    x0 = m0;
                }
            }
            let x = 0.5 * (x0 + x1);
            if f(x).abs() <= LINEAR {
                roots.push(x);
            }
        }
    }
    Roots::At(roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Circle3, Ellipse3, Line3};
    use peet_math::Plane;

    fn roots(c: &Curve3, e: &Curve3, host: &Surface) -> Vec<f64> {
        let imp = Implicit::of(e, host).unwrap();
        match solve(c, &imp, (-100.0, 100.0)) {
            Roots::At(mut v) => {
                for t in &v {
                    assert!(e.distance(c.point(*t)) < 1e-6 || matches!(host, Surface::Cylinder(_)));
                }
                v.sort_by(f64::total_cmp);
                v
            }
            Roots::Coincident => panic!("coincident"),
        }
    }

    #[test]
    fn line_and_circle_in_a_plane() {
        let host = Surface::Plane(Plane::TOP);
        let circle = Curve3::Circle(Circle3 {
            frame: Frame::WORLD,
            radius: 2.0,
        });
        let line = |y: f64| {
            Curve3::Line(Line3 {
                origin: DVec3::new(-5.0, y, 0.0),
                dir: DVec3::X,
            })
        };
        let r = roots(&line(0.0), &circle, &host);
        assert!((r[0] - 3.0).abs() < 1e-12 && (r[1] - 7.0).abs() < 1e-12);
        // Tangent: exactly one root.
        assert_eq!(roots(&line(2.0), &circle, &host), vec![5.0]);
        assert!(roots(&line(2.5), &circle, &host).is_empty());
        // The circle against the line.
        let r = roots(&circle, &line(0.0), &host);
        assert_eq!(r.len(), 2);
        assert_eq!(roots(&circle, &line(2.0), &host).len(), 1);
        // Two circles.
        let other = |x: f64| {
            Curve3::Circle(Circle3 {
                frame: Frame {
                    origin: DVec3::X * x,
                    ..Frame::WORLD
                },
                radius: 1.0,
            })
        };
        assert_eq!(roots(&circle, &other(2.0), &host).len(), 2);
        assert_eq!(roots(&circle, &other(3.0), &host).len(), 1);
        assert_eq!(roots(&circle, &other(1.0), &host).len(), 1);
        assert!(roots(&circle, &other(4.0), &host).is_empty());
        let imp = Implicit::of(&circle, &host).unwrap();
        assert_eq!(solve(&circle, &imp, (0.0, 0.0)), Roots::Coincident);
    }

    #[test]
    fn ellipse_in_a_plane() {
        let host = Surface::Plane(Plane::TOP);
        let ellipse = Curve3::Ellipse(Ellipse3 {
            frame: Frame::WORLD,
            major: 3.0,
            minor: 1.0,
        });
        let line = Curve3::Line(Line3 {
            origin: DVec3::new(0.0, -5.0, 0.0),
            dir: DVec3::Y,
        });
        let r = roots(&line, &ellipse, &host);
        assert!((r[0] - 4.0).abs() < 1e-12 && (r[1] - 6.0).abs() < 1e-12);
        let tangent = Curve3::Line(Line3 {
            origin: DVec3::new(3.0, -5.0, 0.0),
            dir: DVec3::Y,
        });
        assert_eq!(roots(&tangent, &ellipse, &host).len(), 1);
        // Numeric fallback: a circle crossing the ellipse four times.
        let circle = Curve3::Circle(Circle3 {
            frame: Frame::WORLD,
            radius: 2.0,
        });
        assert_eq!(roots(&circle, &ellipse, &host).len(), 4);
        assert_eq!(roots(&ellipse, &circle, &host).len(), 4);
        // Touching at the ends of the minor axis.
        let inner = Curve3::Circle(Circle3 {
            frame: Frame::WORLD,
            radius: 1.0,
        });
        assert_eq!(roots(&inner, &ellipse, &host).len(), 2);
    }
}
