//! Fillets and chamfers on edges.
//!
//! **Method.** A blend is made with a *tool*: the sliver of space between the two faces
//! of an edge and the blend surface. In the plane across the edge the sliver is a triangle
//! with its corner on the edge, one side along each face and, opposite the corner, the
//! fillet's arc or the chamfer's line. Swept along the edge it is the tool, which is
//! subtracted from the body on a convex edge (it removes the sharp corner) and added to it
//! on a concave one (it fills the corner in). The boolean operations do the rest: the
//! tool's sides lie on the faces they came from and vanish there, and its curved or
//! slanted face becomes the blend, tangent to both faces.
//!
//! - A *straight* edge between two flat faces sweeps the section along the edge
//!   ([`crate::extrude`]): a fillet is a cylinder, a chamfer a plane.
//! - A *round* edge (a circle or an arc) between faces turned about the circle's axis (a
//!   flat face square to it, a cylinder or a cone) turns the section about that axis
//!   ([`crate::revolve`]): a fillet is a torus, a chamfer a cone.
//!
//! **Ends.** The tool ends where its edge ends. A face that closes the edge off there
//! (flat, and not along the edge) trims the tool: square to the edge, the tool simply
//! stops on it; at a slant, the tool is made longer and cut back with the face's plane.
//! Faces that run along the edge at its end (the next face of a smooth chain) need
//! nothing: the neighbouring edge's tool ends in the same section, and the two meet.
//!
//! **Corners.** Where blended edges meet at a corner, their tools overlap and the blends
//! meet in a mitre (fillets of one radius cross in ellipses). Where three fillets meet at
//! a square, convex corner, the corner is rounded with a ball first, which is what a
//! milling cutter or a mould would leave, and the three fillets run up to it.
//!
//! **Limits.** Every edge of one call takes the same size. Other edges (a line between a
//! flat and a round face, an ellipse) and ends on curved faces are refused with
//! [`KernelError::Unsupported`].

use std::f64::consts::{PI, TAU};

use peet_math::{DVec2, DVec3, Frame, Plane, tolerance};
use peet_sketch::region::{Loop, LoopEdge, Region};
use peet_sketch::{Curve, EntityId};

use crate::boolean::{BooleanOp, boolean_traced};
use crate::extrude::{ExtrudeFace, extrude_traced};
use crate::geom::{Curve3, Surface};
use crate::revolve::{RevolveAxis, RevolveFace, revolve_traced};
use crate::topo::{EdgeId, FaceId, VertexId};
use crate::{KernelError, Solid, primitive, transform};

/// The shape put on an edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Blend {
    /// A rounding of this radius, tangent to both faces.
    Fillet { radius: f64 },
    /// A flat cut this far back from the edge on both faces.
    Chamfer { distance: f64 },
}

/// Where a face of a blended solid came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BlendFace {
    /// (Part of) this face of the input.
    Original(FaceId),
    /// The fillet or chamfer on the edge with this index in the input list.
    Blend(usize),
    /// The flat step left at an end of that edge's blend, where nothing continues it.
    End(usize),
    /// The ball rounding the corner where three fillets meet: the index is that of the
    /// first of the three edges in the input list.
    Corner(usize),
}

/// A blended solid with the origin of its faces.
#[derive(Clone, Debug, PartialEq)]
pub struct Blended {
    pub solid: Solid,
    /// For each face of `solid` (by face index), what it is made of, sorted. Usually one
    /// entry; several where faces were merged.
    pub faces: Vec<Vec<BlendFace>>,
}

/// Edges meeting at less than this angle (or within it of a straight continuation) have
/// no corner to blend.
const MIN_ANGLE: f64 = 1e-3;

/// A face's normal within this (as a cosine) of being square to an edge's direction runs
/// along the edge there.
const ALONG: f64 = 1e-6;

/// Puts `blend` on `edges` of `solid`. Edges given twice are blended once.
pub fn blend(solid: &Solid, edges: &[EdgeId], blend: Blend) -> Result<Blended, KernelError> {
    let size = match blend {
        Blend::Fillet { radius } => radius,
        Blend::Chamfer { distance } => distance,
    };
    if !size.is_finite() || size <= 10.0 * tolerance::LINEAR {
        return Err(KernelError::InvalidInput(match blend {
            Blend::Fillet { .. } => "the fillet radius must be greater than zero".to_owned(),
            Blend::Chamfer { .. } => "the chamfer distance must be greater than zero".to_owned(),
        }));
    }
    if edges.is_empty() {
        return Err(KernelError::InvalidInput(
            "pick at least one edge".to_owned(),
        ));
    }
    let mut jobs: Vec<Job> = Vec::with_capacity(edges.len());
    for (index, &edge) in edges.iter().enumerate() {
        if edge.index() >= solid.edges.len() {
            return Err(KernelError::InvalidInput(
                "an edge to blend doesn't exist".to_owned(),
            ));
        }
        if jobs.iter().any(|j| j.edge == edge) {
            continue;
        }
        jobs.push(Job::new(solid, edge, index, blend)?);
    }

    // Corners where three fillets meet squarely get a ball; the fillets stop short of it.
    let mut body = Labelled {
        solid: solid.clone(),
        labels: solid
            .face_ids()
            .map(|f| vec![BlendFace::Original(f)])
            .collect(),
    };
    if let Blend::Fillet { radius } = blend {
        for corner in ball_corners(solid, &jobs) {
            let tool = corner_tool(&corner, radius, jobs[corner.jobs[0]].index)?;
            body = apply(&body, &tool, BooleanOp::Subtract)?;
            for (&j, at_start) in corner.jobs.iter().zip(corner.at_start) {
                jobs[j].set_back[usize::from(!at_start)] = Some(radius);
            }
        }
    }
    for job in &jobs {
        let tool = job.tool(solid, blend)?;
        let op = if job.section.convex {
            BooleanOp::Subtract
        } else {
            BooleanOp::Union
        };
        body = apply(&body, &tool, op)?;
    }
    for labels in &mut body.labels {
        labels.sort_unstable();
        labels.dedup();
    }
    Ok(Blended {
        solid: body.solid,
        faces: body.labels,
    })
}

/// The edges that continue `edge` smoothly, itself included: followed through every
/// vertex where exactly one other edge carries on in the same direction between faces
/// that also carry on smoothly.
pub fn tangent_chain(solid: &Solid, edge: EdgeId) -> Vec<EdgeId> {
    let mut chain = vec![edge];
    for forward in [true, false] {
        let mut current = edge;
        let mut leaving_end = forward;
        loop {
            let e = solid.edge(current);
            if e.is_closed() {
                break;
            }
            let (v, t) = if leaving_end {
                (e.end, e.curve.tangent(e.t1))
            } else {
                (e.start, -e.curve.tangent(e.t0))
            };
            let next = solid.edge_ids().find_map(|other| {
                if other == current || chain.contains(&other) {
                    return None;
                }
                let o = solid.edge(other);
                let (starts, dir) = if o.start == v {
                    (true, o.curve.tangent(o.t0))
                } else if o.end == v {
                    (false, -o.curve.tangent(o.t1))
                } else {
                    return None;
                };
                (dir.dot(t) > 1.0 - 1e-9 && sharp(solid, other)).then_some((other, starts))
            });
            let Some((other, starts)) = next else {
                break;
            };
            chain.push(other);
            current = other;
            leaving_end = starts;
        }
    }
    chain
}

/// Whether an edge is a real corner between two faces (not a seam, not smooth).
fn sharp(solid: &Solid, edge: EdgeId) -> bool {
    let e = solid.edge(edge);
    let [ca, cb] = e.coedges[..] else {
        return false;
    };
    let (fa, fb) = (solid.coedge_face(ca), solid.coedge_face(cb));
    let p = e.point_at_fraction(0.5);
    fa != fb
        && solid
            .face_normal_at(fa, p)
            .cross(solid.face_normal_at(fb, p))
            .length()
            > MIN_ANGLE
}

/// A solid with the origin of each face.
struct Labelled {
    solid: Solid,
    labels: Vec<Vec<BlendFace>>,
}

/// Combines the body with a tool, carrying the labels over.
fn apply(body: &Labelled, tool: &Labelled, op: BooleanOp) -> Result<Labelled, KernelError> {
    let traced = boolean_traced(&body.solid, &tool.solid, op)?;
    let labels = traced
        .sources
        .iter()
        .map(|sources| {
            sources
                .iter()
                .flat_map(|s| {
                    let from = if s.solid == 0 { body } else { tool };
                    from.labels[s.face.index()].iter().copied()
                })
                .collect()
        })
        .collect();
    Ok(Labelled {
        solid: traced.solid,
        labels,
    })
}

/// The corner across an edge, at one of its points.
#[derive(Clone, Copy, Debug)]
struct Section {
    /// The point on the edge.
    corner: DVec3,
    /// The edge's direction there.
    tangent: DVec3,
    /// Unit directions from the corner along each face, square to the edge.
    along_a: DVec3,
    along_b: DVec3,
    /// The angle between them.
    angle: f64,
    /// The material is between the two directions (else it is everything but).
    convex: bool,
}

/// What ends a blend at one end of its edge.
#[derive(Clone, Debug, Default)]
struct End {
    /// Planes to cut the tool back with, each with a point on the side to keep.
    trims: Vec<(Plane, DVec3)>,
}

/// One edge to blend.
struct Job {
    edge: EdgeId,
    /// Its index in the caller's list.
    index: usize,
    section: Section,
    /// At the edge's start and end.
    ends: [End; 2],
    /// How far the blend stops short of each end (for a ball at the corner).
    set_back: [Option<f64>; 2],
    /// How far the section reaches from the corner.
    reach: f64,
}

impl Job {
    fn new(solid: &Solid, edge: EdgeId, index: usize, blend: Blend) -> Result<Self, KernelError> {
        let unsupported = |m: &str| Err(KernelError::Unsupported(m.to_owned()));
        let e = solid.edge(edge);
        let [ca, cb] = e.coedges[..] else {
            return Err(KernelError::InvalidInput(
                "the edge is not between two faces".to_owned(),
            ));
        };
        let (fa, fb) = (solid.coedge_face(ca), solid.coedge_face(cb));
        if fa == fb {
            return unsupported(
                "the edge is a seam of one face, not a corner between two; pick a sharp edge",
            );
        }
        // The section at the edge's start.
        let corner = e.curve.point(e.t0);
        let tangent = e.curve.tangent(e.t0);
        let along = |c: crate::topo::CoedgeId, f: FaceId| {
            let dir = if solid.coedge(c).reversed {
                -tangent
            } else {
                tangent
            };
            // The face lies to the left of its coedge, seen from outside.
            solid.face_normal_at(f, corner).cross(dir).normalize()
        };
        let (along_a, along_b) = (along(ca, fa), along(cb, fb));
        let angle = along_a.dot(along_b).clamp(-1.0, 1.0).acos();
        if !(MIN_ANGLE..=PI - MIN_ANGLE).contains(&angle) {
            return Err(KernelError::InvalidInput(
                "the faces meet smoothly along the edge: there is no corner to blend".to_owned(),
            ));
        }
        let (sa, sb) = (&solid.face(fa).surface, &solid.face(fb).surface);
        match &e.curve {
            Curve3::Line(_) => {
                if !matches!((sa, sb), (Surface::Plane(_), Surface::Plane(_))) {
                    return unsupported(
                        "a straight edge can be blended between two flat faces; this one is \
                         on a curved face",
                    );
                }
            }
            Curve3::Circle(c) => {
                let turned = |s: &Surface| match s {
                    Surface::Plane(p) => {
                        tolerance::directions_parallel(p.normal(), c.frame.z_axis())
                    }
                    Surface::Cylinder(_) | Surface::Cone(_) => {
                        let f = s.revolution_frame().expect("a surface of revolution");
                        let w = c.frame.origin - f.origin;
                        tolerance::directions_parallel(f.z_axis(), c.frame.z_axis())
                            && (w - f.z_axis() * w.dot(f.z_axis())).length() <= tolerance::LINEAR
                    }
                    _ => false,
                };
                if !(turned(sa) && turned(sb)) {
                    return unsupported(
                        "a round edge can be blended where a flat face meets a cylinder or a \
                         cone about the same axis (the rim of a hole or a boss), or where two \
                         such round faces meet",
                    );
                }
            }
            Curve3::Ellipse(_) => {
                return unsupported("an elliptical edge can't be blended yet");
            }
        }
        let section = Section {
            corner,
            tangent,
            along_a,
            along_b,
            angle,
            convex: solid.face_normal_at(fb, corner).dot(along_a) < 0.0,
        };
        let reach = match blend {
            Blend::Fillet { radius } => radius / (angle / 2.0).tan(),
            Blend::Chamfer { distance } => distance,
        };
        let mut ends = [End::default(), End::default()];
        if !e.is_closed() {
            let middle = e.point_at_fraction(0.5);
            for (k, (v, t, out)) in [
                (e.start, e.t0, -e.curve.tangent(e.t0)),
                (e.end, e.t1, e.curve.tangent(e.t1)),
            ]
            .into_iter()
            .enumerate()
            {
                let at = e.curve.point(t);
                for face in vertex_faces(solid, v) {
                    if face == fa || face == fb {
                        continue;
                    }
                    let n = solid.face_normal_at(face, at);
                    if n.dot(out).abs() <= ALONG {
                        continue; // runs along the edge: nothing to trim against
                    }
                    let Surface::Plane(plane) = solid.face(face).surface else {
                        return unsupported(
                            "the edge ends on a curved face that cuts across it; blends can \
                             end on flat faces",
                        );
                    };
                    if tolerance::directions_parallel(n, out) {
                        continue; // square to the edge: the tool ends exactly there
                    }
                    if !matches!(e.curve, Curve3::Line(_)) {
                        return unsupported(
                            "the round edge ends on a face that isn't square to it",
                        );
                    }
                    ends[k].trims.push((plane, middle));
                }
            }
        }
        Ok(Self {
            edge,
            index,
            section,
            ends,
            set_back: [None, None],
            reach,
        })
    }

    /// The tool: the sliver between the faces and the blend, along the whole edge.
    fn tool(&self, solid: &Solid, blend: Blend) -> Result<Labelled, KernelError> {
        let e = solid.edge(self.edge);
        let s = &self.section;
        // A section that can't be turned about the edge's axis reaches the axis.
        let too_big = |_: KernelError| {
            KernelError::InvalidInput(
                "the blend is too big for this edge: it would reach the axis the edge turns                  about"
                    .to_owned(),
            )
        };
        let tag = |edge: usize| match edge {
            1 => BlendFace::Blend(self.index),
            // The tool's sides lie on the edge's own faces and vanish there.
            _ => BlendFace::End(self.index),
        };
        let mut tool = match &e.curve {
            Curve3::Line(_) => {
                let plane = Plane::from_origin_normal_x(s.corner, s.tangent, s.along_a)
                    .ok_or_else(|| KernelError::InvalidInput("a degenerate edge".to_owned()))?;
                let region = section_region(&plane, s, s.corner, blend);
                // Past each end that is cut back with a slanted face, far enough for the
                // cut to pass through the whole section.
                let margin = |k: usize, out: DVec3| {
                    self.ends[k]
                        .trims
                        .iter()
                        .map(|(p, _)| {
                            let slant = p.normal().dot(out).abs().max(0.05);
                            self.reach * (2.0 / slant + 1.0)
                        })
                        .fold(0.0, f64::max)
                };
                let length = e.t1 - e.t0;
                let from = self.set_back[0].unwrap_or(-margin(0, -s.tangent));
                let to = length - self.set_back[1].unwrap_or(-margin(1, s.tangent));
                if to - from <= tolerance::LINEAR {
                    return Err(KernelError::InvalidInput(
                        "the edge is too short for a blend of this size".to_owned(),
                    ));
                }
                let (tool, faces) = extrude_traced(&plane, &[region], from, to)?;
                Labelled {
                    solid: tool,
                    labels: faces
                        .iter()
                        .map(|f| {
                            vec![match *f {
                                ExtrudeFace::Side { edge, .. } => tag(edge),
                                _ => BlendFace::End(self.index),
                            }]
                        })
                        .collect(),
                }
            }
            Curve3::Circle(c) => {
                // The meridian plane through the edge's start: x away from the axis, y
                // along it.
                let radial = (s.corner - c.frame.origin).normalize();
                let axis = c.frame.z_axis();
                let plane = Plane::from_origin_normal_x(c.frame.origin, radial.cross(axis), radial)
                    .ok_or_else(|| KernelError::InvalidInput("a degenerate edge".to_owned()))?;
                let region = section_region(&plane, s, s.corner, blend);
                let turn = RevolveAxis {
                    origin: DVec2::ZERO,
                    dir: DVec2::Y,
                };
                let span = if e.is_closed() { TAU } else { e.t1 - e.t0 };
                let (tool, faces) =
                    revolve_traced(&plane, &[region], &turn, 0.0, span).map_err(too_big)?;
                Labelled {
                    solid: tool,
                    labels: faces
                        .iter()
                        .map(|f| {
                            vec![match *f {
                                RevolveFace::Side { edge, .. } => tag(edge),
                                _ => BlendFace::End(self.index),
                            }]
                        })
                        .collect(),
                }
            }
            Curve3::Ellipse(_) => unreachable!("refused when the job was made"),
        };
        // Cut the tool back with the slanted faces at its ends.
        let size = 4.0 * (solid.bounds().size().length() + self.reach);
        for end in &self.ends {
            for (plane, keep) in &end.trims {
                let inside = plane.signed_distance(*keep) < 0.0;
                let (lo, hi) = if inside { (-size, 0.0) } else { (0.0, size) };
                let block =
                    primitive::cuboid(DVec3::new(-size, -size, lo), DVec3::new(size, size, hi));
                let block = Labelled {
                    labels: vec![vec![BlendFace::End(self.index)]; block.faces.len()],
                    solid: transform::solid(&block, &plane.frame),
                };
                tool = apply(&tool, &block, BooleanOp::Intersect)?;
                if tool.solid.faces.is_empty() {
                    return Err(KernelError::InvalidInput(
                        "the edge is too short for a blend of this size".to_owned(),
                    ));
                }
            }
        }
        Ok(tool)
    }
}

/// The faces around a vertex.
fn vertex_faces(solid: &Solid, vertex: VertexId) -> Vec<FaceId> {
    let mut out = Vec::new();
    for e in &solid.edges {
        if e.start == vertex || e.end == vertex {
            for &c in &e.coedges {
                let f = solid.coedge_face(c);
                if !out.contains(&f) {
                    out.push(f);
                }
            }
        }
    }
    out
}

/// The tool's section in `plane` (which is square to the edge at `corner`): from the
/// corner along face A, across the blend, and back along face B. The loop's edges are
/// numbered 0 (on A), 1 (the blend), 2 (on B).
fn section_region(plane: &Plane, s: &Section, corner: DVec3, blend: Blend) -> Region {
    let to_2d = |v: DVec3| plane.frame.vector_to_local(v).truncate();
    let p = plane.to_plane_coords(corner);
    let (a, b) = (to_2d(s.along_a).normalize(), to_2d(s.along_b).normalize());
    let (ta, tb, across) = match blend {
        Blend::Chamfer { distance } => {
            let (ta, tb) = (p + a * distance, p + b * distance);
            (ta, tb, (Curve::Line { a: ta, b: tb }, false))
        }
        Blend::Fillet { radius } => {
            let d = radius / (s.angle / 2.0).tan();
            let (ta, tb) = (p + a * d, p + b * d);
            let center = p + (a + b).normalize() * (radius / (s.angle / 2.0).sin());
            // The arc turns its hollow side to the corner, so going from A's tangent
            // point to B's it runs against the turn from A to B.
            let sweep = PI - s.angle;
            if a.perp_dot(b) > 0.0 {
                // Clockwise from TA to TB: the counter-clockwise arc from TB, reversed.
                let arc = Curve::Arc {
                    center,
                    radius,
                    start_angle: (tb - center).to_angle(),
                    sweep,
                };
                (ta, tb, (arc, true))
            } else {
                let arc = Curve::Arc {
                    center,
                    radius,
                    start_angle: (ta - center).to_angle(),
                    sweep,
                };
                (ta, tb, (arc, false))
            }
        }
    };
    let edge = |entity: u32, curve: Curve, reversed: bool| LoopEdge {
        entity: EntityId(entity),
        curve,
        reversed,
    };
    Region {
        outer: Loop {
            edges: vec![
                edge(0, Curve::Line { a: p, b: ta }, false),
                edge(1, across.0, across.1),
                edge(2, Curve::Line { a: tb, b: p }, false),
            ],
            signed_area: 0.5 * (ta - p).perp_dot(tb - p),
        },
        holes: Vec::new(),
    }
}

/// A convex corner where three fillets meet squarely.
struct BallCorner {
    vertex: DVec3,
    /// Unit directions from the vertex along the three edges.
    dirs: [DVec3; 3],
    /// The three jobs, and whether each has the corner at its edge's start.
    jobs: [usize; 3],
    at_start: [bool; 3],
}

/// The corners to round with a ball: vertices where exactly three edges meet, all three
/// straight, convex, being filleted, and square to each other.
fn ball_corners(solid: &Solid, jobs: &[Job]) -> Vec<BallCorner> {
    let mut out = Vec::new();
    for v in solid.vertex_ids() {
        let at: Vec<EdgeId> = solid
            .edge_ids()
            .filter(|&e| solid.edge(e).start == v || solid.edge(e).end == v)
            .collect();
        if at.len() != 3 {
            continue;
        }
        let mut found = Vec::with_capacity(3);
        for &e in &at {
            let Some(j) = jobs.iter().position(|j| j.edge == e) else {
                break;
            };
            let edge = solid.edge(e);
            let Curve3::Line(l) = edge.curve else {
                break;
            };
            if !jobs[j].section.convex {
                break;
            }
            let at_start = edge.start == v;
            found.push((j, at_start, if at_start { l.dir } else { -l.dir }));
        }
        if found.len() != 3 {
            continue;
        }
        let dirs = [found[0].2, found[1].2, found[2].2];
        let square = |a: DVec3, b: DVec3| a.dot(b).abs() <= 1e-9;
        if !(square(dirs[0], dirs[1]) && square(dirs[1], dirs[2]) && square(dirs[0], dirs[2])) {
            continue;
        }
        out.push(BallCorner {
            vertex: solid.vertex(v).point,
            dirs,
            jobs: [found[0].0, found[1].0, found[2].0],
            at_start: [found[0].1, found[1].1, found[2].1],
        });
    }
    out
}

/// The tool for a ball corner: the cube of side `radius` at the corner, less the ball
/// centred on its inner corner. What it leaves of the body there is an eighth of a ball.
fn corner_tool(corner: &BallCorner, radius: f64, label: usize) -> Result<Labelled, KernelError> {
    let [x, y, z] = corner.dirs;
    // A right-handed frame along the edges.
    let (x, y) = if x.cross(y).dot(z) > 0.0 {
        (x, y)
    } else {
        (y, x)
    };
    let frame = Frame::from_origin_z_x(corner.vertex, x.cross(y), x)
        .ok_or_else(|| KernelError::InvalidInput("a degenerate corner".to_owned()))?;
    let cube = transform::solid(
        &primitive::cuboid(DVec3::ZERO, DVec3::splat(radius)),
        &frame,
    );
    let centre = Frame {
        origin: frame.to_world(DVec3::splat(radius)),
        rotation: frame.rotation,
    };
    let ball = primitive::ball(&centre, radius)?;
    let cube = Labelled {
        labels: vec![vec![BlendFace::End(label)]; cube.faces.len()],
        solid: cube,
    };
    let ball = Labelled {
        labels: vec![vec![BlendFace::Corner(label)]; ball.faces.len()],
        solid: ball,
    };
    apply(&cube, &ball, BooleanOp::Subtract)
}

#[cfg(test)]
mod tests;
