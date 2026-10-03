//! Changing a solid's faces without changing how they connect: offsetting faces along
//! their normals, hollowing a body into a shell, and tapering walls with a draft angle.
//!
//! **Method.** All three give some faces a new surface (a plane moved or tilted, a
//! cylinder made thinner, a freeform surface offset) and keep the topology: the same
//! faces meet along the same edges at the same vertices. [`resurface`] works the geometry
//! out again from the new surfaces:
//!
//! - a vertex is the point where its faces' new surfaces meet, found by Newton's method
//!   from where it was;
//! - an edge is the intersection curve of its two faces' new surfaces. Where that has a
//!   closed form ([`crate::boolean`]'s surface–surface intersection) it is the branch
//!   through the edge's new vertices, running the way the edge did, so a cylinder stays
//!   bounded by lines and circles. Where it has none (a freeform surface, cylinders that
//!   cross) the edge is worked out point by point from the old one and fitted with a
//!   curve ([`freeform`]). A seam follows its surface.
//!
//! The result is validated. A change too big for the topology to survive (a thickness
//! that swallows a small face, a draft that pulls a wall off its neighbours) shows up as
//! an edge that turns round or a loop that turns inside out, and is refused with a
//! message; nothing is patched up.
//!
//! **Freeform faces.** The offset of a NURBS surface is not a NURBS surface, so it is
//! fitted to a fraction of the modelling tolerance ([`NurbsSurface::offset`]), carried
//! on a little past the surface's edges so that it still reaches neighbours that moved
//! away. An offset as large as the surface's tightest radius of curvature on the side it
//! moves to would fold over itself and is refused. A freeform face that would have to
//! carry on further than that to meet its neighbour is refused too.
//!
//! **Shell.** The hollow of a shell is the body offset inwards by the wall thickness,
//! with the faces to open left where they are; it is then cut from the body. The cut
//! starts exactly on the open faces, which is a case the boolean code handles as such.
//! For a body with freeform faces the cut is not computed but written down ([`lined`]):
//! the hollow touches the body only on the open faces, so the result is the body, the
//! hollow's faces turned inside out, and a rim on each open face. Tracing every pair of
//! freeform faces to find that they don't meet would take seconds.
//!
//! **Draft.** Each drafted face is replaced by the ruled surface through its neutral
//! curve (where it crosses the neutral plane), every ruling tilted from the pull
//! direction (the neutral plane's normal) by the draft angle, so the part keeps its size
//! in the neutral plane and tapers along the pull. For a flat face that is the plane
//! turned about a line; for a cylinder or a cone along the pull it is the cone through
//! its circle in the neutral plane, so a rounded boss or a hole takes draft together
//! with the flat walls that run into it; for a freeform face (the wall of an extruded
//! spline, the side of a loft) it is a fitted ruled surface ([`freeform`]).

mod freeform;

use std::f64::consts::TAU;
use std::sync::Arc;

use peet_math::tolerance::{self, LINEAR};
use peet_math::{DQuat, DVec2, DVec3, Plane};

use self::freeform::{EdgeJob, Probe};
use crate::boolean::classify::Body;
use crate::boolean::domain::FaceGeom;
use crate::boolean::ssi::{self, Ssi};
use crate::boolean::{BooleanOp, boolean_traced};
use crate::geom::{Circle3, Cone, Curve3, Cylinder, Ellipse3, Line3, Sphere, Surface, Torus};
#[cfg(doc)]
use crate::nurbs::NurbsSurface;
use crate::topo::{EdgeId, FaceId};
use crate::{KernelError, Solid};

/// How far past its edges an offset freeform surface is carried on, as a multiple of
/// the offset: enough for it to meet a neighbour at any but the sharpest angles.
const REACH: f64 = 4.0;

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
    offset_faces(solid, distance, false, true)
}

/// [`offset`]; `wall` words the refusals for a shell's wall thickness, and `checked` is
/// as for [`resurface`].
fn offset_faces(
    solid: &Solid,
    distance: impl Fn(FaceId) -> f64,
    wall: bool,
    checked: bool,
) -> Result<Solid, KernelError> {
    let mut surfaces = Vec::with_capacity(solid.faces.len());
    for face in solid.face_ids() {
        let d = distance(face);
        if !d.is_finite() {
            return Err(KernelError::InvalidInput(
                "the offset is not a finite number".to_owned(),
            ));
        }
        let f = solid.face(face);
        surfaces.push(offset_surface(
            &f.surface,
            if f.reversed { -d } else { d },
            wall,
        )?);
    }
    resurface(solid, &surfaces, checked).map_err(|e| match e {
        KernelError::InvalidResult(m) => KernelError::InvalidResult(too_far(&m)),
        other => other,
    })
}

/// What to say when an offset has not kept the body's topology.
fn too_far(problem: &str) -> String {
    format!(
        "The faces can't be moved that far: the body would lose a face or fold over itself. \
         Use a smaller distance. ({problem})"
    )
}

/// `surface` moved by `d` along its natural normal. A freeform surface that doesn't move
/// is kept as it is (the very same surface, which is how a shell's open faces are known
/// to lie on the body's).
fn offset_surface(surface: &Surface, d: f64, wall: bool) -> Result<Surface, KernelError> {
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
        Surface::Nurbs(s) => Surface::Nurbs(Arc::new(
            s.offset(d, REACH * d.abs(), freeform::SURFACE_FIT)
                .map_err(|e| freeform::offset_error(e, wall))?,
        )),
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
    let too_thick = |e: KernelError| match e {
        KernelError::InvalidResult(m) => KernelError::InvalidResult(m.replace(
            "The faces can't be moved that far",
            "The walls are too thick for this body",
        )),
        other => other,
    };
    // A body with freeform faces is lined with its hollow directly, and validated once,
    // as a whole.
    let direct = lines_directly(solid, open);
    let hollow = offset_faces(
        solid,
        |f| if open.contains(&f) { 0.0 } else { -thickness },
        true,
        !direct,
    )
    .map_err(too_thick)?;
    if direct {
        return lined(solid, &hollow, open).map_err(too_thick);
    }
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

/// Whether the shell of `solid` is put together by [`lined`] instead of by cutting the
/// hollow from the body: if the body has a freeform face (without one the cut is quick
/// and exact), and no two open faces touch (where they do, their rims run into each
/// other and the general cut is needed).
fn lines_directly(solid: &Solid, open: &[FaceId]) -> bool {
    let freeform = solid
        .faces
        .iter()
        .any(|f| matches!(f.surface, Surface::Nurbs(_)));
    if !freeform {
        return false;
    }
    // No vertex may be on two open faces.
    let mut opened = vec![0u8; solid.vertices.len()];
    for &f in open {
        let mut seen: Vec<crate::topo::VertexId> = Vec::new();
        for &l in &solid.face(f).loops {
            for c in solid.loop_coedges(l) {
                let v = solid.coedge_start(c);
                if !seen.contains(&v) {
                    seen.push(v);
                    opened[v.index()] += 1;
                }
            }
        }
    }
    opened.iter().all(|n| *n <= 1)
}

/// The shell of a body, put together directly: the body, lined with its `hollow`
/// turned inside out, and each open face left as the rim between its own outline and
/// the hollow's. This is what cutting the hollow from the body gives when the hollow
/// touches the body only on the open faces, which an inward offset that has kept its
/// topology does; it spares tracing where dozens of pairs of freeform faces don't
/// meet. The hollow comes unvalidated: the result is validated as a whole.
fn lined(solid: &Solid, hollow: &Solid, open: &[FaceId]) -> Result<Shelled, KernelError> {
    let mut out = solid.clone();
    let mut labels: Vec<Vec<ShellFace>> = solid
        .face_ids()
        .map(|f| vec![ShellFace::Outer(f)])
        .collect();
    let vertices: Vec<crate::topo::VertexId> = hollow
        .vertices
        .iter()
        .map(|v| out.add_vertex(v.point))
        .collect();
    let edges: Vec<EdgeId> = hollow
        .edges
        .iter()
        .map(|e| {
            out.add_edge(
                e.curve.clone(),
                vertices[e.start.index()],
                vertices[e.end.index()],
                e.t0,
                e.t1,
            )
        })
        .collect();
    // A loop of the hollow, run the other way round.
    let turned = |l: crate::topo::LoopId| -> Vec<(EdgeId, bool)> {
        hollow
            .loop_coedges(l)
            .into_iter()
            .rev()
            .map(|c| {
                let co = hollow.coedge(c);
                (edges[co.edge.index()], !co.reversed)
            })
            .collect()
    };
    for (si, shell) in hollow.shells.iter().enumerate() {
        // Through an open face the lining joins the shell it lines; otherwise it is a
        // shell of its own (a closed hollow).
        let joined = shell.faces.iter().any(|f| open.contains(f));
        let target = if joined {
            crate::topo::ShellId(si as u32)
        } else {
            out.add_shell()
        };
        for &f in &shell.faces {
            let face = hollow.face(f);
            if !open.contains(&f) {
                let lining = out.add_face(target, face.surface.clone(), !face.reversed);
                labels.push(vec![ShellFace::Inner(f)]);
                for &l in &face.loops {
                    out.add_loop(lining, &turned(l));
                }
                continue;
            }
            // The rim: the face's outline, with the hollow's outline as a hole in it.
            out.add_loop(f, &turned(face.loops[0]));
            // Around each hole of the face the hollow's (larger) hole leaves a ring.
            let holes: Vec<crate::topo::LoopId> = solid.face(f).loops[1..].to_vec();
            for (k, hole) in holes.into_iter().enumerate() {
                let ring = out.add_face(target, face.surface.clone(), face.reversed);
                labels.push(vec![ShellFace::Outer(f)]);
                out.add_loop(ring, &turned(face.loops[k + 1]));
                out.faces[f.index()].loops.retain(|l| *l != hole);
                out.loops[hole.index()].face = ring;
                out.faces[ring.index()].loops.push(hole);
            }
        }
    }
    crate::validate::validate(&out).map_err(|problems| {
        let list: Vec<&str> = problems.iter().map(|p| p.message.as_str()).collect();
        KernelError::InvalidResult(too_far(&list.join("; ")))
    })?;
    // The cut would have found the hollow breaking out through a thin part of the body;
    // here that has to be looked for. The hollow's corners and the middles of its edges
    // (those not on an open face, which are on the body's boundary) must be inside.
    let body = Body {
        faces: solid.face_ids().map(|f| FaceGeom::new(solid, f)).collect(),
        bounds: solid.bounds(),
    };
    let on_open = |e: &crate::topo::Edge| {
        e.coedges
            .iter()
            .any(|c| open.contains(&hollow.coedge_face(*c)))
    };
    let mut inside: Vec<DVec3> = Vec::new();
    let mut at_opening = vec![false; hollow.vertices.len()];
    for e in hollow.edges.iter().filter(|e| on_open(e)) {
        at_opening[e.start.index()] = true;
        at_opening[e.end.index()] = true;
    }
    for (v, vertex) in hollow.vertices.iter().enumerate() {
        if !at_opening[v] {
            inside.push(vertex.point);
        }
    }
    for e in hollow.edges.iter().filter(|e| !on_open(e)) {
        inside.push(e.point_at_fraction(0.5));
    }
    if let Some(p) = inside.iter().find(|p| !body.contains(**p)) {
        return Err(KernelError::InvalidResult(format!(
            "The walls are too thick for this body: the hollow would break out through \
             it near ({:.3}, {:.3}, {:.3}). Use a thinner wall.",
            p.x, p.y, p.z
        )));
    }
    Ok(Shelled {
        solid: out,
        faces: labels,
    })
}

/// Tilts the `faces` by `angle` (radians) about the curves where they cross `neutral`:
/// flat faces stay flat, cylinders and cones whose axis is along `neutral`'s normal
/// become cones, and freeform faces become ruled surfaces through their neutral curves.
/// With a positive angle the body gets narrower along `neutral`'s normal (the direction
/// a mould would be pulled off in).
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
        // A round wall about the pull direction: its frame, and its radius at a height.
        let round = match &f.surface {
            Surface::Cylinder(c) => Some((c.frame, None, c.radius)),
            Surface::Cone(c) => Some((c.frame, Some(*c), c.radius)),
            _ => None,
        };
        if let Some((frame, cone, radius)) = round {
            // It becomes the cone through the circle where it crosses the neutral plane.
            let along = frame.z_axis().dot(pull);
            if along.abs() < 1.0 - 1e-9 {
                return Err(KernelError::Unsupported(
                    "a round face can be drafted only if its axis is along the direction of \
                     pull (square to the neutral plane)"
                        .to_owned(),
                ));
            }
            if angle.abs() <= 1e-12 && cone.is_none() {
                continue;
            }
            let height = (neutral.origin() - frame.origin).dot(pull) / along;
            let radius = cone.map_or(radius, |c| c.radius_at(height));
            if radius <= 10.0 * LINEAR {
                return Err(KernelError::InvalidInput(
                    "a conical face to draft comes to its point at or before the neutral \
                     plane, so it has no circle there to turn about. Move the neutral plane"
                        .to_owned(),
                ));
            }
            let frame = peet_math::Frame {
                origin: frame.origin + frame.z_axis() * height,
                rotation: frame.rotation,
            };
            // The outward normal turns towards the pull: a boss narrows, a hole widens.
            let side = if f.reversed { 1.0 } else { -1.0 };
            surfaces[face.index()] = if angle.abs() <= 1e-12 {
                Surface::Cylinder(Cylinder { frame, radius })
            } else {
                Surface::Cone(Cone {
                    frame,
                    radius,
                    half_angle: side * angle * along.signum(),
                })
            };
            continue;
        }
        if matches!(f.surface, Surface::Nurbs(_)) {
            surfaces[face.index()] = freeform::draft_surface(solid, face, neutral, angle)?;
            continue;
        }
        let Surface::Plane(plane) = f.surface else {
            return Err(KernelError::Unsupported(
                "flat faces, round faces along the pull direction and freeform faces can be \
                 drafted; a spherical or a toroidal face can't"
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
    resurface(solid, &surfaces, true).map_err(|e| match e {
        KernelError::InvalidResult(m) => KernelError::InvalidResult(format!(
            "The faces can't be drafted that far: a wall would come off its neighbours or \
             a face would vanish. Use a smaller angle. ({m})"
        )),
        other => other,
    })
}

/// `solid` with `surfaces[i]` as the surface of face `i`, and its vertices and edges
/// worked out again from them. The result is validated unless `checked` is off, which
/// is for a caller that validates what it makes of the result.
fn resurface(solid: &Solid, surfaces: &[Surface], checked: bool) -> Result<Solid, KernelError> {
    let mut out = solid.clone();
    for (f, s) in out.faces.iter_mut().zip(surfaces) {
        f.surface = s.clone();
    }
    let scale = solid.bounds().size().length().max(1.0);
    // Faces whose surface is the one they had.
    let kept: Vec<bool> = solid
        .faces
        .iter()
        .zip(surfaces)
        .map(|(f, new)| match (&f.surface, new) {
            (Surface::Nurbs(a), Surface::Nurbs(b)) => Arc::ptr_eq(a, b),
            (a, b) => a == b,
        })
        .collect();
    let freeform = |f: FaceId| matches!(surfaces[f.index()], Surface::Nurbs(_));

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
        // Nothing at this corner has changed.
        if faces.iter().all(|f| kept[f.index()]) {
            continue;
        }
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
        let mut probes: Vec<Probe<'_>> = faces
            .iter()
            .map(|f| Probe::new(&surfaces[f.index()]))
            .collect();
        let point = match pole {
            Some(p) => p,
            None => freeform::meet(&mut probes, None, old)?,
        };
        // Fitted surfaces meet to the tolerance they were fitted to, not exactly.
        let allowed = if faces.iter().any(|f| freeform(*f)) {
            freeform::ON_FACE
        } else {
            0.01 * LINEAR * scale.max(1.0)
        };
        let (mut off, mut gap) = (0.0_f64, 0.0_f64);
        for probe in &mut probes {
            let r = probe.read(point);
            off = off.max(r.distance.abs());
            gap = gap.max(r.gap);
        }
        if off > allowed {
            return Err(KernelError::InvalidResult(format!(
                "the faces at a corner near ({:.3}, {:.3}, {:.3}) no longer meet in one point",
                old.x, old.y, old.z
            )));
        }
        if gap > allowed {
            return Err(freeform::past_the_edge(point));
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
        let old_ends = [solid.vertex(e.start).point, solid.vertex(e.end).point];
        if kept[fa.index()] && kept[fb.index()] && [start, end] == old_ends {
            continue;
        }
        // Worked out point by point, where the intersection has no closed form.
        let numeric = || {
            EdgeJob {
                old: e,
                surfaces: [&surfaces[fa.index()], &surfaces[fb.index()]],
                old_ends,
                ends: [start, end],
            }
            .solve()
        };
        let closed_form = if fa == fb {
            Some(seam(solid, id, &surfaces[fa.index()], start, end)?)
        } else if kept[fa.index()]
            && kept[fb.index()]
            && e.curve.distance(start).max(e.curve.distance(end)) <= freeform::ON_FACE
        {
            // The same curve, between ends that have moved along it.
            Some(e.curve.clone())
        } else if freeform(fa) || freeform(fb) {
            None
        } else {
            match ssi::intersect(&surfaces[fa.index()], &surfaces[fb.index()], middle) {
                Ssi::Unsupported => None,
                found => {
                    let curves = match found {
                        Ssi::Curves(curves) => curves,
                        _ => Vec::new(),
                    };
                    // The branch through the edge's new vertices (nearest where it used
                    // to be).
                    let branch = curves
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
                                "two faces no longer meet along their edge near ({:.3}, {:.3}, \
                                 {:.3})",
                                middle.x, middle.y, middle.z
                            ))
                        })?;
                    Some(branch)
                }
            }
        };
        let (curve, t0, t1) = match closed_form {
            Some(curve) => fit(e, curve, start, end).ok_or_else(|| {
                KernelError::InvalidResult(format!(
                    "the edge near ({:.3}, {:.3}, {:.3}) would shrink to nothing or turn round",
                    middle.x, middle.y, middle.z
                ))
            })?,
            None => numeric()?,
        };
        let edge = &mut out.edges[id.index()];
        edge.curve = curve;
        edge.t0 = t0;
        edge.t1 = t1;
    }

    if checked {
        crate::validate::validate(&out).map_err(|problems| {
            let list: Vec<&str> = problems.iter().map(|p| p.message.as_str()).collect();
            KernelError::InvalidResult(list.join("; "))
        })?;
    }
    Ok(out)
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
    // A freeform face that closes on itself: the same edge of its parameter rectangle.
    if let Surface::Nurbs(new) = new {
        let Surface::Nurbs(old) = &solid.face(solid.coedge_face(e.coedges[0])).surface else {
            return unsupported();
        };
        return freeform::seam(old, new, e).map_or_else(unsupported, Ok);
    }
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
