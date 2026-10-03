//! Changing a solid's faces without changing how they connect: offsetting faces along
//! their normals, hollowing a body into a shell, and tapering walls with a draft angle.
//!
//! **Method.** All three give some faces a new surface (a plane moved or tilted, a
//! cylinder made thinner) and keep the topology: the same faces meet along the same edges
//! at the same vertices. [`resurface`] works the geometry out again from the new surfaces:
//!
//! - a vertex is the point where its faces' new surfaces meet, found by Newton's method
//!   from where it was;
//! - an edge is the intersection curve of its two faces' new surfaces
//!   ([`crate::boolean`]'s surface–surface intersection), the branch through its new
//!   vertices, running the way it did; a seam follows its surface.
//!
//! The result is validated. A change too big for the topology to survive (a thickness
//! that swallows a small face, a draft that pulls a wall off its neighbours) shows up as
//! an edge that turns round or a loop that turns inside out, and is refused with a
//! message; nothing is patched up.
//!
//! **Shell.** The hollow of a shell is the body offset inwards by the wall thickness,
//! with the faces to open left where they are; it is then cut from the body. The cut
//! starts exactly on the open faces, which is a case the boolean code handles as such.
//!
//! **Draft.** Each drafted face is a plane turned about the line where it crosses the
//! neutral plane, so the part keeps its size there and tapers along the pull direction
//! (the neutral plane's normal). A cylinder along the pull direction becomes the cone
//! through its circle in the neutral plane, so a rounded boss or a hole takes draft
//! together with the flat walls that run into it.

use std::f64::consts::TAU;

use peet_math::tolerance::{self, LINEAR};
use peet_math::{DMat3, DQuat, DVec2, DVec3, Plane};

use crate::boolean::ssi::{self, Ssi};
use crate::boolean::{BooleanOp, boolean_traced};
use crate::geom::{Circle3, Cone, Curve3, Cylinder, Ellipse3, Line3, Sphere, Surface, Torus};
use crate::topo::{EdgeId, FaceId};
use crate::{KernelError, Solid};

/// Where a face of a shelled solid came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ShellFace {
    /// (Part of) this face of the input: the outside, and the rims around the openings.
    Outer(FaceId),
    /// The inside wall behind this face of the input.
    Inner(FaceId),
}

/// A shelled solid with the origin of its faces.
#[derive(Clone, Debug, PartialEq)]
pub struct Shelled {
    pub solid: Solid,
    /// For each face of `solid` (by face index), what it is made of, sorted.
    pub faces: Vec<Vec<ShellFace>>,
}

/// Moves every face of `solid` along its outward normal by `distance(face)` (inwards for
/// negative values) and returns the solid with the same topology.
pub fn offset(solid: &Solid, distance: impl Fn(FaceId) -> f64) -> Result<Solid, KernelError> {
    let mut surfaces = Vec::with_capacity(solid.faces.len());
    for face in solid.face_ids() {
        let d = distance(face);
        if !d.is_finite() {
            return Err(KernelError::InvalidInput(
                "the offset is not a finite number".to_owned(),
            ));
        }
        let f = solid.face(face);
        surfaces.push(offset_surface(&f.surface, if f.reversed { -d } else { d })?);
    }
    resurface(solid, &surfaces).map_err(|e| match e {
        KernelError::InvalidResult(m) => KernelError::InvalidResult(format!(
            "The faces can't be moved that far: the body would lose a face or fold over \
             itself. Use a smaller distance. ({m})"
        )),
        other => other,
    })
}

/// `surface` moved by `d` along its natural normal.
fn offset_surface(surface: &Surface, d: f64) -> Result<Surface, KernelError> {
    let gone = |what: &str| {
        Err(KernelError::InvalidResult(format!(
            "The distance is as large as the radius of a {what} face, which would vanish. \
             Use a smaller distance."
        )))
    };
    Ok(match surface {
        Surface::Plane(p) => Surface::Plane(Plane {
            frame: peet_math::Frame {
                origin: p.origin() + p.normal() * d,
                rotation: p.frame.rotation,
            },
        }),
        Surface::Cylinder(c) => {
            if c.radius + d <= 10.0 * LINEAR {
                return gone("cylindrical");
            }
            Surface::Cylinder(Cylinder {
                radius: c.radius + d,
                ..*c
            })
        }
        Surface::Cone(c) => {
            // The rulings move square to themselves: further from the axis by d / cos α
            // at the same height. Slide the frame along the axis if that would make the
            // radius at its origin negative.
            let radius = c.radius + d / c.half_angle.cos();
            if radius >= 0.0 {
                Surface::Cone(Cone { radius, ..*c })
            } else {
                let shift = -radius / c.slope();
                Surface::Cone(Cone {
                    frame: peet_math::Frame {
                        origin: c.frame.origin + c.axis() * shift,
                        rotation: c.frame.rotation,
                    },
                    radius: 0.0,
                    half_angle: c.half_angle,
                })
            }
        }
        Surface::Sphere(s) => {
            if s.radius + d <= 10.0 * LINEAR {
                return gone("spherical");
            }
            Surface::Sphere(Sphere {
                radius: s.radius + d,
                ..*s
            })
        }
        Surface::Torus(t) => {
            if t.minor + d <= 10.0 * LINEAR {
                return gone("rounded");
            }
            Surface::Torus(Torus {
                minor: t.minor + d,
                ..*t
            })
        }
        Surface::Nurbs(_) if d == 0.0 => surface.clone(),
        Surface::Nurbs(_) => {
            return Err(KernelError::Unsupported(
                "freeform faces can't be offset yet: a shell or an offset needs every moved \
                 face to be flat, cylindrical, conical, spherical or toroidal"
                    .to_owned(),
            ));
        }
    })
}

/// Hollows `solid`: walls `thickness` thick are left behind every face except those in
/// `open`, which are removed to open the hollow (none: a closed hollow body).
pub fn shell(solid: &Solid, open: &[FaceId], thickness: f64) -> Result<Shelled, KernelError> {
    if !thickness.is_finite() || thickness <= 10.0 * LINEAR {
        return Err(KernelError::InvalidInput(
            "the wall thickness must be greater than zero".to_owned(),
        ));
    }
    if let Some(bad) = open.iter().find(|f| f.index() >= solid.faces.len()) {
        return Err(KernelError::InvalidInput(format!(
            "face {} to open doesn't exist",
            bad.0
        )));
    }
    let hollow = offset(solid, |f| if open.contains(&f) { 0.0 } else { -thickness }).map_err(
        |e| match e {
            KernelError::InvalidResult(m) => KernelError::InvalidResult(m.replace(
                "The faces can't be moved that far",
                "The walls are too thick for this body",
            )),
            other => other,
        },
    )?;
    let traced = boolean_traced(solid, &hollow, BooleanOp::Subtract)?;
    let faces = traced
        .sources
        .iter()
        .map(|sources| {
            let mut labels: Vec<ShellFace> = sources
                .iter()
                .map(|s| {
                    if s.solid == 0 {
                        ShellFace::Outer(s.face)
                    } else {
                        ShellFace::Inner(s.face)
                    }
                })
                .collect();
            labels.sort_unstable();
            labels.dedup();
            labels
        })
        .collect();
    Ok(Shelled {
        solid: traced.solid,
        faces,
    })
}

/// Tilts the `faces` by `angle` (radians) about the lines where they cross `neutral`:
/// flat faces stay flat, and round faces whose axis is along `neutral`'s normal become
/// cones. With a positive angle the body gets narrower along `neutral`'s normal (the
/// direction a mould would be pulled off in).
pub fn draft(
    solid: &Solid,
    faces: &[FaceId],
    neutral: &Plane,
    angle: f64,
) -> Result<Solid, KernelError> {
    if !angle.is_finite() || angle.abs() >= 89f64.to_radians() {
        return Err(KernelError::InvalidInput(
            "the draft angle must be between −89° and 89°".to_owned(),
        ));
    }
    if faces.is_empty() {
        return Err(KernelError::InvalidInput(
            "pick the faces to draft".to_owned(),
        ));
    }
    let pull = neutral.normal();
    let mut surfaces: Vec<Surface> = solid.faces.iter().map(|f| f.surface.clone()).collect();
    for &face in faces {
        let Some(f) = solid.faces.get(face.index()) else {
            return Err(KernelError::InvalidInput(
                "a face to draft doesn't exist".to_owned(),
            ));
        };
        if let Surface::Cylinder(c) = &f.surface {
            // A round wall along the pull direction becomes a cone through the circle
            // where it crosses the neutral plane.
            let along = c.axis().dot(pull);
            if along.abs() < 1.0 - 1e-9 {
                return Err(KernelError::Unsupported(
                    "a round face can be drafted only if its axis is along the direction of \
                     pull (square to the neutral plane)"
                        .to_owned(),
                ));
            }
            if angle.abs() <= 1e-12 {
                continue;
            }
            let height = (neutral.origin() - c.axis_origin()).dot(pull) / along;
            // The outward normal turns towards the pull: a boss narrows, a hole widens.
            let side = if f.reversed { 1.0 } else { -1.0 };
            surfaces[face.index()] = Surface::Cone(Cone {
                frame: peet_math::Frame {
                    origin: c.axis_origin() + c.axis() * height,
                    rotation: c.frame.rotation,
                },
                radius: c.radius,
                half_angle: side * angle * along.signum(),
            });
            continue;
        }
        let Surface::Plane(plane) = f.surface else {
            return Err(KernelError::Unsupported(
                "flat faces and round faces along the pull direction can be drafted; this \
                 face is neither"
                    .to_owned(),
            ));
        };
        let outward = if f.reversed {
            -plane.normal()
        } else {
            plane.normal()
        };
        // The hinge: where the face's plane crosses the neutral plane.
        let hinge = pull.cross(outward);
        if hinge.length() <= 1e-9 {
            return Err(KernelError::InvalidInput(
                "a face parallel to the neutral plane can't be drafted: it has no line to \
                 turn about"
                    .to_owned(),
            ));
        }
        let (d1, d2) = (pull.dot(neutral.origin()), outward.dot(plane.origin()));
        let point = (outward.cross(hinge) * d1 + hinge.cross(pull) * d2) / hinge.length_squared();
        // Turn the normal towards the pull direction.
        let turned = DQuat::from_axis_angle(outward.cross(pull).normalize(), angle) * outward;
        let normal = if f.reversed { -turned } else { turned };
        let new = Plane::from_origin_normal_x(point, normal, hinge)
            .ok_or_else(|| KernelError::InvalidInput("a degenerate face".to_owned()))?;
        surfaces[face.index()] = Surface::Plane(new);
    }
    resurface(solid, &surfaces).map_err(|e| match e {
        KernelError::InvalidResult(m) => KernelError::InvalidResult(format!(
            "The faces can't be drafted that far: a wall would come off its neighbours or \
             a face would vanish. Use a smaller angle. ({m})"
        )),
        other => other,
    })
}

/// `solid` with `surfaces[i]` as the surface of face `i`, and its vertices and edges
/// worked out again from them.
fn resurface(solid: &Solid, surfaces: &[Surface]) -> Result<Solid, KernelError> {
    let mut out = solid.clone();
    for (f, s) in out.faces.iter_mut().zip(surfaces) {
        f.surface = s.clone();
    }
    let scale = solid.bounds().size().length().max(1.0);

    // Vertices: where their faces' new surfaces meet.
    let mut faces_at: Vec<Vec<FaceId>> = vec![Vec::new(); solid.vertices.len()];
    for e in &solid.edges {
        for v in [e.start, e.end] {
            for &c in &e.coedges {
                let f = solid.coedge_face(c);
                if !faces_at[v.index()].contains(&f) {
                    faces_at[v.index()].push(f);
                }
            }
        }
    }
    for v in solid.vertex_ids() {
        let old = solid.vertex(v).point;
        let faces = &faces_at[v.index()];
        // A pole stays a pole.
        let pole = faces.iter().find_map(|f| {
            let pole = solid.face(*f).surface.pole_at(old)?;
            let new = &surfaces[f.index()];
            let same = new
                .poles()
                .into_iter()
                .flatten()
                .find(|p| p.top == pole.top)?;
            Some(new.point(DVec2::new(0.0, same.v)))
        });
        let list: Vec<&Surface> = faces.iter().map(|f| &surfaces[f.index()]).collect();
        let point = match pole {
            Some(p) => p,
            None => meeting_point(&list, old)?,
        };
        let off = list
            .iter()
            .map(|s| s.signed_distance(point).abs())
            .fold(0.0, f64::max);
        if off > 0.01 * LINEAR * scale.max(1.0) {
            return Err(KernelError::InvalidResult(format!(
                "the faces at a corner near ({:.3}, {:.3}, {:.3}) no longer meet in one point",
                old.x, old.y, old.z
            )));
        }
        out.vertices[v.index()].point = point;
    }

    // Edges: where their two faces' new surfaces meet.
    for id in solid.edge_ids() {
        let e = solid.edge(id);
        let [ca, cb] = e.coedges[..] else {
            return Err(KernelError::InvalidInput(
                "the body is not closed".to_owned(),
            ));
        };
        let (fa, fb) = (solid.coedge_face(ca), solid.coedge_face(cb));
        let (start, end) = (
            out.vertices[e.start.index()].point,
            out.vertices[e.end.index()].point,
        );
        let middle = e.point_at_fraction(0.5);
        let curve = if fa == fb {
            seam(solid, id, &surfaces[fa.index()], start, end)?
        } else {
            let found = match ssi::intersect(&surfaces[fa.index()], &surfaces[fb.index()], middle) {
                Ssi::Curves(curves) => curves,
                Ssi::Unsupported => {
                    return Err(KernelError::Unsupported(
                        "two of the changed faces would meet in a curve the kernel can't \
                         represent"
                            .to_owned(),
                    ));
                }
                Ssi::None | Ssi::Coincident => Vec::new(),
            };
            // The branch through the edge's new vertices (nearest where it used to be).
            found
                .into_iter()
                .map(|c| {
                    let miss = c.distance(start) + c.distance(end);
                    (miss, c.distance(middle), c)
                })
                .filter(|(miss, _, _)| *miss <= 1e-6 * scale)
                .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)))
                .map(|(_, _, c)| c)
                .ok_or_else(|| {
                    KernelError::InvalidResult(format!(
                        "two faces no longer meet along their edge near ({:.3}, {:.3}, {:.3})",
                        middle.x, middle.y, middle.z
                    ))
                })?
        };
        let (curve, t0, t1) = fit(e, curve, start, end).ok_or_else(|| {
            KernelError::InvalidResult(format!(
                "the edge near ({:.3}, {:.3}, {:.3}) would shrink to nothing or turn round",
                middle.x, middle.y, middle.z
            ))
        })?;
        let edge = &mut out.edges[id.index()];
        edge.curve = curve;
        edge.t0 = t0;
        edge.t1 = t1;
    }

    crate::validate::validate(&out).map_err(|problems| {
        let list: Vec<&str> = problems.iter().map(|p| p.message.as_str()).collect();
        KernelError::InvalidResult(list.join("; "))
    })?;
    Ok(out)
}

/// The point nearest `near` where the surfaces meet (Gauss–Newton on their signed
/// distances; with fewer than three surfaces, or dependent ones, the point stays as
/// close to `near` as the surfaces allow).
fn meeting_point(surfaces: &[&Surface], near: DVec3) -> Result<DVec3, KernelError> {
    let mut p = near;
    for _ in 0..64 {
        let mut jtj = DMat3::ZERO;
        let mut jtf = DVec3::ZERO;
        let mut worst = 0.0_f64;
        for s in surfaces {
            let f = s.signed_distance(p);
            let n = s.normal_at(p);
            worst = worst.max(f.abs());
            jtj += DMat3::from_cols(n * n.x, n * n.y, n * n.z);
            jtf += n * f;
        }
        if worst <= 1e-13 * near.length().max(1.0) {
            break;
        }
        // A little damping keeps the step defined where the normals don't span space
        // (a vertex on a single face, or on two): the point then moves only as needed.
        let trace = jtj.x_axis.x + jtj.y_axis.y + jtj.z_axis.z;
        let damped = jtj + DMat3::from_diagonal(DVec3::splat(1e-12 * trace.max(1e-300)));
        if damped.determinant().abs() <= f64::MIN_POSITIVE {
            break;
        }
        let step = damped.inverse() * jtf;
        if !step.is_finite() {
            return Err(KernelError::InvalidResult(
                "a corner could not be found again".to_owned(),
            ));
        }
        p -= step;
    }
    Ok(p)
}

/// The curve of a seam edge on its face's new surface, through the new vertices.
fn seam(
    solid: &Solid,
    id: EdgeId,
    new: &Surface,
    start: DVec3,
    end: DVec3,
) -> Result<Curve3, KernelError> {
    let e = solid.edge(id);
    let unsupported = || {
        Err(KernelError::Unsupported(
            "a seam of this kind can't follow its face yet".to_owned(),
        ))
    };
    match (&e.curve, new) {
        // A ruling of a cylinder or a cone: the line through its ends.
        (Curve3::Line(_), _) => Curve3::line_through(start, end)
            .ok_or_else(|| KernelError::InvalidResult("a seam would shrink to nothing".to_owned())),
        // A meridian of a sphere or a torus keeps its plane and centre.
        (Curve3::Circle(c), Surface::Sphere(s)) => Ok(Curve3::Circle(Circle3 {
            frame: c.frame,
            radius: s.radius,
        })),
        (Curve3::Circle(c), Surface::Torus(t)) => {
            let axis = t.frame.z_axis();
            if c.frame.z_axis().dot(axis).abs() <= tolerance::ANGULAR {
                Ok(Curve3::Circle(Circle3 {
                    frame: c.frame,
                    radius: t.minor,
                }))
            } else if tolerance::directions_parallel(c.frame.z_axis(), axis) {
                // A circle around the axis, at the tube angle it had.
                let old = &solid.face(solid.coedge_face(e.coedges[0])).surface;
                let v = old.param(e.point_at_fraction(0.5)).y;
                let m = new.meridian(v).expect("a torus has a meridian");
                Ok(Curve3::Circle(Circle3 {
                    frame: peet_math::Frame {
                        origin: t.frame.origin + axis * m.z,
                        rotation: c.frame.rotation,
                    },
                    radius: m.rho,
                }))
            } else {
                unsupported()
            }
        }
        _ => unsupported(),
    }
}

/// `curve` turned to run the way the edge `e` did, with the parameters of the new ends.
/// `None` if the edge has shrunk to nothing or turned round.
fn fit(
    e: &crate::topo::Edge,
    curve: Curve3,
    start: DVec3,
    end: DVec3,
) -> Option<(Curve3, f64, f64)> {
    // Its direction where the old edge's middle now is, against the old direction there.
    let old_dir = e.curve.tangent(0.5 * (e.t0 + e.t1));
    let at = curve.param(e.point_at_fraction(0.5));
    let curve = if curve.tangent(at).dot(old_dir) < 0.0 {
        reversed(&curve)
    } else {
        curve
    };
    let t0 = curve.param(start);
    let mut t1 = curve.param(end);
    let span = e.t1 - e.t0;
    match curve.period() {
        None => (t1 - t0 > LINEAR).then_some((curve, t0, t1)),
        Some(period) => {
            if e.is_closed() {
                return Some((curve, t0, t0 + period));
            }
            t1 = t0 + (t1 - t0).rem_euclid(period);
            // An arc can't jump from a sliver to nearly a full turn, or back.
            if (t1 - t0 - span).abs() > 0.5 * TAU {
                return None;
            }
            (t1 - t0 > 1e-9).then_some((curve, t0, t1))
        }
    }
}

/// The same curve run the other way: `point(t)` becomes `point(−t)`.
fn reversed(curve: &Curve3) -> Curve3 {
    let flip = |frame: &peet_math::Frame| peet_math::Frame {
        origin: frame.origin,
        rotation: (frame.rotation * DQuat::from_rotation_x(std::f64::consts::PI)).normalize(),
    };
    match curve {
        Curve3::Line(l) => Curve3::Line(Line3 {
            origin: l.origin,
            dir: -l.dir,
        }),
        Curve3::Circle(c) => Curve3::Circle(Circle3 {
            frame: flip(&c.frame),
            radius: c.radius,
        }),
        Curve3::Ellipse(e) => Curve3::Ellipse(Ellipse3 {
            frame: flip(&e.frame),
            ..*e
        }),
        Curve3::Nurbs(c) => Curve3::Nurbs(std::sync::Arc::new(c.reversed())),
    }
}

#[cfg(test)]
mod tests;
