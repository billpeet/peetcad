//! Mates: what holds the components of an assembly together.
//!
//! A [`Mate`] relates two components through geometry of their parts: a flat face (a
//! plane), a round face or a round edge (its axis), a straight edge (a line) or a corner
//! (a point). Its ends are persistent references ([`crate::naming`]) with the component
//! they are on, so a mate follows its faces through edits to the part.
//!
//! **Solving.** Every component that is not fixed has six unknowns: where its origin is,
//! and a rotation vector `w` that turns it from the orientation it had when the solve
//! started (`R = exp(w) R₀`). A mate becomes one to six residual equations in the world
//! positions `P = t + R p` and directions `D = R d` of its two ends:
//!
//! | equation | residual |
//! |---|---|
//! | `Dot` | `Dₐ · D_b − c`: two directions at an angle (square to each other for `c = 0`) |
//! | `Offset` | `Dₐ · (P_b − Pₐ) − c`: a point at a distance along a direction of the other |
//! | `Distance` | `|P_b − Pₐ| − c` |
//! | `LineDistance` | the distance of `P_b` from the line through `Pₐ` along `Dₐ`, `− c` |
//!
//! So "two planes coincide" is two `Dot`s (two directions in the first plane are square
//! to the second's normal) and an `Offset`; "two axes are in line" is two `Dot`s and two
//! `Offset`s. Lengths are in mm and the rotation unknowns are scaled by the component's
//! size, so every entry of the Jacobian is of order one.
//!
//! The equations are solved together by `peet-solve`, with minimum norm steps: the
//! components move as little as they can from where they are. Where they are is
//! therefore the suggestion: placing a mated component somewhere and rebuilding brings
//! it to the nearest place the mates allow.
//!
//! Components that mates join (directly or through others) are a *group*, solved on its
//! own; a fixed component joins nothing, since it doesn't move. A group whose mates
//! already hold is left exactly as it is, so adding a mate to one corner of an assembly
//! moves nothing in another, and an assembly that is solved stays bit for bit the same
//! however often it is rebuilt.
//!
//! **Dragging.** A [`Drag`] pulls a point of a component towards a place. Before the
//! mates are solved, the component's group is moved in steps, as the sketcher drags: a
//! step is the least squares answer to the mates' equations, linearised, together with
//! weak ones that ask the point to be at the place; then the mates are solved exactly
//! from there. The mates far outweigh the pull, so a step keeps them. The steps stop
//! when the point gets no nearer. This is done first without letting the dragged
//! component turn, then letting it: so a part that can slide to the place slides there,
//! and a hinged one swings round to follow. A component with no mates is simply moved.
//!
//! **Which way round.** Two planes are parallel with their normals opposed (faces
//! against each other, the default) or the same way (`flip`). Both satisfy the same
//! equations, and the solve goes to the nearer one, so a component that is the wrong way
//! round is turned over first.
//!
//! **Conflicts.** If the mates can't all hold, they are added one at a time in their
//! order; each that can't be satisfied with those before it is flagged and left out, so
//! the rest still hold.

use std::collections::HashMap;
use std::sync::Arc;

use peet_kernel::{Curve3, Surface};
use peet_math::{DQuat, DVec3, Frame};
use peet_sketch::expr::Parameters;
use peet_solve::{Equation, MAX_SLOTS, Options, Problem, SKIP, Space};
use serde::{Deserialize, Serialize};

use crate::assembly::CompId;
use crate::feature::{Scalar, ScalarKind};
use crate::naming::{Body, EdgeRef, FaceRef, VertexRef, find_edge, find_face, find_vertex};
use crate::regen::Status;

/// Identifies a mate within its assembly. Ids are not reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MateId(pub u32);

/// The geometry at one end of a mate, in its part.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MateGeom {
    Face(FaceRef),
    Edge(EdgeRef),
    Vertex(VertexRef),
}

/// One end of a mate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MateEnd {
    /// The component: the assembly's own first, then (for a part inside a sub-assembly)
    /// the components below it.
    pub path: Vec<CompId>,
    /// The geometry, or `None` for the component as a whole (a fasten mate).
    pub geom: Option<MateGeom>,
}

impl MateEnd {
    /// The component of the assembly this end is on.
    pub fn component(&self) -> Option<CompId> {
        self.path.first().copied()
    }
}

/// What a mate asks for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MateKind {
    /// The two ends lie in each other: faces against each other, a point on a plane,
    /// lines in line.
    Coincident,
    /// Two axes in line.
    Concentric,
    Parallel,
    /// The two ends a length apart.
    Distance(Scalar),
    /// The directions of the two ends at an angle (degrees).
    Angle(Scalar),
    /// The second component held where it is relative to the first: its placement in
    /// the first's coordinates.
    Fasten(Frame),
}

impl MateKind {
    /// The kind in one word, for names and messages.
    pub fn word(&self) -> &'static str {
        match self {
            Self::Coincident => "coincident",
            Self::Concentric => "concentric",
            Self::Parallel => "parallel",
            Self::Distance(_) => "distance",
            Self::Angle(_) => "angle",
            Self::Fasten(_) => "fasten",
        }
    }

    /// The start of automatic names: `Coincident1`.
    pub(crate) fn name_prefix(&self) -> &'static str {
        match self {
            Self::Coincident => "Coincident",
            Self::Concentric => "Concentric",
            Self::Parallel => "Parallel",
            Self::Distance(_) => "Distance",
            Self::Angle(_) => "Angle",
            Self::Fasten(_) => "Fasten",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mate {
    pub id: MateId,
    /// Unique in the assembly: "Coincident1".
    pub name: String,
    pub kind: MateKind,
    pub a: MateEnd,
    pub b: MateEnd,
    /// For two planes: their normals the same way, instead of against each other.
    pub flip: bool,
    pub suppressed: bool,
}

// ---- Geometry of an end ----

/// What an end is, in its component's coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Element {
    /// A point of the plane and its outward normal.
    Plane(DVec3, DVec3),
    /// A point of the line and its direction.
    Line(DVec3, DVec3),
    Point(DVec3),
}

impl Element {
    fn word(&self) -> &'static str {
        match self {
            Self::Plane(..) => "a flat face",
            Self::Line(..) => "an axis or a straight edge",
            Self::Point(_) => "a point",
        }
    }
}

/// A body of a component: the components below it down to its part (none for a part
/// placed directly), the body, and where the part is in the component's coordinates.
pub(crate) type ComponentBody = (Vec<CompId>, Arc<Body>, Frame);

/// A component as the solve sees it.
pub(crate) struct Placed {
    pub id: CompId,
    pub name: String,
    pub frame: Frame,
    pub fixed: bool,
    pub bodies: Vec<ComponentBody>,
}

impl Placed {
    /// A length that one radian of turning moves the component's far points by.
    fn size(&self) -> f64 {
        let mut bounds = peet_math::Aabb::EMPTY;
        for (_, body, rel) in &self.bodies {
            let b = body.solid.bounds();
            if b.min.cmple(b.max).all() {
                bounds.extend(rel.to_world(b.min));
                bounds.extend(rel.to_world(b.max));
            }
        }
        if bounds.min.cmple(bounds.max).all() {
            (bounds.size().length() * 0.5).max(1.0)
        } else {
            1.0
        }
    }
}

fn element(end: &MateEnd, component: &Placed) -> Result<Element, String> {
    let name = &component.name;
    let Some(geom) = &end.geom else {
        return Ok(Element::Point(DVec3::ZERO));
    };
    let inner = &end.path[1..];
    let (bodies, frames): (Vec<Arc<Body>>, Vec<Frame>) = component
        .bodies
        .iter()
        .filter(|(path, ..)| path == inner)
        .map(|(_, body, rel)| (body.clone(), *rel))
        .unzip();
    let gone = |what: &str| {
        format!(
            "The {what} of {name} it was on is gone: the part was changed. Edit the mate to pick it again, or delete the mate."
        )
    };
    let (found, local) = match geom {
        MateGeom::Face(r) => {
            let f = find_face(&bodies, r).ok_or_else(|| gone("face"))?;
            let body = &bodies[f.body];
            let face = body.solid.face(f.id);
            let e = match &face.surface {
                Surface::Plane(plane) => {
                    let n = if face.reversed {
                        -plane.normal()
                    } else {
                        plane.normal()
                    };
                    Element::Plane(plane.project_point(body.face_center(f.id)), n)
                }
                Surface::Cylinder(c) => Element::Line(c.axis_origin(), c.axis()),
                Surface::Cone(c) => Element::Line(c.axis_origin(), c.frame.z_axis()),
                Surface::Sphere(s) => Element::Point(s.frame.origin),
                Surface::Torus(_) | Surface::Nurbs(_) => {
                    return Err(format!(
                        "The face of {name} is neither flat nor round about one axis, so a mate has nothing to hold on to: pick a flat face, a hole or a shaft, an edge or a corner."
                    ));
                }
            };
            (f.body, e)
        }
        MateGeom::Edge(r) => {
            let f = find_edge(&bodies, r).ok_or_else(|| gone("edge"))?;
            let e = match &bodies[f.body].solid.edge(f.id).curve {
                Curve3::Line(l) => Element::Line(l.origin, l.dir),
                Curve3::Circle(c) => Element::Line(c.frame.origin, c.frame.z_axis()),
                Curve3::Ellipse(_) | Curve3::Nurbs(_) => {
                    return Err(format!(
                        "The edge of {name} is neither straight nor round, so a mate has nothing to hold on to: pick a straight edge, a round one (its axis), a face or a corner."
                    ));
                }
            };
            (f.body, e)
        }
        MateGeom::Vertex(r) => {
            let f = find_vertex(&bodies, r).ok_or_else(|| gone("corner"))?;
            (
                f.body,
                Element::Point(bodies[f.body].solid.vertex(f.id).point),
            )
        }
    };
    let rel = frames[found];
    Ok(match local {
        Element::Plane(p, n) => Element::Plane(rel.to_world(p), rel.vector_to_world(n)),
        Element::Line(p, d) => Element::Line(rel.to_world(p), rel.vector_to_world(d)),
        Element::Point(p) => Element::Point(rel.to_world(p)),
    })
}

// ---- Equations ----

#[derive(Clone, Copy, Debug, PartialEq)]
enum EqKind {
    Dot,
    Offset,
    Distance,
    LineDistance,
}

/// One residual before it is tied to components: `first` says which end carries the
/// direction (`true`: end A).
#[derive(Clone, Copy, Debug)]
struct Proto {
    kind: EqKind,
    first_is_a: bool,
    /// Direction and point on the first end, direction and point on the other.
    d1: DVec3,
    p1: DVec3,
    d2: DVec3,
    p2: DVec3,
    target: f64,
}

/// One side of an equation: a component's base orientation and scale, and the slots of
/// its unknowns.
#[derive(Clone, Copy, Debug)]
struct Side {
    base: DQuat,
    scale: f64,
    dir: DVec3,
    point: DVec3,
}

#[derive(Clone, Debug)]
struct Eq {
    kind: EqKind,
    slots: [u32; MAX_SLOTS],
    first: Side,
    other: Side,
    target: f64,
    /// What the residual is multiplied by: 1 for a mate, less for a drag's pull.
    weight: f64,
}

/// `J_l(w)ᵀ v`, with `J_l` the left Jacobian of the rotation `exp(w)`: turns the
/// gradient by a small rotation of the world into the gradient by `w`.
fn rotation_gradient(w: DVec3, v: DVec3) -> DVec3 {
    let theta = w.length();
    let (b, c) = if theta < 1e-4 {
        (
            0.5 - theta * theta / 24.0,
            1.0 / 6.0 - theta * theta / 120.0,
        )
    } else {
        (
            (1.0 - theta.cos()) / (theta * theta),
            (theta - theta.sin()) / (theta * theta * theta),
        )
    };
    v - b * w.cross(v) + c * w.cross(w.cross(v))
}

impl Eq {
    /// The world point and direction of a side, with its rotation vector.
    fn world(side: &Side, v: &[f64], at: usize) -> (DVec3, DVec3, DVec3, DVec3) {
        let t = DVec3::new(v[at], v[at + 1], v[at + 2]);
        let w = DVec3::new(v[at + 3], v[at + 4], v[at + 5]) / side.scale;
        let r = DQuat::from_scaled_axis(w) * side.base;
        let arm = r * side.point;
        (t + arm, r * side.dir, arm, w)
    }
}

impl Equation for Eq {
    fn slots(&self) -> &[u32] {
        &self.slots
    }

    fn eval(&self, v: &[f64], g: &mut [f64; MAX_SLOTS]) -> f64 {
        let (s1, s2) = (self.slots[0] as usize, self.slots[6] as usize);
        let (p1, d1, arm1, w1) = Self::world(&self.first, v, s1);
        let (p2, d2, arm2, w2) = Self::world(&self.other, v, s2);
        let r = p2 - p1;
        // The residual, and its gradient by the two world points and directions.
        let (f, gp1, gd1, gp2, gd2) = match self.kind {
            EqKind::Dot => (d1.dot(d2), DVec3::ZERO, d2, DVec3::ZERO, d1),
            EqKind::Offset => (d1.dot(r), -d1, r, d1, DVec3::ZERO),
            EqKind::Distance => {
                let e = r.try_normalize().unwrap_or(DVec3::X);
                (r.length(), -e, DVec3::ZERO, e, DVec3::ZERO)
            }
            EqKind::LineDistance => {
                let along = r.dot(d1);
                let q = r - along * d1;
                let e = q
                    .try_normalize()
                    .unwrap_or_else(|| d1.any_orthonormal_vector());
                (q.length(), -e, -along * e, e, DVec3::ZERO)
            }
        };
        let turn1 = rotation_gradient(w1, arm1.cross(gp1) + d1.cross(gd1)) / self.first.scale;
        let turn2 = rotation_gradient(w2, arm2.cross(gp2) + d2.cross(gd2)) / self.other.scale;
        g[..3].copy_from_slice(&gp1.to_array());
        g[3..6].copy_from_slice(&turn1.to_array());
        g[6..9].copy_from_slice(&gp2.to_array());
        g[9..12].copy_from_slice(&turn2.to_array());
        for x in g.iter_mut() {
            *x *= self.weight;
        }
        (f - self.target) * self.weight
    }
}

/// Two unit directions square to `d` and to each other.
fn square_to(d: DVec3) -> (DVec3, DVec3) {
    let u = d.any_orthonormal_vector();
    (u, d.cross(u).normalize())
}

fn proto(kind: EqKind, first_is_a: bool, d1: DVec3, p1: DVec3, d2: DVec3, p2: DVec3) -> Proto {
    Proto {
        kind,
        first_is_a,
        d1,
        p1,
        d2,
        p2,
        target: 0.0,
    }
}

/// The direction `d2` of the other end is along `d1` (either way): two equations.
fn parallel(out: &mut Vec<Proto>, first_is_a: bool, d1: DVec3, d2: DVec3) {
    let (u, v) = square_to(d1);
    for dir in [u, v] {
        out.push(proto(
            EqKind::Dot,
            first_is_a,
            dir,
            DVec3::ZERO,
            d2,
            DVec3::ZERO,
        ));
    }
}

/// The point `p2` of the other end is on the line through `p1` along `d1`: two equations.
fn on_line(out: &mut Vec<Proto>, first_is_a: bool, p1: DVec3, d1: DVec3, p2: DVec3) {
    let (u, v) = square_to(d1);
    for dir in [u, v] {
        out.push(proto(EqKind::Offset, first_is_a, dir, p1, DVec3::ZERO, p2));
    }
}

/// The point `p2` of the other end is `distance` along `n` from the plane through `p1`.
fn on_plane(out: &mut Vec<Proto>, first_is_a: bool, p1: DVec3, n: DVec3, p2: DVec3, distance: f64) {
    out.push(Proto {
        target: distance,
        ..proto(EqKind::Offset, first_is_a, n, p1, DVec3::ZERO, p2)
    });
}

/// The equations of a mate between two elements, each in its component's coordinates.
fn equations(
    kind: &MateKind,
    a: Element,
    b: Element,
    params: &Parameters,
) -> Result<Vec<Proto>, String> {
    use Element::{Line, Plane, Point};
    let mut out = Vec::new();
    let wrong = |takes: &str| {
        format!(
            "A {} mate can't join {} and {}: it takes {takes}.",
            kind.word(),
            a.word(),
            b.word()
        )
    };
    match kind {
        MateKind::Fasten(relative) => {
            // B's origin stays where it is in A, and B's axes stay along the same
            // directions of A.
            for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                out.push(proto(
                    EqKind::Offset,
                    true,
                    axis,
                    relative.origin,
                    DVec3::ZERO,
                    DVec3::ZERO,
                ));
            }
            for (i, j) in [
                (DVec3::X, DVec3::Y),
                (DVec3::Y, DVec3::Z),
                (DVec3::Z, DVec3::X),
            ] {
                out.push(proto(
                    EqKind::Dot,
                    true,
                    relative.rotation * i,
                    DVec3::ZERO,
                    j,
                    DVec3::ZERO,
                ));
            }
        }
        MateKind::Coincident | MateKind::Concentric => match (a, b) {
            (Line(pa, da), Line(pb, db)) => {
                parallel(&mut out, true, da, db);
                on_line(&mut out, true, pa, da, pb);
            }
            _ if *kind == MateKind::Concentric => {
                return Err(wrong(
                    "two axes: round faces (holes, shafts) or round edges",
                ));
            }
            (Plane(pa, na), Plane(pb, nb)) => {
                parallel(&mut out, true, na, nb);
                on_plane(&mut out, true, pa, na, pb, 0.0);
            }
            (Point(pa), Point(pb)) => {
                for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                    out.push(proto(EqKind::Offset, true, axis, pa, DVec3::ZERO, pb));
                }
            }
            (Plane(pa, na), Point(pb)) => on_plane(&mut out, true, pa, na, pb, 0.0),
            (Point(pa), Plane(pb, nb)) => on_plane(&mut out, false, pb, nb, pa, 0.0),
            (Line(pa, da), Point(pb)) => on_line(&mut out, true, pa, da, pb),
            (Point(pa), Line(pb, db)) => on_line(&mut out, false, pb, db, pa),
            (Plane(pa, na), Line(pb, db)) => {
                out.push(proto(EqKind::Dot, true, na, DVec3::ZERO, db, DVec3::ZERO));
                on_plane(&mut out, true, pa, na, pb, 0.0);
            }
            (Line(pa, da), Plane(pb, nb)) => {
                out.push(proto(EqKind::Dot, false, nb, DVec3::ZERO, da, DVec3::ZERO));
                on_plane(&mut out, false, pb, nb, pa, 0.0);
            }
        },
        MateKind::Parallel => match (a, b) {
            (Plane(_, da), Plane(_, db)) | (Line(_, da), Line(_, db)) => {
                parallel(&mut out, true, da, db);
            }
            (Plane(_, n), Line(_, d)) => {
                out.push(proto(EqKind::Dot, true, n, DVec3::ZERO, d, DVec3::ZERO));
            }
            (Line(_, d), Plane(_, n)) => {
                out.push(proto(EqKind::Dot, false, n, DVec3::ZERO, d, DVec3::ZERO));
            }
            _ => return Err(wrong("flat faces, axes and straight edges")),
        },
        MateKind::Distance(value) => {
            let d = value
                .evaluate(ScalarKind::Length, params)
                .map_err(|m| format!("Distance: {m}."))?;
            if d < 0.0 {
                return Err(
                    "The distance is negative: give it as a positive length (flip turns a face the other way round)."
                        .to_owned(),
                );
            }
            // Between two points, or a point and a line, a distance of nothing has no
            // direction to move along: that is what coincident is for.
            let needs_length = |d: f64| {
                if d < 1e-9 {
                    Err("The distance is zero: use a coincident mate for that.".to_owned())
                } else {
                    Ok(d)
                }
            };
            let line_distance = |first_is_a, p1, d1, p2, d| Proto {
                target: d,
                ..proto(EqKind::LineDistance, first_is_a, d1, p1, DVec3::ZERO, p2)
            };
            match (a, b) {
                (Plane(pa, na), Plane(pb, nb)) => {
                    parallel(&mut out, true, na, nb);
                    on_plane(&mut out, true, pa, na, pb, d);
                }
                (Plane(pa, na), Point(pb)) => on_plane(&mut out, true, pa, na, pb, d),
                (Point(pa), Plane(pb, nb)) => on_plane(&mut out, false, pb, nb, pa, d),
                (Plane(pa, na), Line(pb, db)) => {
                    out.push(proto(EqKind::Dot, true, na, DVec3::ZERO, db, DVec3::ZERO));
                    on_plane(&mut out, true, pa, na, pb, d);
                }
                (Line(pa, da), Plane(pb, nb)) => {
                    out.push(proto(EqKind::Dot, false, nb, DVec3::ZERO, da, DVec3::ZERO));
                    on_plane(&mut out, false, pb, nb, pa, d);
                }
                (Point(pa), Point(pb)) => out.push(Proto {
                    target: needs_length(d)?,
                    ..proto(EqKind::Distance, true, DVec3::ZERO, pa, DVec3::ZERO, pb)
                }),
                (Line(pa, da), Line(pb, db)) => {
                    parallel(&mut out, true, da, db);
                    out.push(line_distance(true, pa, da, pb, needs_length(d)?));
                }
                (Line(pa, da), Point(pb)) => {
                    out.push(line_distance(true, pa, da, pb, needs_length(d)?));
                }
                (Point(pa), Line(pb, db)) => {
                    out.push(line_distance(false, pb, db, pa, needs_length(d)?));
                }
            }
        }
        MateKind::Angle(value) => {
            let degrees = value
                .evaluate(ScalarKind::Angle, params)
                .map_err(|m| format!("Angle: {m}."))?;
            if !(degrees > 1e-6 && degrees < 180.0 - 1e-6) {
                return Err(
                    "The angle must be between 0 and 180 degrees, and neither: for those, use a parallel mate."
                        .to_owned(),
                );
            }
            let (da, db) = match (a, b) {
                (Plane(_, da) | Line(_, da), Plane(_, db) | Line(_, db)) => (da, db),
                _ => return Err(wrong("flat faces, axes and straight edges")),
            };
            out.push(Proto {
                target: degrees.to_radians().cos(),
                ..proto(EqKind::Dot, true, da, DVec3::ZERO, db, DVec3::ZERO)
            });
        }
    }
    Ok(out)
}

// ---- Solving ----

/// A pull on a component: a point of it towards a place. The component goes as far as
/// its mates let it, turning only if it must.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    pub component: CompId,
    /// The point, in the component's coordinates.
    pub point: DVec3,
    /// Where it should go, in the assembly's.
    pub to: DVec3,
}

/// How much a drag's pull on its point weighs against a mate (1). Small, so that a
/// step keeps the mates even when the place is out of reach; not so small that the
/// solver's damping swallows it.
const PULL: f64 = 3e-3;

/// What solving the mates of an assembly came to.
pub(crate) struct Solved {
    /// Where each component is now (those the solve moved, and those it didn't).
    pub frames: HashMap<CompId, Frame>,
    pub statuses: HashMap<MateId, Status>,
    /// How many ways the components can still move: six for each that is not fixed,
    /// less what the mates hold.
    pub freedom: usize,
    /// How many ways each component can still move, on its own or along with others.
    pub component_freedom: HashMap<CompId, usize>,
}

/// What is kept between solves of the same mates on the same components, wherever the
/// components are: both are slower to work out than the solve itself.
#[derive(Default)]
pub(crate) struct Memo {
    key: u64,
    /// The freedom the mates leave (together, and for each component by its index), if
    /// they all hold (only then does it depend on which mates there are alone).
    freedom: Option<(usize, Vec<usize>)>,
    /// The solver's structure for each group.
    problems: Vec<Kept>,
}

/// A solver's structure, with the unknowns and equations it is for.
type Kept = (Vec<u32>, Problem);

const HARD: Options = Options {
    max_iter: 200,
    hard: true,
    dogleg: true,
};

/// The equations of every mate that can be set up, and the status of those that can't.
struct System {
    eqs: Vec<Eq>,
    /// The equations of each mate, as a range of `eqs`.
    ranges: Vec<(MateId, std::ops::Range<usize>)>,
}

fn slots_of(index: usize) -> [u32; 6] {
    let base = (index * 6) as u32;
    [base, base + 1, base + 2, base + 3, base + 4, base + 5]
}

/// The values of the unknowns for the components where they are: the origin, and no
/// turn from the base orientation.
fn start_values(components: &[Placed]) -> Vec<f64> {
    let mut vals = Vec::with_capacity(components.len() * 6);
    for c in components {
        vals.extend(c.frame.origin.to_array());
        vals.extend([0.0; 3]);
    }
    vals
}

/// Solves `eqs` (indices into `system.eqs`) for the unknowns of the free components they
/// touch. Returns whether every one of them holds.
///
/// With `kept`, the problem's structure is taken from it if it has one for exactly
/// these equations, and left in `used` for the next solve.
///
/// `exactly` solves them even if they already hold to within the tolerance (after a
/// drag's loose solve, which leaves them a little off).
fn solve_equations(
    system: &System,
    eqs: &[u32],
    components: &[Placed],
    vals: &mut [f64],
    kept: Option<(&mut Vec<Kept>, &mut Vec<Kept>)>,
    exactly: bool,
) -> bool {
    if eqs.is_empty() {
        return true;
    }
    let vars = unknowns(system, eqs, components);
    // The structure of the problem (which equation reads which unknown) is kept from
    // the last solve of the same equations: setting it up costs more than solving.
    let signature = signature(system, &vars, eqs, false);
    let new = |vars| Problem::new(vars, eqs.to_vec(), &system.eqs, Space::Equations);
    let Some((kept, used)) = kept else {
        return solve_problem(&mut new(vars), system, vals, exactly);
    };
    let mut problem = take_kept(kept, used, &signature).unwrap_or_else(|| new(vars));
    let solved = solve_problem(&mut problem, system, vals, exactly);
    used.push((signature, problem));
    solved
}

/// The slots of the free components that `eqs` read (a slot beyond the components is
/// a drag's target: a constant).
fn unknowns(system: &System, eqs: &[u32], components: &[Placed]) -> Vec<u32> {
    let mut vars: Vec<u32> = eqs
        .iter()
        .flat_map(|e| system.eqs[*e as usize].slots)
        .filter(|s| components.get(*s as usize / 6).is_some_and(|c| !c.fixed))
        .collect();
    vars.sort_unstable();
    vars.dedup();
    vars
}

fn solve_problem(problem: &mut Problem, system: &System, vals: &mut [f64], exactly: bool) -> bool {
    let mut residuals = Vec::new();
    problem.residuals(&system.eqs, vals, &mut residuals);
    // Already there: nothing is moved, so an assembly that is solved stays as it is.
    if !exactly && peet_solve::max_abs(&residuals) <= SKIP {
        return true;
    }
    problem.solve(&system.eqs, vals, HARD).converged
}

/// The signature a problem is kept under: its unknowns, its equations and their slots
/// (and which of the two kinds of problem it is).
fn signature(system: &System, vars: &[u32], eqs: &[u32], loose: bool) -> Vec<u32> {
    let mut out = vars.to_vec();
    out.push(if loose { u32::MAX - 1 } else { u32::MAX });
    for e in eqs {
        // Which equation it is, too: the problem keeps the equations' indices.
        out.push(*e);
        out.extend(system.eqs[*e as usize].slots);
    }
    out
}

/// The problem kept under `signature`, from the last solve or from earlier in this one.
fn take_kept(kept: &mut Vec<Kept>, used: &mut Vec<Kept>, signature: &[u32]) -> Option<Problem> {
    for list in [kept, used] {
        if let Some(found) = list.iter().position(|(s, _)| s == signature) {
            return Some(list.swap_remove(found).1);
        }
    }
    None
}

/// Most steps of a drag in one solve, and the most a component turns in one step.
const DRAG_STEPS: usize = 60;
/// The point is there when it is this near (mm): far below what a screen shows, and
/// the next frame of a drag carries on from here.
const DRAG_NEAR: f64 = 5e-4;
/// How far the point is pulled in one step, in mm.
const DRAG_REACH: f64 = 10.0;
const DRAG_TURN: f64 = 0.35;
/// How often a step that doesn't help is halved before the drag gives up.
const DRAG_HALVINGS: usize = 8;

/// A drag: the dragged component's group is moved towards where the pull wants it, in
/// steps. Each step is the least squares answer to the group's mates (`hard`),
/// linearised, with the pull (`soft`) at its small weight; then the mates are solved
/// exactly from there. The steps stop when one no longer brings the point (`point`
/// gives where it is) nearer to where it should go (`to`). `target` is the slot the
/// equations of the pull read the place to pull to from.
///
/// It is tried first without letting the dragged component turn (`turn` are the slots
/// of its rotation), and only then with: so a component that can slide to the place
/// slides, and one that can only get nearer by swinging round swings.
#[allow(clippy::too_many_arguments)]
fn pull(
    system: &System,
    hard: &[u32],
    soft: &[u32],
    turn: [u32; 3],
    scales: &[f64],
    components: &[Placed],
    vals: &mut [f64],
    kept: &mut Vec<Kept>,
    used: &mut Vec<Kept>,
    point: impl Fn(&[f64]) -> DVec3,
    to: DVec3,
    target: usize,
) {
    let miss = |vals: &[f64]| point(vals).distance(to);
    let all: Vec<u32> = hard.iter().chain(soft).copied().collect();
    let hard_vars = unknowns(system, hard, components);
    let exact_signature = signature(system, &hard_vars, hard, false);
    let mut exact = take_kept(kept, used, &exact_signature).unwrap_or_else(|| {
        Problem::new(
            hard_vars.clone(),
            hard.to_vec(),
            &system.eqs,
            Space::Equations,
        )
    });
    // The same, with the dragged component not allowed to turn.
    let mut still_vars = hard_vars;
    still_vars.retain(|v| !turn.contains(v));
    let still_signature = signature(system, &still_vars, hard, false);
    let mut still = take_kept(kept, used, &still_signature)
        .unwrap_or_else(|| Problem::new(still_vars, hard.to_vec(), &system.eqs, Space::Equations));

    let mut dx = Vec::new();
    let mut left = miss(vals);
    let start = left;
    for turning in [false, true] {
        // Sliding got it there, or as good as: it is not turned for the last hair.
        if turning && left < (1e-3 * start).max(1e-6) {
            break;
        }
        let mut vars = unknowns(system, &all, components);
        if !turning {
            vars.retain(|v| !turn.contains(v));
        }
        let loose_signature = signature(system, &vars, &all, true);
        let mut loose = take_kept(kept, used, &loose_signature)
            .unwrap_or_else(|| Problem::new(vars, all.clone(), &system.eqs, Space::Variables));
        for _ in 0..DRAG_STEPS {
            if left < DRAG_NEAR {
                break;
            }
            // Pulled a reach at a time: a pull that is long (and perhaps out of reach)
            // would bend the mates in the step it asks for.
            let from = point(vals);
            let aim = from + (to - from) * (DRAG_REACH / left).min(1.0);
            vals[target..target + 3].copy_from_slice(&aim.to_array());
            if !loose.gauss_newton_step(&system.eqs, vals, &mut dx) {
                break;
            }
            // No component turns far in one step: the step is a straight line, and a
            // turn is not.
            let most = loose
                .vars
                .iter()
                .zip(&dx)
                .filter(|(slot, _)| **slot % 6 >= 3)
                .map(|(slot, d)| d.abs() / scales[*slot as usize / 6])
                .fold(0.0, f64::max);
            let mut part = if most > DRAG_TURN {
                DRAG_TURN / most
            } else {
                1.0
            };
            // The step is halved while it doesn't bring the point nearer (it is a
            // straight line, and overshoots round a curve), or the mates can't be
            // solved from where it ends.
            let before = vals.to_vec();
            let mut nearer = None;
            for _ in 0..DRAG_HALVINGS {
                for (slot, d) in loose.vars.iter().zip(&dx) {
                    vals[*slot as usize] += d * part;
                }
                // The mates, exactly: while sliding, without turning the dragged
                // component if they can be had that way (the step leaves them a little
                // off, and putting that right must not turn it either).
                let stepped = vals.to_vec();
                let mut holds = !turning && still.solve(&system.eqs, vals, HARD).converged;
                if !holds {
                    vals.copy_from_slice(&stepped);
                    holds = exact.solve(&system.eqs, vals, HARD).converged;
                }
                let now = miss(vals);
                if holds && now < left - 1e-9 {
                    nearer = Some(now);
                    break;
                }
                vals.copy_from_slice(&before);
                part *= 0.5;
            }
            match nearer {
                Some(now) => left = now,
                None => break,
            }
        }
        used.push((loose_signature, loose));
    }
    used.push((exact_signature, exact));
    used.push((still_signature, still));
}

/// The group a component is in: the first of the components it is joined to.
fn root(group: &mut [usize], mut i: usize) -> usize {
    while group[i] != i {
        group[i] = group[group[i]];
        i = group[i];
    }
    i
}

/// Below this a pivot counts as nothing: the Jacobian's entries are of order one.
const PIVOT: f64 = 1e-7;

/// Brings `rows` to reduced row echelon form in place. Returns the pivot column of each
/// row that has one, in order.
fn reduce(rows: &mut [Vec<f64>], width: usize) -> Vec<usize> {
    let mut pivots = Vec::new();
    for col in 0..width {
        let rank = pivots.len();
        let Some(pivot) =
            (rank..rows.len()).max_by(|a, b| rows[*a][col].abs().total_cmp(&rows[*b][col].abs()))
        else {
            break;
        };
        if rows[pivot][col].abs() < PIVOT {
            continue;
        }
        rows.swap(rank, pivot);
        let scale = rows[rank][col];
        for x in &mut rows[rank][col..] {
            *x /= scale;
        }
        let lead = rows[rank].clone();
        for (r, row) in rows.iter_mut().enumerate() {
            let factor = row[col];
            if r != rank && factor != 0.0 {
                for (x, l) in row.iter_mut().zip(&lead).skip(col) {
                    *x -= factor * l;
                }
            }
        }
        pivots.push(col);
        if pivots.len() == rows.len() {
            break;
        }
    }
    pivots
}

/// How many ways the components can still move, with the mates that hold (`held`): of
/// them all together, and of each one (by its index).
///
/// Together: six for each free component, less the rank of the mates' Jacobian. Of one
/// component: in how many independent ways it moves among all the motions the mates
/// leave, which is the rank of its six rows of a basis of the Jacobian's null space. A
/// component that only moves along with others counts those motions as its own, so two
/// free components fastened to each other each have six. Each group of mated
/// components is worked out on its own.
fn freedoms(
    system: &System,
    held: &[u32],
    components: &[Placed],
    vals: &[f64],
    group: &mut [usize],
) -> (usize, Vec<usize>) {
    let mut each: Vec<usize> = components
        .iter()
        .map(|c| if c.fixed { 0 } else { 6 })
        .collect();
    let mut total: usize = each.iter().sum();
    // The equations of each group, by the group of a free component they read.
    let mut buckets: Vec<(usize, Vec<u32>)> = Vec::new();
    for e in held {
        let slots = system.eqs[*e as usize].slots;
        let free = [slots[0], slots[6]]
            .into_iter()
            .map(|s| s as usize / 6)
            .find(|i| !components[*i].fixed);
        let Some(free) = free else {
            continue;
        };
        let of = root(group, free);
        match buckets.iter_mut().find(|(g, _)| *g == of) {
            Some((_, eqs)) => eqs.push(*e),
            None => buckets.push((of, vec![*e])),
        }
    }
    let mut g = [0.0; MAX_SLOTS];
    for (_, eqs) in &buckets {
        // A column for each unknown of the group's free components.
        let mut members: Vec<usize> = eqs
            .iter()
            .flat_map(|e| {
                let slots = system.eqs[*e as usize].slots;
                [slots[0] as usize / 6, slots[6] as usize / 6]
            })
            .filter(|i| !components[*i].fixed)
            .collect();
        members.sort_unstable();
        members.dedup();
        let width = members.len() * 6;
        let column = |component: usize| members.binary_search(&component).ok().map(|k| k * 6);
        let mut rows: Vec<Vec<f64>> = eqs
            .iter()
            .map(|e| {
                let eq = &system.eqs[*e as usize];
                eq.eval(vals, &mut g);
                let mut row = vec![0.0; width];
                for (k, slot) in eq.slots.iter().enumerate() {
                    if let Some(start) = column(*slot as usize / 6) {
                        row[start + *slot as usize % 6] += g[k];
                    }
                }
                row
            })
            .collect();
        let pivots = reduce(&mut rows, width);
        total -= pivots.len();
        // The null space: one vector for each column without a pivot, with one in
        // that column and, in each pivot's column, minus that row's entry there.
        let loose: Vec<usize> = (0..width).filter(|c| !pivots.contains(c)).collect();
        for (k, member) in members.iter().enumerate() {
            let mut own: Vec<Vec<f64>> = (k * 6..k * 6 + 6)
                .map(|col| {
                    loose
                        .iter()
                        .map(|f| match pivots.iter().position(|p| *p == col) {
                            Some(row) => -rows[row][*f],
                            None => f64::from(u8::from(col == *f)),
                        })
                        .collect()
                })
                .collect();
            each[*member] = reduce(&mut own, loose.len()).len();
        }
    }
    (total, each)
}

/// A component turned half way round about `axis` through `through`.
fn turned_over(frame: Frame, through: DVec3, axis: DVec3) -> Frame {
    let q = DQuat::from_axis_angle(axis, std::f64::consts::PI);
    Frame {
        origin: through + q * (frame.origin - through),
        rotation: (q * frame.rotation).normalize(),
    }
}

/// Solves the mates. `components` are those that are not suppressed, where they are;
/// `mates` are in the assembly's order. `key` identifies the mates and the components
/// (not where they are): what `memo` remembers is used again while it stays the same.
/// `drag` pulls a component as the mates are solved.
pub(crate) fn solve(
    mates: &[&Mate],
    mut components: Vec<Placed>,
    params: &Parameters,
    key: u64,
    memo: &mut Memo,
    drag: Option<Drag>,
) -> Solved {
    if memo.key != key {
        *memo = Memo {
            key,
            ..Memo::default()
        };
    }
    let index: HashMap<CompId, usize> = components
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id, i))
        .collect();
    let mut statuses = HashMap::new();

    // ---- What each mate asks for ----
    let mut asked: Vec<(MateId, usize, usize, Vec<Proto>)> = Vec::new();
    for mate in mates {
        if mate.suppressed {
            statuses.insert(mate.id, Status::Suppressed);
            continue;
        }
        let find = |end: &MateEnd| end.component().and_then(|c| index.get(&c).copied());
        // On a component that is suppressed (or gone): the mate waits for it.
        let (Some(ia), Some(ib)) = (find(&mate.a), find(&mate.b)) else {
            statuses.insert(mate.id, Status::Suppressed);
            continue;
        };
        let built = (|| {
            if ia == ib {
                return Err(format!(
                    "Both ends are on {}: a mate joins two components.",
                    components[ia].name
                ));
            }
            let a = element(&mate.a, &components[ia])?;
            let b = element(&mate.b, &components[ib])?;
            let protos = equations(&mate.kind, a, b, params)?;
            Ok((a, b, protos))
        })();
        let (a, b, protos) = match built {
            Ok(built) => built,
            Err(message) => {
                statuses.insert(mate.id, Status::Failed(message));
                continue;
            }
        };
        // Two planes the wrong way round: turn one over first (see the module docs).
        if let (Element::Plane(pa, na), Element::Plane(pb, nb)) = (a, b)
            && matches!(
                mate.kind,
                MateKind::Coincident | MateKind::Distance(_) | MateKind::Parallel
            )
        {
            let (fa, fb) = (components[ia].frame, components[ib].frame);
            let (wa, wb) = (fa.vector_to_world(na), fb.vector_to_world(nb));
            let wanted = if mate.flip { 1.0 } else { -1.0 };
            if wa.dot(wb) * wanted < 0.0 {
                let axis = wa
                    .cross(wb)
                    .try_normalize()
                    .unwrap_or_else(|| wb.any_orthonormal_vector());
                if !components[ib].fixed {
                    components[ib].frame = turned_over(fb, fb.to_world(pb), axis);
                } else if !components[ia].fixed {
                    components[ia].frame = turned_over(fa, fa.to_world(pa), axis);
                }
            }
        }
        asked.push((mate.id, ia, ib, protos));
    }

    // ---- The equations, from where the components are now ----
    let sides: Vec<(DQuat, f64)> = components
        .iter()
        .map(|c| (c.frame.rotation, c.size()))
        .collect();
    let mut system = System {
        eqs: Vec::new(),
        ranges: Vec::new(),
    };
    for (id, ia, ib, protos) in &asked {
        let start = system.eqs.len();
        for p in protos {
            let (i1, i2) = if p.first_is_a { (*ia, *ib) } else { (*ib, *ia) };
            let mut slots = [0; MAX_SLOTS];
            slots[..6].copy_from_slice(&slots_of(i1));
            slots[6..].copy_from_slice(&slots_of(i2));
            let side = |i: usize, dir, point| Side {
                base: sides[i].0,
                scale: sides[i].1,
                dir,
                point,
            };
            system.eqs.push(Eq {
                kind: p.kind,
                slots,
                first: side(i1, p.d1, p.p1),
                other: side(i2, p.d2, p.p2),
                target: p.target,
                weight: 1.0,
            });
        }
        system.ranges.push((*id, start..system.eqs.len()));
    }

    // ---- The groups: components joined by mates, a fixed one joining nothing ----
    let mut group: Vec<usize> = (0..components.len()).collect();
    for (_, ia, ib, _) in &asked {
        if !components[*ia].fixed && !components[*ib].fixed {
            let (a, b) = (root(&mut group, *ia), root(&mut group, *ib));
            group[a.max(b)] = a.min(b);
        }
    }
    // The mates of each group, in the assembly's order. A mate between two fixed
    // components is a group of its own: there is nothing to solve, only to check.
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    for (k, (_, ia, ib, _)) in asked.iter().enumerate() {
        let of = if !components[*ia].fixed {
            root(&mut group, *ia)
        } else if !components[*ib].fixed {
            root(&mut group, *ib)
        } else {
            components.len() + k
        };
        match groups.iter_mut().find(|(g, _)| *g == of) {
            Some((_, mates)) => mates.push(k),
            None => groups.push((of, vec![k])),
        }
    }

    // ---- Solve each group: all its mates together, or one at a time if they can't
    // all hold ----
    let mut vals = start_values(&components);
    let mut held: Vec<u32> = Vec::new();
    let mut kept = std::mem::take(&mut memo.problems);
    let mut used = Vec::new();
    let mates_equations = system.eqs.len();

    // ---- A drag: the component is pulled before its group is solved ----
    let mut dragged = None;
    if let Some(drag) = drag
        && let Some(&i) = index.get(&drag.component)
        && !components[i].fixed
    {
        let of = root(&mut group, i);
        let hard: Vec<u32> = groups
            .iter()
            .filter(|(g, _)| *g == of)
            .flat_map(|(_, mates)| mates.iter())
            .flat_map(|k| system.ranges[*k].1.clone().map(|e| e as u32))
            .collect();
        if hard.is_empty() {
            // Nothing holds it: it just goes there.
            let by = drag.to - components[i].frame.to_world(drag.point);
            for (v, d) in vals[i * 6..i * 6 + 3].iter_mut().zip(by.to_array()) {
                *v += d;
            }
        } else {
            // The place to go to, as a component that doesn't move, after the others.
            let ground = components.len();
            vals.extend(drag.to.to_array());
            vals.extend([0.0; 3]);
            let fixed = Side {
                base: DQuat::IDENTITY,
                scale: 1.0,
                dir: DVec3::ZERO,
                point: DVec3::ZERO,
            };
            let own = Side {
                base: sides[i].0,
                scale: sides[i].1,
                dir: DVec3::ZERO,
                point: drag.point,
            };
            let mut slots = [0; MAX_SLOTS];
            slots[..6].copy_from_slice(&slots_of(ground));
            slots[6..].copy_from_slice(&slots_of(i));
            for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                system.eqs.push(Eq {
                    kind: EqKind::Offset,
                    slots,
                    first: Side { dir: axis, ..fixed },
                    other: own,
                    target: 0.0,
                    weight: PULL,
                });
            }
            let soft: Vec<u32> = (mates_equations..system.eqs.len())
                .map(|e| e as u32)
                .collect();
            let (base, scale) = sides[i];
            let point = |vals: &[f64]| {
                let v = &vals[i * 6..i * 6 + 6];
                let turn = DQuat::from_scaled_axis(DVec3::new(v[3], v[4], v[5]) / scale);
                DVec3::new(v[0], v[1], v[2]) + turn * base * drag.point
            };
            let own_slots = slots_of(i);
            let scales: Vec<f64> = sides.iter().map(|s| s.1).collect();
            pull(
                &system,
                &hard,
                &soft,
                [own_slots[3], own_slots[4], own_slots[5]],
                &scales,
                &components,
                &mut vals,
                &mut kept,
                &mut used,
                point,
                drag.to,
                ground * 6,
            );
            dragged = Some(of);
        }
    }

    for (of, mates) in &groups {
        let eqs_of = |k: usize| system.ranges[k].1.clone().map(|e| e as u32);
        let all: Vec<u32> = mates.iter().flat_map(|k| eqs_of(*k)).collect();
        let before = vals.clone();
        if solve_equations(
            &system,
            &all,
            &components,
            &mut vals,
            Some((&mut kept, &mut used)),
            dragged == Some(*of),
        ) {
            for k in mates {
                statuses.insert(system.ranges[*k].0, Status::Ok);
            }
            held.extend(all);
            continue;
        }
        vals = before;
        let mut holding: Vec<u32> = Vec::new();
        for k in mates {
            let before = vals.clone();
            let mut with = holding.clone();
            with.extend(eqs_of(*k));
            if solve_equations(&system, &with, &components, &mut vals, None, false) {
                holding = with;
                statuses.insert(system.ranges[*k].0, Status::Ok);
            } else {
                vals = before;
                statuses.insert(
                    system.ranges[*k].0,
                    Status::Failed(
                        "It can't hold together with the mates above it: the components would have to be in two places at once. Change or delete this mate, or one of those."
                            .to_owned(),
                    ),
                );
            }
        }
        held.extend(holding);
    }
    memo.problems = used;

    // ---- Where the components are now ----
    let all_hold =
        held.len() == mates_equations && statuses.values().all(|s| !matches!(s, Status::Failed(_)));
    let known = memo
        .freedom
        .take()
        .filter(|(_, each)| all_hold && each.len() == components.len());
    let (freedom, each) =
        known.unwrap_or_else(|| freedoms(&system, &held, &components, &vals, &mut group));
    let component_freedom = components
        .iter()
        .zip(&each)
        .map(|(c, f)| (c.id, *f))
        .collect();
    if all_hold {
        memo.freedom = Some((freedom, each));
    }
    let mut frames = HashMap::with_capacity(components.len());
    for (i, c) in components.iter().enumerate() {
        let v = &vals[i * 6..i * 6 + 6];
        let origin = DVec3::new(v[0], v[1], v[2]);
        let w = DVec3::new(v[3], v[4], v[5]) / sides[i].1;
        // One the solve didn't touch is exactly where it was.
        let frame = if w == DVec3::ZERO && origin == c.frame.origin {
            c.frame
        } else {
            Frame {
                origin,
                rotation: (DQuat::from_scaled_axis(w) * c.frame.rotation).normalize(),
            }
        };
        frames.insert(c.id, frame);
    }
    Solved {
        frames,
        statuses,
        freedom,
        component_freedom,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gradients of every kind of equation agree with finite differences, at
    /// rotations that are not small.
    #[test]
    fn gradients_match_finite_differences() {
        let side = |seed: f64| Side {
            base: DQuat::from_euler(peet_math::EulerRot::XYZ, 0.3 * seed, -0.7, 1.1 * seed),
            scale: 40.0 + seed,
            dir: DVec3::new(0.3, -0.5 * seed, 0.8).normalize(),
            point: DVec3::new(12.0 * seed, -7.0, 30.0),
        };
        let vals = [
            5.0, -3.0, 8.0, 20.0, -35.0, 50.0, // a: moved, and turned well away from base
            -40.0, 22.0, 6.0, -60.0, 15.0, 33.0,
        ];
        for kind in [
            EqKind::Dot,
            EqKind::Offset,
            EqKind::Distance,
            EqKind::LineDistance,
        ] {
            let mut slots = [0; MAX_SLOTS];
            for (i, s) in slots.iter_mut().enumerate() {
                *s = i as u32;
            }
            let eq = Eq {
                kind,
                slots,
                first: side(1.0),
                other: side(2.0),
                target: 0.25,
                weight: 0.5,
            };
            let mut g = [0.0; MAX_SLOTS];
            eq.eval(&vals, &mut g);
            for k in 0..MAX_SLOTS {
                let h = 1e-5;
                let (mut up, mut down) = (vals, vals);
                up[k] += h;
                down[k] -= h;
                let numeric = (eq.residual(&up) - eq.residual(&down)) / (2.0 * h);
                assert!(
                    (numeric - g[k]).abs() < 1e-7,
                    "{kind:?} slot {k}: {numeric} vs {}",
                    g[k]
                );
            }
        }
    }
}
